# astlens

A small, fast MCP server in Rust for AST-aware code navigation. It uses
tree-sitter to find symbols and trims file contents to fit a token budget, so
AI coding agents can explore large codebases without reading whole files.

Landing page: <https://aeron2k4.github.io/astlens/>

Supported languages: Rust, Python, JavaScript, TypeScript/TSX, Go, C, C++, Java,
C#, GDScript, Ruby. `.h` headers are parsed as C, or as C++ when C fails.

## Performance

On the Go 1.27.3 standard library (8,236 source files, 82 MB), with a warm cache:

- `search_symbols`: 15 ms; after a restart, 96 ms from the on-disk store
  versus 2.4 s cold.
- `find_references`: 13 ms. It skips 49% of the lines a whole-word
  `grep -w` matches: comments, strings, docs, and files in languages it does
  not parse (assembly, ...), which can hold real uses.
- `search_text` over every text file: 37 ms median for a whole-word query,
  32 ms for the regex `TODO|FIXME|XXX`.
- Outlines use 81% fewer tokens than full source; reading one function costs
  a median 0.9% of its file.
- `codebase_map` of the whole library: 240 ms, 120 symbols in 58 files within
  its default 2,000 tokens, 2,051 times fewer than every file's outline.
- The server answers `initialize` in ~3 ms. After searches over the whole
  corpus it holds ~240 MB resident with the on-disk store (~410 MB without).

On the Linux kernel (66,075 files, 1 GB), a warm `search_symbols` over the
whole tree takes 179 ms, `find_references` 281 ms and `codebase_map` 3.5 s.
The server walks 20,000 files by default; raise `ASTLENS_MAX_FILES` for a tree
that large.

Full results for Go, CPython, tokio, the Linux kernel and VS Code are in
[`BENCHMARKS.md`](BENCHMARKS.md).

## Installation

There are four ways to get the binary. Whichever you use, the last step is
`astlens install`, which sets the server up in Claude Code, OpenCode and/or
Antigravity.

| Way                          | Binary lands in | Upgrade with                          |
| ---------------------------- | --------------- | ------------------------------------- |
| `scripts/install.sh`         | `~/.local/bin`  | `astlens update`                      |
| `cargo install astlens`      | `~/.cargo/bin`  | `cargo install astlens`               |
| `cargo install --git ...`    | `~/.cargo/bin`  | `cargo install --force --git ...`     |
| `cargo install --path .`     | `~/.cargo/bin`  | `cargo install --force --path .`      |

The first needs only `curl`; the three cargo ways need Rust 1.90 or later.
Prebuilt binaries exist for Linux x86_64, macOS (Apple silicon and Intel) and
Windows x86_64, which the script downloads; elsewhere it builds from source.

### With the install script

```sh
curl -fsSL https://raw.githubusercontent.com/aeron2k4/astlens/main/scripts/install.sh | bash
```

It checks the release against `SHA256SUMS`, saves the binary as
`~/.local/bin/astlens` (`ASTLENS_BIN_DIR` to change) and adds that directory to
`PATH` if missing, through a marked block in the rc file of `$SHELL`. On
Windows, run it in Git Bash. It needs no `sudo`.

A specific release, or a build from source instead of a download:

```sh
curl -fsSL https://raw.githubusercontent.com/aeron2k4/astlens/main/scripts/install.sh | bash -s -- --version v0.7.1
curl -fsSL https://raw.githubusercontent.com/aeron2k4/astlens/main/scripts/install.sh | bash -s -- --from-source
```

`--from-source` needs `cargo`, and so does a platform with no prebuilt binary,
which the script builds for on its own. A build goes to
`~/.cargo/bin/astlens`, since cargo keeps it.

### With cargo, from crates.io

```sh
cargo install astlens
```

This puts the binary in `~/.cargo/bin`, which cargo does not add to `PATH` for
you.

### With cargo, from git

```sh
cargo install --git https://github.com/aeron2k4/astlens --tag v0.7.1 --locked
```

Same destination, `~/.cargo/bin`. Without `--tag`, cargo builds the tip of
`main`.

### From a clone

```sh
git clone https://github.com/aeron2k4/astlens.git
cd astlens
cargo build --release
./target/release/astlens --serve /path/to/project
```

To install that build instead of running it from `target`, use
`cargo install --path . --locked --force`, or run `scripts/install.sh
--from-source` inside the clone.

### Set up the client

Any of the above leaves an `astlens` on `PATH`. Then set up the client. This
needs `jq`, `bash` when enforcing, and `python3` for Antigravity (on Windows,
`winget install jqlang.jq`):

```sh
astlens install
```

It detects Claude Code, OpenCode and Antigravity, setting up each one it finds.
Pass `--client claude`, `--client opencode`, `--client antigravity`, or
`--client all` to choose yourself. Restart the client afterwards.

### Updating

```sh
astlens update          # download the latest release over this binary
astlens update --check  # only report whether a newer one exists
```

`update` asks GitHub for the latest release, compares its tag with the
version running, downloads the asset for your platform and replaces the binary
that is running, after checking the download against the release's
`SHA256SUMS`. A checksum mismatch, a failed download or an unwritable directory
leaves the old binary in place. The asset has no version in its name, so the
command works the same whatever release is current.

A binary in `~/.local/bin` (installed by `scripts/install.sh`) is replaced. A
binary cargo installed, in `~/.cargo/bin`, is not: `update` prints the version
and the command to run instead, so cargo's record of what is installed stays
right. For a crates.io install, `cargo install astlens` again is enough to move
to a newer version; `--force` only redoes a build that has no newer version to
fetch, such as one from `--git` or `--path`, or rebuilds the version already
installed. A binary in `/usr` or `/opt` belongs to a package manager, and
`update` leaves it to that.

If both locations hold a copy, only the one running is updated, and the other
is named. Remove it with `cargo uninstall astlens` (a cargo copy) or by hand (a
`~/.local/bin` copy); `uninstall` removes only the binary it runs from.

`astlens doctor` reports the same facts without changing anything: the running
binary, its installer, the other copy, and whether `update` would replace it.

### What `install` changes

All of it is at user scope, so it applies to every project.

Claude Code:

- Registers the MCP server as `astlens`, with its tools always loaded.
- Writes the hook `~/.claude/hooks/astlens-guard.sh` and wires it in
  `~/.claude/settings.json`.
- Adds a short instruction block to `~/.claude/CLAUDE.md`.

OpenCode (v2):

- Adds the server to `mcp.servers.astlens` in `opencode.json` or
  `opencode.jsonc` in the OpenCode config directory, usually `~/.config/opencode`.
  Comments and other settings stay as they are; the file is copied to
  `opencode.json.bak` first. Its tools are exposed directly, not through Code
  Mode, as `astlens_<tool>`.
- OpenCode reads a global `opencode.json`, then `opencode.jsonc`, and a file's
  non-empty `mcp.servers` map replaces the earlier one entirely. The entry
  therefore goes into `opencode.jsonc` when that file lists servers, and
  otherwise into whichever file exists (`opencode.json` first). An astlens
  entry left in the other file by an earlier install is removed.
- Writes the plugin `~/.config/opencode/plugins/astlens-guard/` with the shared
  guard script.
- Adds a short instruction block to `~/.config/opencode/AGENTS.md`.

Antigravity:

- Writes the workspace-aware MCP stdio bridge
  `~/.gemini/config/astlens/agy-mcp-bridge.py` and registers `mcpServers.astlens`
  in `~/.gemini/config/mcp_config.json` (backing it up to `.json.bak`).
- Writes the plugin `~/.gemini/config/plugins/astlens/` (`plugin.json`,
  `hooks.json`, `agy-guard.py`, `astlens-guard.sh`, and `rules/AGENTS.md`).

The hook and plugin block the model from reading or searching source files in
the session's workspace the usual way, and point it to the astlens tools
instead. They deny whole-file `Read`/`read`, the `Grep`/`grep` tool, and
commands such as `cat`, `grep` and `rg` on those files. A read or command
output limited to 300 lines (`ASTLENS_MAX_READ_LINES`) is still allowed, as are
non-code files and paths outside the workspace.

`astlens install --no-enforce` registers the MCP server only, with no hook,
plugin or instruction block.

## Usage

Once installed, the client starts the server itself, using the session's working
directory as the workspace root.

| Command                     | Effect                                                       |
| --------------------------- | ------------------------------------------------------------ |
| `astlens install`           | Set up Claude Code, OpenCode and/or Antigravity              |
| `astlens install --client antigravity` | Set up one client (`claude`, `opencode`, `antigravity`, `all`) |
| `astlens install --no-enforce` | Register the MCP server only                              |
| `astlens uninstall`         | Undo `install`, remove the `PATH` entry and the binary       |
| `astlens update`            | Replace this binary with the latest release                  |
| `astlens update --check`    | Only report whether a newer release exists                   |
| `astlens doctor`            | Report the running binary and how `update` treats it         |
| `astlens -s`, `--serve [ROOT]` | Run the server over stdio                                 |
| `astlens -h`, `--help`      | Show the help                                                |
| `astlens -v`, `--version`   | Show the version                                             |

A bare word names a command. `--serve` and the two display flags stay options
because the MCP client starts the server itself, and because `--help` and
`--version` are what every program's user already types. An older install
spelled `install`, `uninstall` and `update` as flags too; those spellings are
gone. A directory named after a command is served with `--serve <name>` or
`$ASTLENS_ROOT`.

`uninstall` keeps the parse cache in `~/.cache/astlens`. `update` is the
one command that uses the network.

## Tools

| Tool               | Purpose                                                                          | Access |
| ------------------ | -------------------------------------------------------------------------------- | ------ |
| `codebase_map`     | The symbols the rest of the code uses most, by file, within `max_tokens`         | read   |
| `get_file_outline` | Symbols in a file with signatures and line ranges                                | read   |
| `search_symbols`   | Ranked name search across the workspace (exact > prefix > substring)             | read   |
| `read_symbol`      | Exact source of one or more definitions in a file, line-numbered                 | read   |
| `read_lines`       | A line range of a file, line-numbered, at most 400 lines per call                | read   |
| `replace_symbol`   | Rewrite or delete one definition without reading the file; rejects syntax errors | write  |
| `replace_text`     | Exact-text edits, each matching once, applied all or none; rejects syntax errors | write  |
| `replace_lines`    | Replace/delete a line range, guarded by the lines as they are now                | write  |
| `insert_symbol`    | Add code after/before a symbol or at the end of a file; rejects syntax errors    | write  |
| `read_file_pruned` | File text within `max_tokens`: full → bodies elided → outline → truncated        | read   |
| `find_references`  | Uses of a symbol from identifier tokens (comments and strings skipped)           | read   |
| `search_text`      | Literal or regex search over files, honouring `.gitignore`, skipping hidden ones | read   |
| `list_files`       | Files under a directory, honouring `.gitignore`                                  | read   |
| `changed_symbols`  | Symbols added/modified/removed by uncommitted changes or since a revision        | read   |
| `git_status`       | Working-tree status                                                              | read   |

Every tool carries all four MCP annotations as explicit booleans. `read`
means `readOnlyHint: true`. `write` means `readOnlyHint: false`, and
`destructiveHint: true` for the three that can delete code (`replace_symbol`,
`replace_text`, `replace_lines`). `insert_symbol` only adds, so
`destructiveHint: false`. No tool reaches the network: `openWorldHint: false`
throughout. `update` is a command, not a tool, and is the only thing that
does.

Files in other languages are plain text: `read_lines`, `read_file_pruned` (the
text, cut at `max_tokens`), `replace_text`, `replace_lines` and `insert_symbol`
at the end of the file work on them, without the syntax check.

`codebase_map` ranks files by PageRank over the names they mention and the
files defining them, then prints the best symbols of the best files, each under
the symbols enclosing it. A mention only counts where the language lets it
reach the definition: another file of the same language family, outside the
definition's function, past its `static`, `private` or unexported scope, and
qualified or imported the way the language needs (a Go package, a Python
module, a JavaScript import path, the owner of a method). A name defined once
per platform in one directory (Go build constraints, Rust `cfg`) counts once,
for its most common build. Private symbols (a Go lower-case name, a Python
`_name`, a Rust item without `pub`) score below public ones used as much, so
the map leads with what the rest of the code can call. `focus` takes files or
identifiers and ranks around them; `dir` limits the map to a directory.

Paths are relative to the workspace root. The tools that only read
(`get_file_outline`, `read_symbol`, `read_lines`, `read_file_pruned`, and
`codebase_map`, `search_symbols`, `search_text`, `list_files`, `find_references`
through `dir`) also take the absolute or `~/` path of a dependency source. They look under these
directories, when they exist:
- `~/.cargo/registry/src`, `~/.cargo/git/checkouts` (`CARGO_HOME`)
- `~/.rustup/toolchains` (`RUSTUP_HOME`)
- the Go module cache (`GOMODCACHE`, `GOPATH`) and standard library
  (`GOROOT/src`, else `/usr/local/go`, `/usr/lib/go`, Homebrew's or
  `~/sdk/go*`)
- `/usr/include`, `/usr/local/include`
- `node_modules` and the `python3*` directories in `/usr/lib` and
  `/usr/local/lib`, and the `python3*` directories in `~/.local/lib`

`ASTLENS_LIBRARY_PATHS` adds more directories, separated like `PATH`. The tools
that write stay inside the workspace.

## Build from source

Rust 1.90 or later. This is also the way on a platform without a prebuilt
binary, such as Linux ARM. The commands are under
[Installation](#installation): `cargo build --release` from a clone,
`cargo install --path .`, or `scripts/install.sh --from-source`. Run
`astlens install` afterwards.

To contribute, see [`CONTRIBUTING.md`](CONTRIBUTING.md).

## Privacy

astlens runs locally, sends no data anywhere and keeps no account. Its parse
cache lives in `~/.cache/astlens`. `update` is the only command that
contacts a server, and only to fetch a release. See [`PRIVACY.md`](PRIVACY.md).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
