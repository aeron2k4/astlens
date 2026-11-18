//! Native PreToolUse guard hook and Antigravity workspace-root tracking.
//!
//! Invoked by MCP clients as `astlens guard --client <client>` with hook JSON on
//! stdin. Reuses [`Lang`] and path/root helpers so behavior matches both the
//! parser extensions and the reference shell guard [`scripts/astlens-guard.sh`].

use std::collections::VecDeque;
use std::fs;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::ast::parser::Lang;
use crate::atomic::write_atomic;
use crate::cli::Client;
use crate::mcp::tools::{file_uri_to_path, is_broad_root};

const DEFAULT_MAX_LINES: usize = 300;
const ANTIGRAVITY_ROOT_FILE: &str = "antigravity_root";
const MAX_GLOB_CANDIDATES: usize = 2048;

/// Base cache directory used for persistent LMDB parse data and client state:
/// `$ASTLENS_CACHE_DIR`, else `$XDG_CACHE_HOME/astlens`, `$LOCALAPPDATA/astlens`,
/// or `$HOME/.cache/astlens`.
pub fn base_cache_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("ASTLENS_CACHE_DIR").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("LOCALAPPDATA")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        })
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .filter(|v| !v.is_empty())
                .map(|h| PathBuf::from(h).join(".cache"))
        })?;
    Some(base.join("astlens"))
}

/// Path holding Antigravity's most recently active workspace directory.
pub fn antigravity_root_file() -> Option<PathBuf> {
    base_cache_dir().map(|dir| dir.join(ANTIGRAVITY_ROOT_FILE))
}

/// Read hook payload from stdin and emit any hook response JSON on stdout.
pub fn run(client: Client) -> Result<()> {
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .context("failed to read hook input from stdin")?;
    if let Some(output) = evaluate(&input, client) {
        println!("{output}");
    }
    Ok(())
}

/// Evaluate one hook payload JSON string and return the JSON line to print,
/// or `None` when allowed without output (Claude/OpenCode convention).
pub fn evaluate(raw: &str, client: Client) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let payload: Value = serde_json::from_str(trimmed).ok()?;
    let obj = payload.as_object()?;

    if client == Client::Antigravity {
        return evaluate_antigravity(obj);
    }

    let opencode = client == Client::Opencode
        || std::env::var_os("ASTLENS_OPENCODE").is_some_and(|v| v == "1");
    let tool = obj.get("tool_name").and_then(Value::as_str).unwrap_or("");
    let cwd = obj
        .get("cwd")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(process_cwd_string);
    let input = obj.get("tool_input").and_then(Value::as_object);

    let limit = limit_field(input, "limit");
    let req = HookRequest {
        tool,
        cwd: &normalize(&cwd),
        command: str_field(input, "command"),
        workdir: str_field(input, "workdir"),
        file: str_field(input, "file_path"),
        limit: &limit,
        path: str_field(input, "path"),
        glob: str_field(input, "glob"),
        kind: str_field(input, "type"),
    };
    let hints = if opencode {
        Hints::opencode()
    } else {
        Hints::claude()
    };
    check_request(&req, &hints).map(|reason| {
        json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": reason,
            }
        })
        .to_string()
    })
}

fn evaluate_antigravity(payload: &serde_json::Map<String, Value>) -> Option<String> {
    let ws_root = pick_workspace_root(payload);
    if let Some(root) = &ws_root
        && let Some(root_file) = antigravity_root_file()
    {
        let _ = record_root(&root_file, root);
    }

    if payload.get("hookEventName").and_then(Value::as_str) != Some("PreToolUse") {
        return Some("{}".to_string());
    }
    let allow = || Some(json!({"decision": "allow"}).to_string());

    let Some(tool_call) = payload.get("toolCall").and_then(Value::as_object) else {
        return allow();
    };
    let tool_name = tool_call.get("name").and_then(Value::as_str).unwrap_or("");
    let Some(ws_root) = ws_root else {
        return allow();
    };
    if tool_name == "call_mcp_tool" {
        return allow();
    }

    let args = tool_call.get("args").and_then(Value::as_object);
    let cwd = normalize(&ws_root.to_string_lossy());
    let hints = Hints::antigravity();

    let reason = match tool_name {
        "read_file" | "view_file" => {
            let file = str_field(args, "Path").or_else(|| str_field(args, "AbsolutePath"));
            let Some(file) = file.filter(|s| !s.is_empty()) else {
                return allow();
            };
            let limit = antigravity_read_limit(args);
            let req = HookRequest {
                tool: "Read",
                cwd: &cwd,
                command: None,
                workdir: None,
                file: Some(file),
                limit: limit.as_deref().unwrap_or(""),
                path: None,
                glob: None,
                kind: None,
            };
            check_request(&req, &hints)
        }
        "grep_search" => {
            let search_path = str_field(args, "SearchPath")
                .or_else(|| str_field(args, "Directory"))
                .filter(|s| !s.is_empty())
                .unwrap_or(".");
            let glob = antigravity_glob(args);
            let req = HookRequest {
                tool: "Grep",
                cwd: &cwd,
                command: None,
                workdir: None,
                file: None,
                limit: "",
                path: Some(search_path),
                glob: Some(glob.as_str()),
                kind: None,
            };
            check_request(&req, &hints)
        }
        "run_command" => {
            let cmd = str_field(args, "CommandLine").filter(|s| !s.is_empty());
            let Some(cmd) = cmd else {
                return allow();
            };
            let req = HookRequest {
                tool: "Bash",
                cwd: &cwd,
                command: Some(cmd),
                workdir: str_field(args, "Cwd"),
                file: None,
                limit: "",
                path: None,
                glob: None,
                kind: None,
            };
            check_request(&req, &hints)
        }
        _ => None,
    };

    match reason {
        Some(reason) => Some(json!({"decision": "deny", "reason": reason}).to_string()),
        None => allow(),
    }
}

fn antigravity_read_limit(args: Option<&serde_json::Map<String, Value>>) -> Option<String> {
    let args = args?;
    let end = args.get("EndLine").and_then(Value::as_u64)?;
    let start = args
        .get("StartLine")
        .and_then(Value::as_u64)
        .filter(|&s| s >= 1)
        .unwrap_or(1);
    if end >= start {
        Some((end - start + 1).to_string())
    } else {
        Some("999999".to_string())
    }
}

fn antigravity_glob(args: Option<&serde_json::Map<String, Value>>) -> String {
    let Some(val) = args.and_then(|a| a.get("Includes")) else {
        return String::new();
    };
    match val {
        Value::Array(items) => items
            .iter()
            .filter_map(|v| match v {
                Value::String(s) if !s.is_empty() => Some(s.clone()),
                Value::Number(_) | Value::Bool(_) => Some(v.to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::String(s) => s.clone(),
        _ => String::new(),
    }
}

fn to_dir(raw: &str) -> Option<PathBuf> {
    let text = raw.trim();
    if text.is_empty() {
        return None;
    }
    let candidate = if text.len() >= 7 && text[..7].eq_ignore_ascii_case("file://") {
        file_uri_to_path(text)?
    } else if text == "~" {
        home_dir()?
    } else if let Some(rest) = text.strip_prefix("~/").or_else(|| text.strip_prefix("~\\")) {
        home_dir()?.join(rest)
    } else {
        PathBuf::from(text)
    };
    let resolved = candidate.canonicalize().ok()?;
    resolved.is_dir().then_some(resolved)
}

fn pick_workspace_root(payload: &serde_json::Map<String, Value>) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(cwd) = payload.get("cwd").and_then(Value::as_str) {
        candidates.push(cwd);
    }
    if let Some(ws_paths) = payload.get("workspacePaths").and_then(Value::as_array) {
        for item in ws_paths {
            if let Some(s) = item.as_str() {
                candidates.push(s);
            }
        }
    }
    for raw in candidates {
        if let Some(dir) = to_dir(raw)
            && !is_broad_root(&dir)
        {
            return Some(dir);
        }
    }
    None
}

fn record_root(root_file: &Path, ws_root: &Path) -> Result<()> {
    let target = format!("{}\n", ws_root.display());
    if fs::read_to_string(root_file).is_ok_and(|cur| cur == target) {
        return Ok(());
    }
    if let Some(parent) = root_file.parent() {
        fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
    }
    write_atomic(root_file, &target)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(root_file, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn process_cwd_string() -> String {
    std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "/".to_string())
}

fn str_field<'a>(map: Option<&'a serde_json::Map<String, Value>>, key: &str) -> Option<&'a str> {
    map?.get(key)?.as_str()
}

fn limit_field(map: Option<&serde_json::Map<String, Value>>, key: &str) -> String {
    let Some(val) = map.and_then(|m| m.get(key)) else {
        return String::new();
    };
    match val {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

struct Hints {
    read_hint: &'static str,
    grep_hint: &'static str,
    whole_read: &'static str,
    limit_wording: &'static str,
}

impl Hints {
    fn claude() -> Self {
        Self {
            read_hint: "Use astlens MCP tools instead (load with ToolSearch \"select:mcp__astlens__get_file_outline,mcp__astlens__read_symbol,mcp__astlens__read_lines,mcp__astlens__read_file_pruned,mcp__astlens__replace_text\"): get_file_outline / read_symbol / read_lines (a line range) / read_file_pruned. To rewrite a whole function or class use replace_symbol, to add one use insert_symbol, and for any other change use replace_text (exact old text and its replacement; no read needed) or replace_lines.",
            grep_hint: "Use astlens MCP tools instead (load with ToolSearch \"select:mcp__astlens__search_text,mcp__astlens__find_references\"): search_text for a literal or regex (.gitignore-aware, shows the enclosing symbol; takes dir), find_references for uses of a symbol. Searching non-code files stays allowed: pass a non-code path, or a filter such as --include=\"*.md\" / -g \"*.md\" / type md.",
            whole_read: "Read",
            limit_wording: "Read only the needed lines with offset and a limit of",
        }
    }

    fn opencode() -> Self {
        Self {
            read_hint: "Use astlens MCP tools instead: astlens_get_file_outline / astlens_read_symbol / astlens_read_lines (a line range) / astlens_read_file_pruned. To rewrite a whole function or class use astlens_replace_symbol, to add one use astlens_insert_symbol, and for any other change use astlens_replace_text (exact old text and its replacement; no read needed) or astlens_replace_lines.",
            grep_hint: "Use astlens MCP tools instead: astlens_search_text for a literal or regex (.gitignore-aware, shows the enclosing symbol; takes dir), astlens_find_references for uses of a symbol. Searching non-code files stays allowed: pass a non-code path, or a filter such as include \"*.md\".",
            whole_read: "Read",
            limit_wording: "Read only the needed lines with offset and a limit of",
        }
    }

    fn antigravity() -> Self {
        Self {
            read_hint: "Use astlens MCP tools instead (via call_mcp_tool with ServerName=\"astlens\"): get_file_outline / read_symbol / read_lines (a line range) / read_file_pruned. To rewrite a whole function or class use replace_symbol, to add one use insert_symbol, and for any other change use replace_text (exact old text and its replacement; no read needed) or replace_lines.",
            grep_hint: "Use astlens MCP tools instead (via call_mcp_tool with ServerName=\"astlens\"): search_text for a literal or regex (.gitignore-aware, shows the enclosing symbol; takes dir), find_references for uses of a symbol. Searching non-code files stays allowed: pass a non-code path, or a filter such as --include=\"*.md\" / -g \"*.md\" / type md.",
            whole_read: "read_file",
            limit_wording: "pass StartLine and EndLine spanning",
        }
    }
}

struct HookRequest<'a> {
    tool: &'a str,
    cwd: &'a str,
    command: Option<&'a str>,
    workdir: Option<&'a str>,
    file: Option<&'a str>,
    limit: &'a str,
    path: Option<&'a str>,
    glob: Option<&'a str>,
    kind: Option<&'a str>,
}

fn max_lines() -> usize {
    std::env::var("ASTLENS_MAX_READ_LINES")
        .ok()
        .and_then(|v| {
            let trimmed = v.trim();
            if !trimmed.is_empty() && trimmed.bytes().all(|b| b.is_ascii_digit()) {
                trimmed.parse().ok()
            } else {
                None
            }
        })
        .unwrap_or(DEFAULT_MAX_LINES)
}

fn small(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.bytes().all(|b| b.is_ascii_digit())
        && value.parse::<usize>().is_ok_and(|n| n <= max)
}

fn check_request(req: &HookRequest<'_>, hints: &Hints) -> Option<String> {
    let max = max_lines();
    match req.tool {
        "Read" => {
            let file = req.file?;
            if !has_code_ext(file) {
                return None;
            }
            if !inside_workspace(&absolute(file, req.cwd), req.cwd) || small(req.limit, max) {
                return None;
            }
            Some(format!(
                "Whole-file {} of {file} (or a {} without a limit of at most {max} lines) is \
                 blocked. {} For a smaller Edit, {} at most {max} lines.",
                hints.whole_read, hints.whole_read, hints.read_hint, hints.limit_wording
            ))
        }
        "Grep" => {
            let target = req.path.filter(|s| !s.is_empty()).unwrap_or(".");
            let abs = absolute(target, req.cwd);
            if !inside_workspace(&abs, req.cwd) {
                return None;
            }
            let p = Path::new(&abs);
            if p.is_file() {
                if !has_code_ext(&abs) {
                    return None;
                }
            } else if let Some(t) = req.kind.filter(|s| !s.is_empty()) {
                if !is_code_type(t) {
                    return None;
                }
            } else if let Some(g) = req.glob.filter(|s| !s.is_empty())
                && !is_code_glob(g)
                && !has_code_ext(g)
            {
                return None;
            }
            Some(format!(
                "Grep over source code is blocked. {}",
                hints.grep_hint
            ))
        }
        "Bash" => {
            let cmd = req.command?;
            check_bash(cmd, req.workdir, req.cwd, max, hints)
        }
        _ => None,
    }
}

fn is_code_ext(ext: &str) -> bool {
    !ext.is_empty()
        && !ext.contains(['/', '\\'])
        && Lang::from_path(Path::new(&format!("a.{ext}"))).is_some()
}

fn has_code_ext(path: &str) -> bool {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    name.rsplit_once('.')
        .is_some_and(|(_, ext)| is_code_ext(ext))
}

fn is_code_glob(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && matches!(bytes[i], b'.' | b',' | b'{' | b'}') {
            i += 1;
        }
        let start = i;
        while i < bytes.len() && !matches!(bytes[i], b'.' | b',' | b'{' | b'}') {
            i += 1;
        }
        if start < i {
            let prev_ok = start == 0 || matches!(bytes[start - 1], b'.' | b',' | b'{');
            let next_ok = i == bytes.len() || matches!(bytes[i], b',' | b'}');
            if prev_ok && next_ok && is_code_ext(&s[start..i]) {
                return true;
            }
        }
    }
    false
}

fn is_code_type(t: &str) -> bool {
    matches!(
        t,
        "rust"
            | "py"
            | "python"
            | "js"
            | "jsx"
            | "ts"
            | "tsx"
            | "typescript"
            | "go"
            | "c"
            | "cpp"
            | "java"
            | "cs"
            | "csharp"
            | "gdscript"
            | "ruby"
    )
}

fn normalize(raw: &str) -> String {
    let clean = raw.replace('\\', "/");
    let mut prefix = String::new();
    let mut rest = clean.as_str();
    if let Some((drive, tail)) = clean.split_once(":/")
        && drive.len() == 1
        && drive.as_bytes()[0].is_ascii_alphabetic()
    {
        prefix = format!("{drive}:/");
        rest = tail;
    } else if clean.starts_with('/') {
        prefix = "/".to_string();
    }

    let mut parts: Vec<&str> = Vec::new();
    for part in rest.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    if parts.is_empty() {
        if prefix.is_empty() {
            "/".to_string()
        } else {
            prefix
        }
    } else if prefix == "/" {
        format!("/{}", parts.join("/"))
    } else {
        format!("{prefix}{}", parts.join("/"))
    }
}

fn is_abs_like(path: &str) -> bool {
    path.starts_with('/')
        || path.starts_with('\\')
        || (path.len() >= 3
            && path.as_bytes()[0].is_ascii_alphabetic()
            && path.as_bytes()[1] == b':'
            && matches!(path.as_bytes()[2], b'/' | b'\\'))
}

fn absolute(path: &str, base_dir: &str) -> String {
    if is_abs_like(path) {
        normalize(path)
    } else if let Some(rest) = path.strip_prefix('~') {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_default();
        normalize(&format!("{home}{rest}"))
    } else {
        normalize(&format!("{base_dir}/{path}"))
    }
}

fn inside_workspace(abs: &str, cwd: &str) -> bool {
    if cwd == "/" {
        return abs.starts_with('/');
    }
    let abs_slash = format!("{abs}/");
    let cwd_prefix = format!("{cwd}/");
    abs_slash.starts_with(&cwd_prefix)
}

fn path_has_code(raw: &str, dir: &str, cwd: &str) -> bool {
    let abs = absolute(raw, dir);
    if !inside_workspace(&abs, cwd) {
        return false;
    }
    if raw.contains(['*', '?', '[']) {
        let base = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
        return has_code_ext(raw) || !base.contains('.');
    }
    let p = Path::new(&abs);
    if p.is_dir() {
        return true;
    }
    p.is_file() && has_code_ext(&abs)
}

fn is_code_file(raw: &str, dir: &str, cwd: &str) -> bool {
    let abs = absolute(raw, dir);
    if !inside_workspace(&abs, cwd) {
        return false;
    }
    if raw.contains(['*', '?', '[', '{']) {
        if has_code_ext(raw) || is_code_glob(raw) {
            return true;
        }
        return glob_matches_code_file(&abs);
    }
    Path::new(&abs).is_file() && has_code_ext(&abs)
}

fn glob_matches_code_file(abs: &str) -> bool {
    let mut parts = abs.split('/').filter(|s| !s.is_empty());
    let mut roots = vec![if abs.starts_with('/') {
        PathBuf::from("/")
    } else {
        let Some(first) = parts.next() else {
            return false;
        };
        PathBuf::from(format!("{first}/"))
    }];

    for seg in parts {
        let mut next = Vec::new();
        let is_glob = seg.contains(['*', '?', '[']);
        for base in &roots {
            if !is_glob {
                let joined = base.join(seg);
                if joined.exists() {
                    next.push(joined);
                }
            } else if let Ok(entries) = fs::read_dir(base) {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    if name_str.starts_with('.') && !seg.starts_with('.') {
                        continue;
                    }
                    if wildcard_match(seg.as_bytes(), name_str.as_bytes()) {
                        next.push(entry.path());
                    }
                    if next.len() >= MAX_GLOB_CANDIDATES {
                        break;
                    }
                }
            }
            if next.len() >= MAX_GLOB_CANDIDATES {
                break;
            }
        }
        roots = next;
        if roots.is_empty() {
            return false;
        }
    }
    roots
        .into_iter()
        .any(|p| p.is_file() && p.to_str().is_some_and(has_code_ext))
}

fn wildcard_match(pat: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0usize, 0usize);
    let (mut star_p, mut star_t) = (None, 0usize);
    while t < text.len() {
        if p < pat.len() && (pat[p] == b'?' || pat[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pat.len() && pat[p] == b'*' {
            star_p = Some(p);
            p += 1;
            star_t = t;
        } else if let Some(sp) = star_p {
            p = sp + 1;
            star_t += 1;
            t = star_t;
        } else {
            return false;
        }
    }
    while p < pat.len() && pat[p] == b'*' {
        p += 1;
    }
    p == pat.len()
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Word(String),
    Pipe,
    Sep,
}

fn lex(cmd: &str) -> Vec<Token> {
    let s = cmd.as_bytes();
    let n = s.len();
    let mut tokens = Vec::new();
    let mut w: Vec<u8> = Vec::new();
    let mut has = false;
    let mut q: Option<u8> = None;
    let mut heredocs: VecDeque<Vec<u8>> = VecDeque::new();
    let mut i = 0;

    let flush = |w: &mut Vec<u8>, has: &mut bool, tokens: &mut Vec<Token>| {
        if *has {
            tokens.push(Token::Word(String::from_utf8_lossy(w).into_owned()));
            w.clear();
            *has = false;
        }
    };

    while i < n {
        let c = s[i];
        if let Some(quote) = q {
            if c == quote {
                q = None;
            } else if c == b'\\' && quote == b'"' && i + 1 < n {
                i += 1;
                w.push(s[i]);
            } else {
                w.push(c);
            }
            i += 1;
            continue;
        }

        match c {
            b'\'' | b'"' => {
                q = Some(c);
                has = true;
            }
            b'\\' => {
                if i + 1 < n {
                    i += 1;
                    w.push(s[i]);
                    has = true;
                }
            }
            b' ' | b'\t' => {
                flush(&mut w, &mut has, &mut tokens);
            }
            b'|' | b';' | b'&' | b'(' | b')' | b'`' | b'\n' => {
                flush(&mut w, &mut has, &mut tokens);
                let is_single_pipe =
                    c == b'|' && s.get(i + 1) != Some(&b'|') && (i == 0 || s[i - 1] != b'|');
                tokens.push(if is_single_pipe {
                    Token::Pipe
                } else {
                    Token::Sep
                });

                if c == b'\n' && !heredocs.is_empty() {
                    i += 1;
                    while let Some(delim) = heredocs.pop_front() {
                        while i < n {
                            let end = s[i..]
                                .iter()
                                .position(|&b| b == b'\n')
                                .map_or(n, |rel| i + rel);
                            let mut line = &s[i..end];
                            while line.first() == Some(&b'\t') {
                                line = &line[1..];
                            }
                            if line.last() == Some(&b'\r') {
                                line = &line[..line.len() - 1];
                            }
                            i = if end < n { end + 1 } else { n };
                            if line == delim.as_slice() {
                                break;
                            }
                        }
                    }
                    continue;
                }
            }
            b'<' => {
                if s[i..].starts_with(b"<<<") {
                    w.extend_from_slice(b"<<<");
                    has = true;
                    i += 3;
                    continue;
                } else if s[i..].starts_with(b"<<") {
                    flush(&mut w, &mut has, &mut tokens);
                    i += 2;
                    if s.get(i) == Some(&b'-') {
                        i += 1;
                    }
                    while matches!(s.get(i), Some(b' ' | b'\t')) {
                        i += 1;
                    }
                    let mut delim = Vec::new();
                    while i < n {
                        match s[i] {
                            b' ' | b'\t' | b'\n' | b';' | b'|' | b'&' | b')' => break,
                            b'\'' | b'"' | b'\\' => {}
                            b => delim.push(b),
                        }
                        i += 1;
                    }
                    heredocs.push_back(delim);
                    continue;
                } else {
                    w.push(c);
                    has = true;
                }
            }
            b'$' if s.get(i + 1) == Some(&b'(') => {
                flush(&mut w, &mut has, &mut tokens);
                tokens.push(Token::Sep);
                i += 2;
                continue;
            }
            b => {
                w.push(b);
                has = true;
            }
        }
        i += 1;
    }
    flush(&mut w, &mut has, &mut tokens);
    tokens
}

fn command_argv<'a>(seg: &'a [&'a str]) -> (&'a [&'a str], bool) {
    let mut j = 0;
    let mut under_rtk = false;
    while j < seg.len() {
        let w = seg[j];
        if w == "rtk" {
            under_rtk = true;
            j += 1;
        } else if w.contains('=')
            || matches!(
                w,
                "sudo"
                    | "command"
                    | "exec"
                    | "env"
                    | "time"
                    | "nice"
                    | "nohup"
                    | "bypass"
                    | "if"
                    | "then"
                    | "else"
                    | "elif"
                    | "do"
                    | "while"
                    | "until"
                    | "{"
                    | "!"
            )
        {
            j += 1;
        } else {
            break;
        }
    }
    (&seg[j..], under_rtk)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SearchKind {
    Grep,
    Rg,
    Git,
}

fn check_search(dir: &str, cwd: &str, piped: bool, argv: &[&str]) -> bool {
    let prog = argv[0].rsplit(['/', '\\']).next().unwrap_or(argv[0]);
    let mut rest = &argv[1..];
    let kind = match prog {
        "rg" => SearchKind::Rg,
        "git" => {
            rest = rest.get(1..).unwrap_or(&[]);
            SearchKind::Git
        }
        _ => SearchKind::Grep,
    };

    let short_vals = match kind {
        SearchKind::Rg => "efgtTmABCMjrEd",
        SearchKind::Git => "efmABCO",
        SearchKind::Grep => "efmABCdDgtO",
    };

    let mut filters: Vec<&str> = Vec::new();
    let mut types: Vec<&str> = Vec::new();
    let mut operands: Vec<&str> = Vec::new();
    let mut pattern_given = false;
    let mut recursive = false;
    let mut opts = true;

    let mut i = 0;
    while i < rest.len() {
        let a = rest[i];
        i += 1;
        if opts && a == "--" {
            opts = false;
            continue;
        }
        if opts && a.starts_with("--") {
            let (name, mut val, has_eq) = match a.split_once('=') {
                Some((n, v)) => (n, v, true),
                None => (a, "", false),
            };
            let takes_val = matches!(
                name,
                "--include"
                    | "--glob"
                    | "--iglob"
                    | "--file-extensions"
                    | "--type"
                    | "--type-not"
                    | "--regexp"
                    | "--file"
                    | "--exclude"
                    | "--exclude-dir"
                    | "--include-dir"
                    | "--max-count"
                    | "--context"
                    | "--after-context"
                    | "--before-context"
                    | "--max-depth"
                    | "--replace"
                    | "--type-add"
                    | "--sort"
                    | "--sortr"
                    | "--color"
                    | "--colors"
                    | "--encoding"
                    | "--threads"
                    | "--max-columns"
                    | "--directories"
                    | "--devices"
                    | "--label"
                    | "--binary-files"
            );
            if takes_val && !has_eq {
                val = rest.get(i).copied().unwrap_or("");
                if i < rest.len() {
                    i += 1;
                }
            }
            match name {
                "--include" | "--glob" | "--iglob" | "--file-extensions" => filters.push(val),
                "--type" => types.push(val),
                "--regexp" | "--file" => pattern_given = true,
                "--recursive" | "--dereference-recursive" => recursive = true,
                "--directories" if val == "recurse" => recursive = true,
                _ => {}
            }
            continue;
        }
        if opts && a.starts_with('-') && a.len() > 1 {
            let bytes = a.as_bytes();
            let mut k = 1;
            while k < bytes.len() {
                let ch = bytes[k] as char;
                if short_vals.contains(ch) {
                    let v = if k + 1 < a.len() {
                        &a[k + 1..]
                    } else {
                        let next = rest.get(i).copied().unwrap_or("");
                        if i < rest.len() {
                            i += 1;
                        }
                        next
                    };
                    match ch {
                        'e' | 'f' => pattern_given = true,
                        'g' => filters.push(v),
                        'O' if kind != SearchKind::Git => filters.push(v),
                        't' => types.push(v),
                        'd' if kind == SearchKind::Grep && v == "recurse" => recursive = true,
                        _ => {}
                    }
                    break;
                }
                if matches!(ch, 'r' | 'R') && kind == SearchKind::Grep {
                    recursive = true;
                }
                k += 1;
            }
            continue;
        }
        operands.push(a);
    }

    let mut paths: Vec<&str> = if pattern_given {
        operands
    } else if operands.len() > 1 {
        operands[1..].to_vec()
    } else {
        Vec::new()
    };
    if paths.is_empty() {
        match kind {
            SearchKind::Git => paths.push("."),
            SearchKind::Rg => {
                if piped {
                    return false;
                }
                paths.push(".");
            }
            SearchKind::Grep => {
                if piped || !recursive {
                    return false;
                }
                paths.push(".");
            }
        }
    }

    if !paths
        .iter()
        .any(|f| inside_workspace(&absolute(f, dir), cwd))
    {
        return false;
    }

    let mut has_filter = false;
    for f in filters {
        if f.starts_with('!') {
            continue;
        }
        has_filter = true;
        if is_code_glob(f) || has_code_ext(f) {
            return true;
        }
    }
    for f in types {
        has_filter = true;
        if is_code_type(f) || is_code_glob(f) {
            return true;
        }
    }
    if has_filter {
        return false;
    }

    paths.iter().any(|f| path_has_code(f, dir, cwd))
}

struct Redirects<'a> {
    args: Vec<&'a str>,
    input: &'a str,
    stdout_gone: bool,
}

fn parse_redirects<'a>(words: &[&'a str]) -> Redirects<'a> {
    let mut args = Vec::new();
    let mut input = "";
    let mut stdout_gone = false;
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        match w {
            ">" | ">>" | "1>" | "1>>" => {
                stdout_gone = true;
                i += 2;
            }
            "2>" | "2>>" | "<<<" => {
                i += 2;
            }
            "<" => {
                input = words.get(i + 1).copied().unwrap_or("");
                i += 2;
            }
            _ if w.starts_with('>') || w.starts_with("1>") => {
                stdout_gone = true;
                i += 1;
            }
            _ if w.starts_with("2>") || w.starts_with("<<<") => {
                i += 1;
            }
            _ if w.starts_with('<') => {
                input = &w[1..];
                i += 1;
            }
            _ => {
                args.push(w);
                i += 1;
            }
        }
    }
    Redirects {
        args,
        input,
        stdout_gone,
    }
}

struct Sed<'a> {
    files: Vec<&'a str>,
    scripts: Vec<&'a str>,
    quiet: bool,
    in_place: bool,
}

fn parse_sed<'a>(args: &[&'a str]) -> Sed<'a> {
    let mut words = Vec::new();
    let mut scripts = Vec::new();
    let mut quiet = false;
    let mut in_place = false;
    let mut opts = true;
    let mut i = 0;
    while i < args.len() {
        let a = args[i];
        i += 1;
        if opts && a == "--" {
            opts = false;
        } else if opts && a.starts_with("--") {
            if a == "--quiet" || a == "--silent" {
                quiet = true;
            } else if a.starts_with("--in-place") {
                in_place = true;
            } else if let Some(val) = a.strip_prefix("--expression=") {
                scripts.push(val);
            } else if a == "--expression" {
                scripts.push(args.get(i).copied().unwrap_or(""));
                if i < args.len() {
                    i += 1;
                }
            } else if a.starts_with("--file=") {
                scripts.push("");
            } else if a == "--file" {
                scripts.push("");
                if i < args.len() {
                    i += 1;
                }
            } else if a == "--line-length" && i < args.len() {
                i += 1;
            }
        } else if opts && a.starts_with('-') && a.len() > 1 {
            let bytes = a.as_bytes();
            let mut k = 1;
            while k < bytes.len() {
                match bytes[k] as char {
                    'n' => quiet = true,
                    'i' => {
                        in_place = true;
                        break;
                    }
                    'e' => {
                        if k + 1 < a.len() {
                            scripts.push(&a[k + 1..]);
                        } else {
                            scripts.push(args.get(i).copied().unwrap_or(""));
                            if i < args.len() {
                                i += 1;
                            }
                        }
                        break;
                    }
                    'f' => {
                        scripts.push("");
                        if k + 1 >= a.len() && i < args.len() {
                            i += 1;
                        }
                        break;
                    }
                    'l' => {
                        if k + 1 >= a.len() && i < args.len() {
                            i += 1;
                        }
                        break;
                    }
                    _ => {}
                }
                k += 1;
            }
        } else {
            words.push(a);
        }
    }

    let files = if scripts.is_empty() && !words.is_empty() {
        scripts.push(words[0]);
        words[1..].to_vec()
    } else {
        words
    };
    Sed {
        files,
        scripts,
        quiet,
        in_place,
    }
}

fn parse_sed_limited(script: &str, quiet: bool, max: usize) -> bool {
    if let Some(n) = script.strip_suffix('q') {
        return small(n, max);
    }
    if !quiet {
        return false;
    }
    let Some(body) = script.strip_suffix('p') else {
        return false;
    };
    match body.split_once(',') {
        Some((a, b)) => {
            !a.is_empty()
                && a.bytes().all(|c| c.is_ascii_digit())
                && !b.is_empty()
                && b.bytes().all(|c| c.is_ascii_digit())
                && match (a.parse::<usize>(), b.parse::<usize>()) {
                    (Ok(first), Ok(last)) => last >= first && (last - first + 1) <= max,
                    _ => false,
                }
        }
        None => !body.is_empty() && body.bytes().all(|c| c.is_ascii_digit()) && max >= 1,
    }
}

fn limited_output(prog: &str, args: &[&str], max: usize) -> bool {
    match prog {
        "head" | "tail" => {
            let mut i = 0;
            while i < args.len() {
                let a = args[i];
                i += 1;
                if a == "-n" || a == "--lines" {
                    let v = args.get(i).copied().unwrap_or("");
                    if !small(v, max) {
                        return false;
                    }
                    if i < args.len() {
                        i += 1;
                    }
                } else if let Some(v) = a.strip_prefix("--lines=").or_else(|| a.strip_prefix("-n"))
                {
                    if !small(v, max) {
                        return false;
                    }
                } else if a.starts_with("-c")
                    || a.starts_with("--bytes")
                    || (a.starts_with('-')
                        && a[1..].starts_with(|c: char| c.is_ascii_digit())
                        && !small(&a[1..], max))
                {
                    return false;
                }
            }
            true
        }
        "sed" => {
            let sed = parse_sed(args);
            sed.scripts.len() == 1 && parse_sed_limited(sed.scripts[0], sed.quiet, max)
        }
        "read" => {
            let mut i = 0;
            while i < args.len() {
                let a = args[i];
                i += 1;
                if matches!(a, "-m" | "--max-lines" | "--head-lines" | "--tail-lines") {
                    let v = args.get(i).copied().unwrap_or("");
                    if small(v, max) {
                        return true;
                    }
                    if i < args.len() {
                        i += 1;
                    }
                } else if let Some(v) = a
                    .strip_prefix("--max-lines=")
                    .or_else(|| a.strip_prefix("--head-lines="))
                    .or_else(|| a.strip_prefix("--tail-lines="))
                    && small(v, max)
                {
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}

fn check_read(
    dir: &str,
    cwd: &str,
    prog: &str,
    rest: &[&str],
    next_piped_seg: Option<&[&str]>,
    max: usize,
) -> bool {
    let redir = parse_redirects(rest);
    if redir.stdout_gone {
        return false;
    }

    let mut inputs: Vec<&str> = match prog {
        "sed" => {
            let sed = parse_sed(&redir.args);
            if sed.in_place {
                return false;
            }
            sed.files
        }
        "awk" | "gawk" | "mawk" | "nawk" => {
            let mut files = Vec::new();
            let mut need_script = true;
            let mut i = 0;
            while i < redir.args.len() {
                let a = redir.args[i];
                i += 1;
                match a {
                    "-f" | "--file" => {
                        need_script = false;
                        if i < redir.args.len() {
                            i += 1;
                        }
                    }
                    "-i" | "--include" => {
                        if redir.args.get(i).copied() == Some("inplace") {
                            return false;
                        }
                        if i < redir.args.len() {
                            i += 1;
                        }
                    }
                    "-v" | "-F" | "--assign" | "--field-separator" => {
                        if i < redir.args.len() {
                            i += 1;
                        }
                    }
                    _ if a.starts_with('-') => {}
                    _ => {
                        if need_script {
                            need_script = false;
                        } else {
                            files.push(a);
                        }
                    }
                }
            }
            files
        }
        _ => redir
            .args
            .iter()
            .copied()
            .filter(|a| !a.starts_with('-'))
            .collect(),
    };
    if !redir.input.is_empty() {
        inputs.push(redir.input);
    }
    if !inputs.iter().any(|a| is_code_file(a, dir, cwd)) {
        return false;
    }
    if limited_output(prog, &redir.args, max) {
        return false;
    }

    let Some(next) = next_piped_seg else {
        return true;
    };
    let (next_argv, _) = command_argv(next);
    let Some(&first) = next_argv.first() else {
        return true;
    };
    let next_prog = first.rsplit(['/', '\\']).next().unwrap_or(first);
    if matches!(
        next_prog,
        "wc" | "sha1sum"
            | "sha224sum"
            | "sha256sum"
            | "sha384sum"
            | "sha512sum"
            | "md5sum"
            | "b2sum"
            | "shasum"
            | "cksum"
    ) {
        return false;
    }
    if matches!(next_prog, "head" | "tail" | "sed") {
        let next_redir = parse_redirects(&next_argv[1..]);
        if limited_output(next_prog, &next_redir.args, max) {
            return false;
        }
    }
    true
}

fn check_bash(
    cmd: &str,
    workdir: Option<&str>,
    cwd: &str,
    max: usize,
    hints: &Hints,
) -> Option<String> {
    let tokens = lex(cmd);
    let mut segments: Vec<(Vec<&str>, bool, bool)> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let mut piped_in = false;
    for tok in &tokens {
        match tok {
            Token::Word(w) => current.push(w.as_str()),
            Token::Pipe => {
                segments.push((std::mem::take(&mut current), piped_in, true));
                piped_in = true;
            }
            Token::Sep => {
                segments.push((std::mem::take(&mut current), piped_in, false));
                piped_in = false;
            }
        }
    }
    segments.push((current, piped_in, false));

    let mut dir = workdir
        .filter(|s| !s.is_empty())
        .map(|w| absolute(w, cwd))
        .unwrap_or_else(|| cwd.to_string());

    for idx in 0..segments.len() {
        let (ref seg, piped, piped_out) = segments[idx];
        if seg.is_empty() {
            continue;
        }
        let (argv, under_rtk) = command_argv(seg);
        let Some(&first) = argv.first() else {
            continue;
        };
        let prog = first.rsplit(['/', '\\']).next().unwrap_or(first);
        let next_seg = if piped_out {
            segments.get(idx + 1).map(|(words, _, _)| words.as_slice())
        } else {
            None
        };

        match prog {
            "cd" => {
                let target = argv.get(1).copied().filter(|s| !s.is_empty());
                let home = std::env::var("HOME")
                    .or_else(|_| std::env::var("USERPROFILE"))
                    .unwrap_or_default();
                dir = absolute(target.unwrap_or(&home), &dir);
            }
            "grep" | "egrep" | "fgrep" | "ugrep" | "rg" => {
                if check_search(&dir, cwd, piped, argv) {
                    return Some(format!(
                        "Searching source code with {prog} is blocked. {}",
                        hints.grep_hint
                    ));
                }
            }
            "git" if argv.get(1).copied() == Some("grep") => {
                if check_search(&dir, cwd, false, argv) {
                    return Some(format!(
                        "Searching source code with git grep is blocked. {}",
                        hints.grep_hint
                    ));
                }
            }
            "cat" | "tac" | "nl" | "more" | "less" | "most" | "bat" | "batcat" | "head"
            | "tail" | "sed" | "awk" | "gawk" | "mawk" | "nawk" => {
                if check_read(&dir, cwd, prog, &argv[1..], next_seg, max) {
                    return Some(format!(
                        "Reading source code with {prog} is blocked. {} To see a few lines, limit \
                         the output to at most {max} lines, e.g. sed -n 10,60p.",
                        hints.read_hint
                    ));
                }
            }
            "read" | "smart"
                if under_rtk && check_read(&dir, cwd, prog, &argv[1..], next_seg, max) =>
            {
                return Some(format!(
                    "Reading source code with rtk {prog} is blocked. {} To see a few lines, \
                     use rtk read --head-lines with at most {max}.",
                    hints.read_hint
                ));
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_and_opencode_denials_match_expected_format() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().canonicalize().unwrap();
        fs::create_dir_all(ws.join("src")).unwrap();
        let file = ws.join("src/main.rs");
        fs::write(&file, "fn main() {}\n").unwrap();

        let claude = evaluate(
            &json!({
                "tool_name": "Read",
                "cwd": ws,
                "tool_input": {"file_path": file}
            })
            .to_string(),
            Client::Claude,
        )
        .unwrap();
        assert!(
            claude.contains("\"permissionDecision\":\"deny\""),
            "{claude}"
        );
        assert!(claude.contains("ToolSearch"), "{claude}");

        let opencode = evaluate(
            &json!({
                "tool_name": "Read",
                "cwd": ws,
                "tool_input": {"file_path": file}
            })
            .to_string(),
            Client::Opencode,
        )
        .unwrap();
        assert!(opencode.contains("astlens_read_symbol"), "{opencode}");

        assert!(
            evaluate(
                &json!({
                    "tool_name": "Read",
                    "cwd": ws,
                    "tool_input": {"file_path": file, "limit": 50}
                })
                .to_string(),
                Client::Claude,
            )
            .is_none()
        );
    }

    #[test]
    fn antigravity_tracks_root_via_file_uri_and_enforces_pre_tool_use() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("project space");
        fs::create_dir_all(ws.join("src")).unwrap();
        let file = ws.join("src/main.rs");
        fs::write(&file, "fn main() {}\n").unwrap();

        let cache = dir.path().join("cache");
        let root_file = cache.join("antigravity_root");
        let canonical_ws = ws.canonicalize().unwrap();
        let uri = format!(
            "file://{}",
            canonical_ws.to_string_lossy().replace(' ', "%20")
        );

        // SessionStart only records the root and replies `{}`.
        let payload = json!({
            "hookEventName": "SessionStart",
            "cwd": "/",
            "workspacePaths": [uri],
        });
        let picked = pick_workspace_root(payload.as_object().unwrap()).unwrap();
        assert_eq!(picked, canonical_ws);
        record_root(&root_file, &picked).unwrap();
        assert_eq!(
            fs::read_to_string(&root_file).unwrap(),
            format!("{}\n", canonical_ws.display())
        );

        let deny = evaluate(
            &json!({
                "hookEventName": "PreToolUse",
                "cwd": canonical_ws,
                "toolCall": {"name": "read_file", "args": {"Path": file}}
            })
            .to_string(),
            Client::Antigravity,
        )
        .unwrap();
        let deny_v: Value = serde_json::from_str(&deny).unwrap();
        assert_eq!(deny_v["decision"], "deny");
        assert!(
            deny_v["reason"].as_str().unwrap().contains("call_mcp_tool"),
            "{deny}"
        );

        let allow = evaluate(
            &json!({
                "hookEventName": "PreToolUse",
                "cwd": canonical_ws,
                "toolCall": {
                    "name": "read_file",
                    "args": {"Path": file, "StartLine": 10, "EndLine": 40}
                }
            })
            .to_string(),
            Client::Antigravity,
        )
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&allow).unwrap()["decision"],
            "allow"
        );
    }

    #[test]
    fn lex_skips_heredoc_bodies_and_utf8() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().canonicalize().unwrap();
        fs::create_dir_all(ws.join("src")).unwrap();
        fs::write(ws.join("src/main.rs"), "fn main() {}\n").unwrap();
        let cwd = normalize(&ws.to_string_lossy());
        let hints = Hints::claude();

        assert!(
            check_bash(
                "cat > notes.md <<'END'\ndòng tiếng Việt với *\ncat src/main.rs\nEND",
                None,
                &cwd,
                300,
                &hints,
            )
            .is_none()
        );
        assert!(
            check_bash(
                "cat > notes.md <<END\ndòng tiếng Việt với *\nEND\ncat src/main.rs",
                None,
                &cwd,
                300,
                &hints,
            )
            .is_some()
        );
        assert!(
            check_bash(
                "git commit -m \"$(cat <<'END'\ncat src/main.rs\nEND\n)\"",
                None,
                &cwd,
                300,
                &hints,
            )
            .is_none()
        );
    }
}
