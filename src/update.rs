//! `update`: replace this binary with the latest GitHub release.

use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use crate::atomic::{create_private, sync_dir};
use crate::install;

/// The release asset for this OS and CPU, or `None` where the release holds
/// none. The names are the ones `scripts/install.sh` downloads.
fn asset_name(os: &str, arch: &str) -> Option<&'static str> {
    let x86_64 = arch == "x86_64";
    match os {
        "linux" if x86_64 => Some("astlens-linux"),
        // A universal binary, for both arm64 and x86_64.
        "macos" | "darwin" if x86_64 || arch == "aarch64" => Some("astlens-macos"),
        "windows" if x86_64 => Some("astlens-windows.exe"),
        _ => None,
    }
}

/// The asset for the platform this binary was built for, for `update` to
/// download and `doctor` to name.
pub(crate) fn current_asset() -> Option<&'static str> {
    asset_name(std::env::consts::OS, std::env::consts::ARCH)
}

/// `tag_name` from the `releases/latest` reply, without its quotes.
fn parse_tag(json: &str) -> Option<&str> {
    let at = json.find("\"tag_name\"")? + "\"tag_name\"".len();
    let rest = json[at..].trim_start().strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(&rest[..end])
}

/// `v0.7.0` or `0.7.0` as three numbers. Anything else, such as a
/// prerelease tag, is not a version this command can compare.
fn parse_version(tag: &str) -> Option<(u64, u64, u64)> {
    let tag = tag.strip_prefix('v').unwrap_or(tag);
    let mut parts = tag.split('.');
    let mut next = || {
        let part = parts.next()?;
        // Digits only: `1x` would otherwise parse as 1.
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        part.parse().ok()
    };
    let version = (next()?, next()?, next()?);
    parts.next().is_none().then_some(version)
}

/// Whether the release is a later version than the one running.
fn is_newer(tag: &str, current: &str) -> bool {
    match (parse_version(tag), parse_version(current)) {
        (Some(release), Some(current)) => release > current,
        _ => false,
    }
}

/// A tag without its `v`, as the messages spell a version.
fn version_of(tag: &str) -> &str {
    tag.strip_prefix('v').unwrap_or(tag)
}

/// The hash `SHA256SUMS` gives the asset, from its second field: a binary
/// prefix `*` marks the line as binary mode, as `sha256sum -b` writes it.
fn expected_hash(sums: &str, asset: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let hash = fields.next()?;
        let same = fields.next().is_some_and(|name| {
            let name = name.strip_prefix('*').unwrap_or(name);
            name == asset
        });
        (same && is_sha256(hash)).then(|| hash.to_ascii_lowercase())
    })
}

/// 64 hexadecimal digits, the length a SHA-256 hash has.
fn is_sha256(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn run(check_only: bool) -> Result<()> {
    let exe = current_exe()?;
    remove_stale_copies(&exe);
    let current = env!("CARGO_PKG_VERSION");

    let Some(asset) = current_asset() else {
        bail!(
            "no prebuilt binary for {} {}; build from source instead: scripts/install.sh --from-source",
            std::env::consts::OS,
            std::env::consts::ARCH
        );
    };

    // A binary cargo or a package manager owns is reported, never written:
    // their records name a version this download would make wrong.
    let owner = install::owner(&exe);
    if owner != install::Owner::Script && owner != install::Owner::User {
        let tag = latest_tag()?;
        if !is_newer(&tag, current) {
            println!("astlens v{current} is the latest");
            return Ok(());
        }
        match owner {
            install::Owner::Cargo => bail!(
                "v{} is available; this binary came from cargo, so run: cargo install --force astlens",
                version_of(&tag)
            ),
            _ => bail!(
                "v{} is available; use the package manager that installed {}",
                version_of(&tag),
                exe.display()
            ),
        }
    }

    let tag = latest_tag()?;
    if !is_newer(&tag, current) {
        println!("astlens v{current} is the latest");
        return Ok(());
    }
    if check_only {
        println!("v{} is available (you have v{current})", version_of(&tag));
        return Ok(());
    }

    println!("Updating v{current} -> v{}", version_of(&tag));
    let sums = fetch(&format!("{BASE}/{tag}/SHA256SUMS"), MAX_SUMS)?;
    let sums = String::from_utf8_lossy(&sums);
    let expected = expected_hash(&sums, asset)
        .with_context(|| format!("{asset} is not listed in the release's SHA256SUMS"))?;
    let download = fetch(&format!("{BASE}/{tag}/{asset}"), MAX_BINARY)?;
    let actual = hex(&Sha256::digest(&download));
    if actual != expected {
        bail!("checksum mismatch for {asset}: expected {expected}, downloaded {actual}");
    }
    install_download(&exe, &download)
        .with_context(|| format!("cannot replace {}", exe.display()))?;
    println!("Updated v{current} -> v{}", version_of(&tag));
    install::report_other_copy(&exe);
    Ok(())
}

/// A digest as the lower-case text `SHA256SUMS` holds.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Where the release assets live, and the tag of the latest one.
const BASE: &str = "https://github.com/aeron2k4/astlens/releases";
const LATEST: &str = "https://api.github.com/repos/aeron2k4/astlens/releases/latest";
/// A checksum file is a few hundred bytes; a binary is tens of megabytes.
const MAX_SUMS: u64 = 64 * 1024;
const MAX_BINARY: u64 = 256 * 1024 * 1024;

/// The running binary, with symlinks resolved: the file a rename must replace.
fn current_exe() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("cannot locate this binary")?;
    Ok(exe.canonicalize().unwrap_or(exe))
}

fn latest_tag() -> Result<String> {
    let reply = fetch(LATEST, MAX_SUMS)?;
    let reply = String::from_utf8_lossy(&reply);
    parse_tag(&reply)
        .map(str::to_string)
        .context("the latest release reply carries no tag_name")
}

fn fetch(url: &str, limit: u64) -> Result<Vec<u8>> {
    let mut response = ureq::get(url)
        .header("User-Agent", concat!("astlens/", env!("CARGO_PKG_VERSION")))
        .call()
        .with_context(|| format!("cannot reach {url}"))?;
    response
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .with_context(|| format!("cannot read the reply from {url}"))
}

/// Put `bytes` at `exe` through a temporary file in the same directory, so
/// an interrupted write never leaves a half-written binary where the old one
/// worked. A binary the user cannot write is refused before anything is
/// written, and the rename gives the new file the old one's permissions.
fn install_download(exe: &Path, bytes: &[u8]) -> io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = exe
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = exe
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not a file"))?;
    let temporary = dir.join(format!(
        ".{}.new-{}-{}",
        name.to_string_lossy(),
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let written = (|| {
        let mut file = create_private(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&temporary, fs::Permissions::from_mode(0o755))?;
        }
        replace_file(&temporary, exe)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written?;
    sync_dir(dir);
    Ok(())
}

/// Move `source` over `target`. Windows refuses to replace a running
/// executable, but lets it be renamed first; the file left behind is removed
/// on a later run.
fn replace_file(source: &Path, target: &Path) -> io::Result<()> {
    match fs::rename(source, target) {
        Ok(()) => Ok(()),
        #[cfg(windows)]
        Err(err) => {
            let _ = err;
            let old = target.with_extension(format!("old-{}", std::process::id()));
            fs::rename(target, &old)?;
            fs::rename(source, target)
        }
        #[cfg(not(windows))]
        Err(err) => Err(err),
    }
}

/// Remove the copies `replace_file` left on Windows, whose process is gone.
fn remove_stale_copies(exe: &Path) {
    let (Some(dir), Some(name)) = (exe.parent(), exe.file_name()) else {
        return;
    };
    let prefix = format!("{}.old-", name.to_string_lossy());
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let left = entry.file_name();
        if left.to_string_lossy().starts_with(&prefix) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_platform_has_the_asset_the_script_downloads() {
        assert_eq!(asset_name("linux", "x86_64"), Some("astlens-linux"));
        assert_eq!(asset_name("macos", "aarch64"), Some("astlens-macos"));
        assert_eq!(asset_name("macos", "x86_64"), Some("astlens-macos"));
        assert_eq!(asset_name("windows", "x86_64"), Some("astlens-windows.exe"));
    }

    #[test]
    fn a_platform_without_a_release_has_no_asset() {
        assert_eq!(asset_name("linux", "aarch64"), None);
        assert_eq!(asset_name("windows", "aarch64"), None);
        assert_eq!(asset_name("macos", "riscv64"), None);
        assert_eq!(asset_name("freebsd", "x86_64"), None);
    }

    #[test]
    fn the_tag_comes_out_of_the_release_reply() {
        let reply = r#"{
  "url": "https://api.github.com/repos/aeron2k4/astlens/releases/1",
  "tag_name": "v0.7.0",
  "name": "astlens 0.7.0",
  "draft": false
}"#;
        assert_eq!(parse_tag(reply), Some("v0.7.0"));
        assert_eq!(parse_tag(r#"{"tag_name":"v1.2.3"}"#), Some("v1.2.3"));
        assert_eq!(parse_tag(r#"{"tag_name" :  "v1.2.3" }"#), Some("v1.2.3"));
        assert_eq!(parse_tag(r#"{"name": "v1.2.3"}"#), None);
        assert_eq!(parse_tag("<html>"), None);
    }

    #[test]
    fn a_version_is_three_numbers() {
        assert_eq!(parse_version("v0.7.0"), Some((0, 7, 0)));
        assert_eq!(parse_version("0.6.13"), Some((0, 6, 13)));
        assert_eq!(parse_version("1.0.0"), Some((1, 0, 0)));
        for bad in [
            "v0.7",
            "v0.7.0.1",
            "v0.7.0-rc1",
            "vx.7.0",
            "v1x.0.0",
            "",
            "v",
        ] {
            assert_eq!(parse_version(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_later_release_is_newer() {
        assert!(is_newer("v0.7.0", "0.6.9"));
        assert!(is_newer("v0.10.0", "0.9.9"));
        assert!(is_newer("v1.0.0", "0.99.99"));
        assert!(!is_newer("v0.6.3", "0.6.3"));
        assert!(!is_newer("v0.6.2", "0.6.3"));
        // Nothing to compare against: never claims a newer version.
        assert!(!is_newer("garbage", "0.6.3"));
        assert!(!is_newer("v0.7.0", "garbage"));
    }

    #[test]
    fn the_sums_name_the_hash_of_one_asset() {
        let sums = concat!(
            "1111111111111111111111111111111111111111111111111111111111111111  astlens-macos\n",
            "2222222222222222222222222222222222222222222222222222222222222222  astlens-linux\n",
            "3333333333333333333333333333333333333333333333333333333333333333 *astlens-windows.exe\n",
        );
        assert_eq!(
            expected_hash(sums, "astlens-linux").as_deref(),
            Some("2222222222222222222222222222222222222222222222222222222222222222")
        );
        // The `*` of binary mode is not part of the name.
        assert_eq!(
            expected_hash(sums, "astlens-windows.exe").as_deref(),
            Some("3333333333333333333333333333333333333333333333333333333333333333")
        );
        assert_eq!(expected_hash(sums, "astlens-freebsd"), None);
    }

    #[test]
    fn a_sums_line_that_is_not_a_hash_is_no_answer() {
        let short = "abc  astlens-linux";
        assert_eq!(expected_hash(short, "astlens-linux"), None);
        let not_hex = format!("{}  astlens-linux", "z".repeat(64));
        assert_eq!(expected_hash(&not_hex, "astlens-linux"), None);
        // Upper case is the same hash.
        let upper = format!("{}  astlens-linux", "A".repeat(64));
        assert_eq!(expected_hash(&upper, "astlens-linux"), Some("a".repeat(64)));
        assert_eq!(expected_hash("", "astlens-linux"), None);
    }

    #[test]
    fn the_download_replaces_the_binary_and_keeps_its_mode() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("astlens");
        fs::write(&exe, b"old").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        }

        install_download(&exe, b"new binary").unwrap();
        assert_eq!(fs::read(&exe).unwrap(), b"new binary");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = fs::metadata(&exe).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o755);
        }
        // No temporary file is left next to it.
        let left: Vec<_> = fs::read_dir(dir.path()).unwrap().flatten().collect();
        assert_eq!(
            left.len(),
            1,
            "{:?}",
            left.iter().map(|e| e.path()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_binary_that_cannot_be_written_is_left_intact() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("astlens");
        fs::write(&exe, b"old").unwrap();
        // The directory refuses new files, so no temporary file can be made.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o555)).unwrap();
            if fs::write(dir.path().join("probe"), b"").is_ok() {
                // Running as root: the mode proves nothing.
                let _ = fs::remove_file(dir.path().join("probe"));
                return;
            }
            assert!(install_download(&exe, b"new").is_err());
            assert_eq!(fs::read(&exe).unwrap(), b"old");
            fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn stale_copies_of_a_replaced_binary_go_on_the_next_run() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("astlens");
        fs::write(&exe, b"new").unwrap();
        let stale = dir.path().join("astlens.old-1234");
        fs::write(&stale, b"old").unwrap();
        let other = dir.path().join("astlens2");
        fs::write(&other, b"kept").unwrap();

        remove_stale_copies(&exe);
        assert!(!stale.exists());
        assert!(exe.exists());
        assert!(other.exists());
    }

    #[test]
    fn a_digest_reads_like_the_sums_file() {
        assert_eq!(
            hex(&Sha256::digest(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(&Sha256::digest(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn this_build_has_an_asset_where_the_release_does() {
        // Only the targets release.yml builds; elsewhere --from-source is the way.
        let expected = matches!(
            (std::env::consts::OS, std::env::consts::ARCH),
            ("linux", "x86_64") | ("macos", _) | ("windows", "x86_64")
        );
        assert_eq!(current_asset().is_some(), expected);
    }
}
