# code-scope-mcp

A small, fast MCP server in Rust for AST-aware code navigation. It uses
tree-sitter to find symbols and trims file contents to fit a token budget, so
AI coding agents can explore large codebases without reading whole files.

Supported languages: Rust, Python, JavaScript, TypeScript/TSX, Go, C, C++, Java,
C#. `.h` headers are parsed as C.

## Tools

| Tool               | Purpose                                                                          |
| ------------------ | -------------------------------------------------------------------------------- |
| `get_file_outline` | Symbols in a file with signatures and line ranges                                |
| `search_symbols`   | Ranked name search across the workspace (exact > prefix > substring)             |
| `read_symbol`      | Exact source of one definition, line-numbered                                    |
| `replace_symbol`   | Rewrite or delete one definition without reading the file; rejects syntax errors |
| `insert_symbol`    | Add code after/before a symbol or at the end of a file; rejects syntax errors   |
| `read_file_pruned` | File text within `max_tokens`: full → bodies elided → outline → truncated        |
| `find_references`  | Uses of a symbol from identifier tokens (comments and strings skipped)           |
| `search_text`      | Literal or regex search over files, honouring `.gitignore`                       |
| `list_files`       | Files under a directory, honouring `.gitignore`                                  |
| `changed_symbols`  | Symbols added/modified/removed by uncommitted changes or since a revision        |
| `git_status`       | Working-tree status                                                              |

Hits are one per line. `find_references` prints `path:line:col: text`, with
`[definition]` on definitions when asked for; `search_text` prints
`path:line: text`. Either adds `[in Symbol]` when the hit lies inside a symbol
(top-level code has none):

```text
src/lib.rs:42:9: load();  [in Config::reload]
src/lib.rs:7:12: pub fn load() -> Self {  [definition]  [in Config]
README.md:3: Call load() once at startup.
```

Files are parsed lazily the first time they are requested, so nothing is
indexed at startup. A parse records a file's symbols and the offsets of its
identifier tokens, and is cached at two levels:

- **in memory**, reused while a file's modification time and size are unchanged,
  within a memory budget (`CODE_SCOPE_CACHE_MB`);
- **on disk**, in an LMDB (memory-mapped) store keyed by the file's git blob
  hash, so unchanged files are not re-parsed after a restart. Entries of files
  deleted from the workspace are dropped in the background at startup.

| Variable               | Effect                                                                  |
| ---------------------- | ----------------------------------------------------------------------- |
| `CODE_SCOPE_ROOT`      | Workspace root when no argument is given                                |
| `CODE_SCOPE_CACHE_DIR` | Store location (default `$XDG_CACHE_HOME/code-scope-mcp` or `~/.cache/code-scope-mcp`) |
| `CODE_SCOPE_NO_CACHE`  | Set to `1` to keep the cache in memory only                             |
| `CODE_SCOPE_CACHE_MB`  | Memory kept for parsed files, least recently used evicted first (default 512) |
| `CODE_SCOPE_MAX_FILES` | Files walked per workspace-wide search (default 20000); output notes when the limit is hit |
| `CODE_SCOPE_LOG`       | Log filter, e.g. `debug` (stderr only)                                  |

## Performance

On the Go 1.27.3 standard library (8,236 source files, 82 MB), with a warm cache:

- `search_symbols`: 23 ms; after a restart, 86 ms from the on-disk store
  versus 2.8 s cold.
- `find_references`: 24 ms. It skips 49% of the lines a whole-word
  `grep -w` matches: comments, strings, docs, and files in languages it does
  not parse (assembly, ...), which can hold real uses.
- `search_text` over every text file: 35 ms median for a whole-word query,
  31 ms for the regex `TODO|FIXME|XXX`.
- Outlines use ~92% fewer tokens than full source; reading one function costs
  a median 0.9% of its file.
- The server answers `initialize` in ~2 ms. After searches over the whole
  corpus it holds ~200 MB resident with the on-disk store (~315 MB without).

Full results for Go, CPython and tokio are in [`BENCHMARKS.md`](BENCHMARKS.md).

## Usage

Prebuilt binaries for Linux (x86_64) and Windows (x86_64) are attached to each
[release](https://github.com/aeron2k4/code-scope-mcp/releases), with their
SHA-256 checksums in `SHA256SUMS`. For example, on Linux:

```sh
curl -fLo ~/.local/bin/code-scope-mcp \
  https://github.com/aeron2k4/code-scope-mcp/releases/latest/download/code-scope-mcp-linux
chmod +x ~/.local/bin/code-scope-mcp
```

The Windows binary is `code-scope-mcp-windows.exe`. Other platforms (macOS, Linux
ARM) can build from source.

Or build from source:

```sh
cargo build --release
./target/release/code-scope-mcp serve /path/to/project   # or $CODE_SCOPE_ROOT, or the cwd
```

### Claude Code

Installing takes two steps. First the binary, on Linux and Windows (run it in
Git Bash, which Claude Code also uses for hooks). This needs `curl`, not `sudo`:

```sh
curl -fsSL https://raw.githubusercontent.com/aeron2k4/code-scope-mcp/main/scripts/install.sh | bash
```

or, from a clone, `scripts/install.sh`. It checks the latest release binary
against `SHA256SUMS` and saves it as `~/.local/bin/code-scope-mcp`
(`CODE_SCOPE_BIN_DIR` to change). A marked block in the rc file of your shell
(bash, zsh or fish) adds the alias `csm`, and that directory to `PATH` if
missing; both apply to new shells. Options (after `| bash -s --` when piped):

```sh
scripts/install.sh --version v0.1.0   # a specific release
scripts/install.sh --from-source      # build with cargo instead (needs Rust)
```

Where no prebuilt binary exists (macOS, Linux ARM) the script builds from source
if `cargo` is installed. CI does not run on macOS.

Then set up Claude Code. This needs `claude`, and `jq` for the hook (on Windows,
`winget install jqlang.jq`):

```sh
csm install                # or code-scope-mcp install, in a shell opened before the alias
```

`csm` alone shows the help. `csm install` does three things, all at user scope
so they apply to every project:

1. Register the binary as `code-scope`, run as `code-scope-mcp serve`, with
   `"alwaysLoad": true`, so its tools are in the model's tool list from the start instead of deferred behind ToolSearch.
   With no root argument the server uses the session's working directory.
2. Add a `PreToolUse` hook (`~/.claude/hooks/code-scope-guard.sh`, wired in
   `~/.claude/settings.json`) that points Claude to the code-scope tools:
   - denies whole-file `Read` of source files in the session's workspace; a
     `Read` with a `limit` of at most 300 lines (`CODE_SCOPE_MAX_READ_LINES`)
     is still allowed, because `Edit` needs a prior `Read`;
   - denies searches that may cover those files inside the session's
     workspace, both the `Grep` tool and `grep`/`egrep`/`fgrep`/`ugrep`/`rg`/`git
     grep` in Bash (use `search_text` / `find_references`). Searching a non-code
     path, with a non-code filter such as `--include='*.md'` / `-g '*.md'`,
     piped input (`cargo test | grep FAILED`) or outside the workspace is
     allowed. Bash parsing is best effort: `eval`, scripts, `find -exec` and
     `xargs` are not inspected.
   - denies reading those files through Bash: `cat`, `tac`, `nl`, `more`, `less`,
     `most`, `bat`, `head`, `tail`, `sed`, `awk`, `rtk read` and `rtk smart`.
     Output limited to at most 300 lines (`head -n 50`, `sed -n 1,50p`,
     `rtk read --head-lines 50`), sent to a file, counted or hashed
     (`cat x.rs | wc -l`), and `sed -i` are allowed. Other programs that can
     print a file (`python`, `xxd`, `cut`) are not inspected.

   The hook is [`scripts/code-scope-guard.sh`](scripts/code-scope-guard.sh).
3. Add a short instruction block to `~/.claude/CLAUDE.md`.

Restart Claude Code afterwards. Both commands are safe to re-run. To upgrade, run
the script for the new binary, then `csm install` for the hook that came with it.

```sh
csm install --no-enforce   # step 1 only, no hook or CLAUDE.md block
csm uninstall              # undo steps 1–3, remove the shell block and the binary
```

`csm uninstall` keeps the parse cache (`~/.cache/code-scope-mcp`). A binary from
`cargo install` or a package manager is left for that tool to remove, and on
Windows a running binary cannot be deleted: it says so, and you delete it after
closing Claude Code.

To register manually instead:

```sh
claude mcp add-json code-scope \
  '{"type": "stdio", "command": "/abs/path/to/code-scope-mcp", "args": ["serve", "/path/to/project"], "alwaysLoad": true}'
```

## Releasing

Bump `version` in `Cargo.toml`, commit, then push a matching tag:

```sh
git tag -a v0.2.0 -m "code-scope-mcp 0.2.0"
git push origin v0.2.0
```

The release workflow checks that the tag matches `Cargo.toml`, runs the tests on
Linux and publishes a GitHub Release with the Linux binary, `SHA256SUMS` and
generated notes, without waiting for the Windows runner. The Windows binary is
then built and added to that release, so it appears later. Until then,
downloading it fails, including through `scripts/install.sh`.

## Development

```sh
cargo test
cargo fmt
cargo clippy --all-targets -- -D warnings
scripts/test-guard.sh   # table test for the hook; needs jq
```

`cargo bench --bench report` measures this crate's own `src/`. To regenerate
[`BENCHMARKS.md`](BENCHMARKS.md), set `BENCH_CORPORA` to comma-separated
`name=directory` pairs; the bench stops if a directory does not exist. Paths
vary by system, for example:

```sh
BENCH_CORPORA="Go stdlib=$(go env GOROOT)/src,\
CPython stdlib=$(python3 -c 'import sysconfig; print(sysconfig.get_path("stdlib"))'),\
tokio (Rust)=/path/to/tokio" \
BENCH_OUT=BENCHMARKS.md cargo bench --bench report
```

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
