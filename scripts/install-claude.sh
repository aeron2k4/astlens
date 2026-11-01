#!/usr/bin/env bash
# Install code-scope-mcp into Claude Code and make Claude use it for source code.
# Runs on Linux, macOS and Windows (in Git Bash, which Claude Code also uses
# for hooks). Needs curl, jq and the claude CLI.
#
#   scripts/install-claude.sh                   download the latest release, register, enforce
#   scripts/install-claude.sh --version v0.1.0  download that release instead
#   scripts/install-claude.sh --from-source     build with cargo instead of downloading
#   scripts/install-claude.sh --no-enforce      install and register only, no hook or CLAUDE.md block
#   scripts/install-claude.sh --uninstall       undo everything this script did
#
# Without a clone: curl -fsSL https://raw.githubusercontent.com/aeron2k4/code-scope-mcp/main/scripts/install-claude.sh | bash -s -- [options]
#
# Enforcement = a PreToolUse hook that denies whole-file Read of source files
# (a Read limited to 300 lines stays allowed, since Edit needs a prior Read) and Grep
# over source files in the workspace, plus a short instruction block in
# ~/.claude/CLAUDE.md.
set -euo pipefail

REPO=aeron2k4/code-scope-mcp
# Set when the script runs from a clone; empty when piped from curl.
REPO_DIR=
if [ -f "${BASH_SOURCE[0]:-}" ]; then
  dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
  [ -f "$dir/Cargo.toml" ] && REPO_DIR=$dir
fi
CLAUDE_DIR=${CLAUDE_CONFIG_DIR:-$HOME/.claude}
BIN_DIR=${CODE_SCOPE_BIN_DIR:-$HOME/.local/bin}
HOOK=$CLAUDE_DIR/hooks/code-scope-guard.sh
SETTINGS=$CLAUDE_DIR/settings.json
MEMORY=$CLAUDE_DIR/CLAUDE.md
BEGIN='<!-- code-scope:begin -->'
END='<!-- code-scope:end -->'

log() { printf '==> %s\n' "$*" >&2; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }
need() { have "$1" || die "'$1' not found in PATH${2:+ ($2)}"; }

mode=install
enforce=1
source_build=0
version=latest
while [ $# -gt 0 ]; do
  case "$1" in
    --uninstall) mode=uninstall ;;
    --no-enforce) enforce=0 ;;
    --from-source) source_build=1 ;;
    --version) [ $# -ge 2 ] || die "--version needs a tag, e.g. v0.1.0"; version=$2; shift ;;
    --version=*) version=${1#--version=} ;;
    -h|--help) sed -n '2,18p' "${BASH_SOURCE[0]:-$0}" 2>/dev/null || true; exit 0 ;;
    *) die "unknown option: $1" ;;
  esac
  shift
done

# Release asset for this OS and CPU, or empty when there is none.
os=$(uname -s)
arch=$(uname -m)
exe=
case "$os" in
  Linux) [ "$arch" = x86_64 ] && asset=code-scope-mcp-linux ;;
  Darwin) [ "$arch" = arm64 ] && asset=code-scope-mcp-macos ;;
  MINGW*|MSYS*|CYGWIN*) exe=.exe; [ "$arch" = x86_64 ] && asset=code-scope-mcp-windows.exe ;;
  *) die "unsupported OS: $os" ;;
esac
asset=${asset:-}

case "$os" in
  Darwin) jq_hint="brew install jq" ;;
  MINGW*|MSYS*|CYGWIN*) jq_hint="winget install jqlang.jq, then reopen Git Bash" ;;
  *) jq_hint="install it with your package manager, e.g. apt install jq" ;;
esac

# Paths handed to Claude Code: Windows-style (C:/Users/...) under Git Bash.
native_path() {
  if have cygpath; then cygpath -m "$1"; else printf '%s\n' "$1"; fi
}

sha256() {
  if have sha256sum; then sha256sum "$1" | cut -d' ' -f1
  else shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

# Drop the code-scope block from CLAUDE.md, if present.
strip_memory() {
  [ -f "$MEMORY" ] || return 0
  local tmp
  tmp=$(mktemp)
  awk -v b="$BEGIN" -v e="$END" '$0==b{skip=1;next} $0==e{skip=0;next} !skip' "$MEMORY" >"$tmp"
  mv "$tmp" "$MEMORY"
}

# Drop our hook entry from settings.json, leaving other hooks untouched.
strip_hook() {
  [ -f "$SETTINGS" ] || return 0
  local tmp
  tmp=$(mktemp)
  jq --arg cmd "$(native_path "$HOOK")" '
    if .hooks.PreToolUse then
      .hooks.PreToolUse |= map(.hooks |= map(select(.command != $cmd)) | select(.hooks | length > 0))
      | if (.hooks.PreToolUse | length) == 0 then del(.hooks.PreToolUse) else . end
      | if (.hooks | length) == 0 then del(.hooks) else . end
    else . end' "$SETTINGS" >"$tmp"
  mv "$tmp" "$SETTINGS"
}

need claude "https://docs.anthropic.com/en/docs/claude-code"
need jq "$jq_hint"

if [ "$mode" = uninstall ]; then
  log "Removing MCP server registration"
  claude mcp remove --scope user code-scope >/dev/null 2>&1 || true
  log "Removing hook and CLAUDE.md block"
  strip_hook
  rm -f "$HOOK"
  strip_memory
  if [ -f "$BIN_DIR/code-scope-mcp$exe" ]; then
    log "Removing $BIN_DIR/code-scope-mcp$exe"
    rm -f "$BIN_DIR/code-scope-mcp$exe"
  fi
  if [ -f "${CARGO_HOME:-$HOME/.cargo}/bin/code-scope-mcp$exe" ]; then
    log "A source build is still installed; remove it with: cargo uninstall code-scope-mcp"
  fi
  log "Done."
  exit 0
fi

tmpdir=$(mktemp -d)
trap 'rm -rf "$tmpdir"' EXIT

if [ "$source_build" = 0 ] && [ -z "$asset" ]; then
  have cargo || die "no prebuilt binary for $os $arch; install Rust (https://rustup.rs) and rerun with --from-source"
  log "No prebuilt binary for $os $arch, building from source"
  source_build=1
fi

# Tag whose guard hook matches the binary; main for a source build from git.
if [ "$source_build" = 1 ]; then
  need cargo "https://rustup.rs"
  BIN=${CARGO_HOME:-$HOME/.cargo}/bin/code-scope-mcp$exe
  if [ -n "$REPO_DIR" ]; then
    log "Building $REPO_DIR and installing $BIN"
    cargo install --path "$REPO_DIR" --locked --force
  else
    ref=$([ "$version" = latest ] && echo main || echo "$version")
    log "Building $REPO@$ref and installing $BIN"
    if [ "$ref" = main ]; then
      cargo install --git "https://github.com/$REPO" --branch main --locked --force
    else
      cargo install --git "https://github.com/$REPO" --tag "$ref" --locked --force
    fi
  fi
  tag=${ref:-}
else
  need curl
  if [ "$version" = latest ]; then
    # releases/latest redirects to releases/tag/<tag>.
    url=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest")
    tag=${url##*/}
    case "$tag" in v*) ;; *) die "could not find the latest release" ;; esac
  else
    tag=$version
  fi
  base="https://github.com/$REPO/releases/download/$tag"
  log "Downloading $asset ($tag)"
  curl -fsSL -o "$tmpdir/$asset" "$base/$asset" || die "download failed: $base/$asset"
  curl -fsSL -o "$tmpdir/SHA256SUMS" "$base/SHA256SUMS" || die "download failed: $base/SHA256SUMS"
  want=$(awk -v f="$asset" '$2==f || $2=="*"f {print $1}' "$tmpdir/SHA256SUMS")
  [ -n "$want" ] || die "$asset is not listed in SHA256SUMS"
  [ "$(sha256 "$tmpdir/$asset")" = "$want" ] || die "checksum mismatch for $asset"
  BIN=$BIN_DIR/code-scope-mcp$exe
  mkdir -p "$BIN_DIR"
  install -m 755 "$tmpdir/$asset" "$BIN"
  log "Installed $BIN"
  case ":$PATH:" in *":$BIN_DIR:"*) ;; *) log "Note: $BIN_DIR is not in PATH (Claude Code does not need it)" ;; esac
fi

log "Registering MCP server 'code-scope' (user scope)"
claude mcp remove --scope user code-scope >/dev/null 2>&1 || true
# alwaysLoad keeps the tools in the model's manifest; without it Claude Code
# defers them behind ToolSearch and the model falls back to Read/Grep.
claude mcp add-json --scope user code-scope \
  "$(jq -n --arg cmd "$(native_path "$BIN")" '{type: "stdio", command: $cmd, alwaysLoad: true}')"

if [ "$enforce" = 1 ]; then
  mkdir -p "$CLAUDE_DIR/hooks"

  log "Writing hook $HOOK"
  if [ -n "$REPO_DIR" ] && [ "$source_build" = 1 ]; then
    install -m 755 "$REPO_DIR/scripts/code-scope-guard.sh" "$HOOK"
  else
    # The hook from the same tag as the binary.
    need curl
    curl -fsSL -o "$tmpdir/guard.sh" \
      "https://raw.githubusercontent.com/$REPO/$tag/scripts/code-scope-guard.sh" \
      || die "could not download the hook for $tag"
    install -m 755 "$tmpdir/guard.sh" "$HOOK"
  fi

  log "Adding hook to $SETTINGS"
  [ -f "$SETTINGS" ] || echo '{}' >"$SETTINGS"
  cp "$SETTINGS" "$SETTINGS.bak"
  strip_hook
  tmp=$(mktemp)
  jq --arg cmd "$(native_path "$HOOK")" '
    .hooks.PreToolUse = ((.hooks.PreToolUse // []) +
      [{matcher: "Read|Grep|Bash", hooks: [{type: "command", command: $cmd}]}])' "$SETTINGS" >"$tmp"
  mv "$tmp" "$SETTINGS"

  log "Adding instructions to $MEMORY"
  touch "$MEMORY"
  strip_memory
  cat >>"$MEMORY" <<EOF
$BEGIN
# Code navigation
When exploring or reading source code, prefer the code-scope MCP tools over Read/Grep:
- \`get_file_outline\` before opening a file; \`search_symbols\` to find a definition.
- \`read_symbol\` to read one function/class instead of the whole file.
- \`replace_symbol\` to rewrite or delete a whole function/class, \`insert_symbol\` to add one; no Read needed.
- \`read_file_pruned\` when a whole file is needed but may be large.
- \`find_references\` to see where a symbol is used; \`search_text\` instead of Grep.
- \`changed_symbols\` to see which symbols a diff touches before reading it.
Use Read/Grep for non-code files or when code-scope fails.
Load the tools with ToolSearch ("select:mcp__code-scope__...") if they are deferred.
$END
EOF
fi

log "Done. Restart Claude Code, then check with: claude mcp list"
