//! Git integration: `.gitignore`-aware file walking and working-tree status.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use ignore::WalkBuilder;
use serde::Serialize;

/// List files under `root`, honouring `.gitignore`, `.ignore` and git
/// excludes. Paths are relative to `root` and sorted. Stops after `limit`.
pub fn list_files(root: &Path, limit: usize) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        // Respect .gitignore even outside a git repository.
        .require_git(false)
        .sort_by_file_path(|a, b| a.cmp(b))
        .build()
        .filter_map(|entry| match entry {
            Ok(e) => Some(e),
            Err(err) => {
                tracing::debug!(%err, "skipping unreadable entry");
                None
            }
        })
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .filter_map(|e| e.path().strip_prefix(root).ok().map(Path::to_path_buf))
        .take(limit)
        .collect();
    files.sort();
    files
}

/// Git blob object id of `content`, identical to `git hash-object`.
pub fn blob_hash(content: &[u8]) -> String {
    let mut hasher = sha1_smol::Sha1::new();
    hasher.update(format!("blob {}\0", content.len()).as_bytes());
    hasher.update(content);
    hasher.digest().to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileStatus {
    pub path: String,
    /// Two-letter porcelain code, e.g. `M `, ` M`, `??`, `R `.
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub renamed_from: Option<String>,
}

/// Working-tree status via `git status --porcelain`. Errors when `root` is
/// not inside a git repository or git is unavailable.
pub fn status(root: &Path) -> Result<Vec<FileStatus>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        .output()
        .context("failed to run git")?;
    if !output.status.success() {
        bail!(
            "git status failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(parse_porcelain(&String::from_utf8_lossy(&output.stdout)))
}

fn parse_porcelain(raw: &str) -> Vec<FileStatus> {
    let mut out = Vec::new();
    let mut fields = raw.split('\0').filter(|f| !f.is_empty());
    while let Some(entry) = fields.next() {
        if entry.len() < 4 {
            continue;
        }
        let (code, path) = entry.split_at(2);
        let path = path[1..].to_string();
        // Renames and copies carry the original path as the next field.
        let renamed_from = if code.contains('R') || code.contains('C') {
            fields.next().map(str::to_string)
        } else {
            None
        };
        out.push(FileStatus {
            path,
            code: code.to_string(),
            renamed_from,
        });
    }
    out
}

/// Changed line ranges of one file in a diff, as `(start, count)` pairs on
/// each side; a count of 0 marks a pure insertion or deletion point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    /// Path on the new side relative to `root`, or the old path when deleted.
    pub path: String,
    pub old: Vec<(usize, usize)>,
    pub new: Vec<(usize, usize)>,
    pub added: bool,
    pub deleted: bool,
}

/// Line-level diff of the working tree (or the index when `staged`) against
/// `base`, limited to files under `root`, with paths relative to it.
pub fn diff(root: &Path, base: &str, staged: bool) -> Result<Vec<FileDiff>> {
    let mut cmd = Command::new("git");
    // Fixed prefixes and unquoted paths whatever the user's git config says.
    cmd.arg("-C").arg(root).args([
        "-c",
        "core.quotePath=false",
        "diff",
        "--src-prefix=a/",
        "--dst-prefix=b/",
        "--no-color",
        "--no-ext-diff",
        "--no-renames",
        "--unified=0",
        "--relative",
    ]);
    if staged {
        cmd.arg("--cached");
    }
    let output = cmd
        .arg(base)
        .arg("--")
        .output()
        .context("failed to run git")?;
    if !output.status.success() {
        bail!(
            "git diff failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(parse_diff(&String::from_utf8_lossy(&output.stdout)))
}

fn parse_diff(raw: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut old_path = None;
    for line in raw.lines() {
        if line.starts_with("diff --git ") {
            old_path = None;
        } else if let Some(p) = line.strip_prefix("--- ") {
            old_path = header_path(p, "a/");
        } else if let Some(p) = line.strip_prefix("+++ ") {
            let new_path = header_path(p, "b/");
            let deleted = new_path.is_none();
            let Some(path) = new_path.or_else(|| old_path.take()) else {
                continue;
            };
            files.push(FileDiff {
                path,
                old: Vec::new(),
                new: Vec::new(),
                added: old_path.is_none() && !deleted,
                deleted,
            });
        } else if let Some(header) = line.strip_prefix("@@ ")
            && let Some(file) = files.last_mut()
        {
            // @@ -a[,b] +c[,d] @@ context
            let mut parts = header.split_whitespace();
            let range = |part: Option<&str>, sign: char| {
                let spec = part?.strip_prefix(sign)?;
                let (start, count) = spec.split_once(',').unwrap_or((spec, "1"));
                Some((start.parse().ok()?, count.parse().ok()?))
            };
            if let (Some(old), Some(new)) = (range(parts.next(), '-'), range(parts.next(), '+')) {
                file.old.push(old);
                file.new.push(new);
            }
        }
    }
    files
}

/// The path in a `---` or `+++` line. Git ends the line with a tab when the
/// path has a space; a path that itself holds a tab is quoted instead.
fn header_path(text: &str, prefix: &str) -> Option<String> {
    let path = text.strip_prefix(prefix)?;
    Some(path.strip_suffix('\t').unwrap_or(path).to_string())
}

/// Contents of `path` (relative to `root`) at `rev`, or in the index when
/// `rev` is empty; `None` when it does not exist there.
pub fn show(root: &Path, rev: &str, path: &str) -> Result<Option<String>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("show")
        .arg(format!("{rev}:./{path}"))
        .output()
        .context("failed to run git")?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(String::from_utf8(output.stdout).ok())
}

/// Untracked, non-ignored files under `root`, relative to it.
pub fn untracked(root: &Path) -> Result<Vec<String>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "--others", "--exclude-standard", "-z"])
        .output()
        .context("failed to run git")?;
    if !output.status.success() {
        bail!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn walk_respects_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join(".gitignore"), "target/\n*.log\n").unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("target")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(root.join("target/out.rs"), "").unwrap();
        fs::write(root.join("debug.log"), "").unwrap();
        fs::write(root.join("README.md"), "").unwrap();

        let files = list_files(root, 100);
        assert_eq!(
            files,
            vec![PathBuf::from("README.md"), PathBuf::from("src/main.rs")]
        );
        assert_eq!(list_files(root, 1).len(), 1);
    }

    #[test]
    fn blob_hash_matches_git() {
        assert_eq!(blob_hash(b""), "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391");
        assert_eq!(
            blob_hash(b"hello\n"),
            "ce013625030ba8dba906f756967f9e9ca394464a"
        );
    }

    #[test]
    fn parses_porcelain_entries() {
        let raw = " M src/a.rs\0?? new.txt\0R  b.rs\0old_b.rs\0";
        let parsed = parse_porcelain(raw);
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].code, " M");
        assert_eq!(parsed[0].path, "src/a.rs");
        assert_eq!(parsed[1].code, "??");
        assert_eq!(parsed[2].path, "b.rs");
        assert_eq!(parsed[2].renamed_from.as_deref(), Some("old_b.rs"));
    }

    #[test]
    fn parses_unified_zero_diff() {
        let raw = "diff --git a/src/a.rs b/src/a.rs\nindex 1..2 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -3 +3,2 @@ fn f() {\n-x\n+y\n+z\n@@ -10,2 +11,0 @@\n-a\n-b\ndiff --git a/new.py b/new.py\nnew file mode 100644\n--- /dev/null\n+++ b/new.py\n@@ -0,0 +1,2 @@\n+def f():\n+    pass\ndiff --git a/old.go b/old.go\ndeleted file mode 100644\n--- a/old.go\n+++ /dev/null\n@@ -1 +0,0 @@\n-package p\n";
        let files = parse_diff(raw);
        assert_eq!(
            files,
            vec![
                FileDiff {
                    path: "src/a.rs".into(),
                    old: vec![(3, 1), (10, 2)],
                    new: vec![(3, 2), (11, 0)],
                    added: false,
                    deleted: false,
                },
                FileDiff {
                    path: "new.py".into(),
                    old: vec![(0, 0)],
                    new: vec![(1, 2)],
                    added: true,
                    deleted: false,
                },
                FileDiff {
                    path: "old.go".into(),
                    old: vec![(1, 1)],
                    new: vec![(0, 0)],
                    added: false,
                    deleted: true,
                },
            ]
        );
    }

    #[test]
    fn diff_paths_with_spaces_lose_the_trailing_tab() {
        let raw = "diff --git a/my file.rs b/my file.rs\n\
                   --- a/my file.rs\t\n\
                   +++ b/my file.rs\t\n\
                   @@ -1 +1 @@\n";
        let files = parse_diff(raw);
        assert_eq!(files[0].path, "my file.rs");
        assert!(!files[0].added && !files[0].deleted);

        let raw = "diff --git a/my file.rs b/my file.rs\n\
                   --- a/my file.rs\t\n\
                   +++ /dev/null\n\
                   @@ -1 +0,0 @@\n";
        let files = parse_diff(raw);
        assert_eq!(files[0].path, "my file.rs");
        assert!(files[0].deleted);
    }

    #[test]
    fn status_outside_repo_is_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(status(dir.path()).is_err());
    }
}
