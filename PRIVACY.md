# Privacy

astlens is a local program. It runs as an MCP server over stdio on the machine
that starts it, and it has no backend, no account and no telemetry.

## What it collects

Nothing. astlens sends no data anywhere. The only network access it makes is
the download of its own binary and of its dependencies, when you build it or
install it; the server itself does not open a network connection.

## What stays on your machine

- **Your source code.** The read tools open files under the workspace root and
  under the dependency directories listed in the README (`~/.cargo/registry/src`,
  the Go module cache, system headers, and so on). The write tools change files
  inside the workspace only, and only when the client asks them to.
- **Parse cache.** Parsed syntax trees and symbol names are cached in
  `~/.cache/astlens`, or in `$ASTLENS_CACHE_DIR` when set, to make a restart
  fast. Delete the directory to remove it, or set `ASTLENS_NO_CACHE=1` to keep
  the cache in memory for one run only. The cache holds derived data from the
  files you opened, so treat it as you treat those files.
- **Logs.** Diagnostics go to stderr, never to stdout, which carries the
  protocol. `ASTLENS_LOG` sets the level. No log is written to a file unless
  the client that started astlens redirects stderr to one.

## What it shares with your AI client

The MCP client you connect it to decides what leaves your machine. astlens
returns file contents, symbols and search results to that client, and the
client may send them to a model provider under its own terms. That transfer is
between you and your client; astlens is not a party to it and has no visibility
into it.

## Third parties

None. There is no analytics, no crash reporting, no advertising identifier and
no third-party SDK that phones home.

## Children

astlens is a developer tool and is not directed at children.

## Changes

If this policy changes, the change lands in this file and in the release
notes.

## Contact

Open an issue at
<https://github.com/aeron2k4/code-scope-mcp/issues>, or report a security
problem privately through the repository's Security tab, as described in
[`SECURITY.md`](SECURITY.md).
