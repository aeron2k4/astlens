//! OpenCode user configuration: the MCP server entry in `opencode.json(c)`.
//!
//! The file may contain comments and trailing commas, so it is edited through
//! a CST rather than parsed and serialised: everything we do not own stays as
//! the user wrote it.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use jsonc_parser::ParseOptions;
use jsonc_parser::cst::CstRootNode;
use jsonc_parser::json;

use crate::atomic::write_atomic;

pub const SERVER: &str = "astlens";

/// OpenCode reads `opencode.jsonc` before `opencode.json`; use whichever the
/// user already has, and create the plain JSON file otherwise.
pub fn config_path(dir: &Path) -> PathBuf {
    let jsonc = dir.join("opencode.jsonc");
    if jsonc.exists() {
        jsonc
    } else {
        dir.join("opencode.json")
    }
}

fn read(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("cannot read {}", path.display())),
    }
}

fn parse(path: &Path, text: &str) -> Result<CstRootNode> {
    CstRootNode::parse(text, &ParseOptions::default())
        .map_err(|err| anyhow!("{} is not valid JSONC: {err}", path.display()))
}

fn write(path: &Path, original: Option<&str>, edited: &str) -> Result<()> {
    if original == Some(edited) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    if original.is_some() {
        let backup = path.with_extension(format!(
            "{}.bak",
            path.extension()
                .map(|extension| extension.to_string_lossy())
                .unwrap_or_default()
        ));
        fs::copy(path, &backup).with_context(|| format!("cannot write {}", backup.display()))?;
    }
    write_atomic(path, edited).with_context(|| format!("cannot write {}", path.display()))
}

fn finish(original: &str, root: &CstRootNode) -> String {
    let mut text = root.to_string();
    if original.ends_with('\n') && !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// Add or replace `mcp.servers.astlens`, leaving other servers and comments
/// alone. The server runs in OpenCode's workspace directory, so it needs no
/// root argument; `codemode: false` exposes its tools directly.
pub fn add_server(config: &Path, command: &str) -> Result<()> {
    let original = read(config)?;
    let text = original.as_deref().unwrap_or("{\n}\n");
    let root = parse(config, text)?;
    let object = root
        .object_value_or_create()
        .ok_or_else(|| anyhow!("{} is not a JSON object", config.display()))?;
    let mcp = object
        .object_value_or_create("mcp")
        .ok_or_else(|| anyhow!("{}: 'mcp' is not an object", config.display()))?;
    let servers = mcp
        .object_value_or_create("servers")
        .ok_or_else(|| anyhow!("{}: 'mcp.servers' is not an object", config.display()))?;
    let value = json!({
        "type": "local",
        "command": [command, "--serve"],
        "codemode": false,
    });
    match servers.get(SERVER) {
        Some(property) => property.set_value(value),
        None => {
            servers.append(SERVER, value);
        }
    }
    write(config, original.as_deref(), &finish(text, &root))
}

/// Remove only `mcp.servers.astlens`. A clean or absent config is not an
/// error, so uninstall stays idempotent.
pub fn remove_server(config: &Path) -> Result<()> {
    let Some(original) = read(config)? else {
        return Ok(());
    };
    let root = parse(config, &original)?;
    let Some(object) = root.object_value() else {
        return Ok(());
    };
    let Some(mcp) = object.object_value("mcp") else {
        return Ok(());
    };
    let Some(servers) = mcp.object_value("servers") else {
        return Ok(());
    };
    let Some(property) = servers.get(SERVER) else {
        return Ok(());
    };
    property.remove();
    write(config, Some(&original), &finish(&original, &root))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(dir: &Path) -> PathBuf {
        dir.join("opencode.jsonc")
    }

    #[test]
    fn a_missing_config_is_created() {
        let dir = tempfile::tempdir().unwrap();
        let config = file(dir.path());
        add_server(&config, "/bin/astlens").unwrap();
        let value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&config).unwrap()).unwrap();
        assert_eq!(
            value["mcp"]["servers"]["astlens"],
            serde_json::json!({
                "type": "local",
                "command": ["/bin/astlens", "--serve"],
                "codemode": false,
            })
        );
    }

    #[test]
    fn comments_and_other_servers_survive() {
        let dir = tempfile::tempdir().unwrap();
        let config = file(dir.path());
        fs::write(
            &config,
            "{\n  // keep me\n  \"model\": \"x\",\n  \"mcp\": {\n    \"servers\": {\n      \"other\": {\"type\": \"remote\", \"url\": \"https://example.com\"}\n    }\n  }\n}\n",
        )
        .unwrap();
        add_server(&config, "/bin/astlens").unwrap();
        add_server(&config, "/bin/astlens").unwrap();
        let text = fs::read_to_string(&config).unwrap();
        assert!(text.contains("// keep me"));
        assert!(text.contains("\"other\""));
        assert_eq!(text.matches("\"astlens\"").count(), 1);
        assert!(text.ends_with('\n'));
        assert_eq!(
            fs::read_to_string(config.with_extension("jsonc.bak"))
                .unwrap()
                .matches("\"astlens\"")
                .count(),
            0
        );
    }

    #[test]
    fn remove_only_drops_our_server() {
        let dir = tempfile::tempdir().unwrap();
        let config = file(dir.path());
        fs::write(
            &config,
            "{\n  // keep me\n  \"mcp\": {\"servers\": {\n    \"astlens\": {\"type\": \"local\", \"command\": [\"x\"]},\n    \"other\": {\"type\": \"remote\"}\n  }}\n}\n",
        )
        .unwrap();
        remove_server(&config).unwrap();
        let text = fs::read_to_string(&config).unwrap();
        assert!(!text.contains("\"astlens\""));
        assert!(text.contains("\"other\""));
        assert!(text.contains("// keep me"));
    }

    #[test]
    fn invalid_config_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let config = file(dir.path());
        fs::write(&config, "{ not json").unwrap();
        assert!(add_server(&config, "/bin/astlens").is_err());
        assert_eq!(fs::read_to_string(&config).unwrap(), "{ not json");
    }

    #[test]
    fn the_jsonc_file_is_preferred() {
        let dir = tempfile::tempdir().unwrap();
        let jsonc = dir.path().join("opencode.jsonc");
        fs::write(&jsonc, "{}").unwrap();
        assert_eq!(config_path(dir.path()), jsonc);
    }
}
