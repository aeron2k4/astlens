//! astlens: AST-aware code navigation MCP server over stdio.
//!
//! While serving, `stdout` is reserved for JSON-RPC traffic. All diagnostics go
//! to `stderr`.
//!
//! Commands are in [`cli`]. `--serve [ROOT]` runs the server; the workspace root
//! defaults to `$ASTLENS_ROOT`, then the current directory.
//!
//! Parse results persist in `$ASTLENS_CACHE_DIR`, defaulting to
//! `$XDG_CACHE_HOME/astlens` or `~/.cache/astlens`. Set
//! `ASTLENS_NO_CACHE=1` to keep the cache in memory only, and
//! `ASTLENS_MAX_FILES` to change how many files a search walks and
//! `codebase_map` maps (20000), and
//! `ASTLENS_CACHE_MB` to change the memory kept for parsed files and their
//! symbol names (512).
//! `ASTLENS_LIBRARY_PATHS` adds directories, separated like `PATH`, to the
//! dependency sources the read-only tools may open (see [`libraries`]).

use std::ffi::OsString;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::thread::JoinHandle;

use anyhow::Result;
use astlens::cli::{self, Command};
use astlens::index::cache::PersistentStore;
use astlens::mcp::{AstLens, Workspace};
use astlens::{doctor, install, libraries, update};
use rmcp::ServiceExt;
use tracing_subscriber::EnvFilter;

fn init_tracing() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_env_filter(
            EnvFilter::try_from_env("ASTLENS_LOG").unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
}

fn workspace_root(arg: Option<PathBuf>) -> Result<(PathBuf, bool)> {
    if let Some(arg) = arg {
        return Ok((arg, true));
    }
    if let Some(env) = std::env::var_os("ASTLENS_ROOT").filter(|v| !v.is_empty()) {
        return Ok((env.into(), true));
    }
    Ok((std::env::current_dir()?, false))
}

fn cache_dir() -> Option<PathBuf> {
    if std::env::var_os("ASTLENS_NO_CACHE").is_some_and(|v| !v.is_empty() && v != "0") {
        return None;
    }
    if let Some(dir) = std::env::var_os("ASTLENS_CACHE_DIR") {
        return Some(dir.into());
    }
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from))
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(|h| PathBuf::from(h).join(".cache"))
        })?;
    Some(base.join("astlens"))
}

/// Drop stored results of deleted files on a background thread, so the
/// server answers `initialize` without waiting for the sweep. Its handle goes
/// back to the caller, whose job it is to wait before the last flush: a save
/// can clear the store whole, and an entry the sweep has not deleted yet would
/// then be dropped from an iterator that is still walking it.
fn prune_in_background(store: PersistentStore, root: PathBuf) -> JoinHandle<()> {
    std::thread::spawn(move || match store.prune_missing(&root) {
        Ok(0) => {}
        Ok(n) => tracing::info!(removed = n, "pruned parse results of deleted files"),
        Err(err) => tracing::warn!(%err, "failed to prune the parse store"),
    })
}

/// Attach the persistent store when available; a broken or unwritable cache
/// directory only costs re-parsing, so it is never fatal. The prune sweep it
/// starts is handed back, None when there is no store.
fn with_persistence(workspace: Workspace) -> (Workspace, Option<JoinHandle<()>>) {
    let Some(dir) = cache_dir() else {
        tracing::info!("persistent cache disabled");
        return (workspace, None);
    };
    match PersistentStore::open(&dir) {
        Ok(store) => {
            tracing::info!(dir = %dir.display(), "persistent cache enabled");
            let sweep = (!workspace.has_broad_implicit_root())
                .then(|| prune_in_background(store.clone(), workspace.root()));
            (workspace.with_store(store), sweep)
        }
        Err(err) => {
            tracing::warn!(%err, "persistent cache unavailable, using memory only");
            (workspace, None)
        }
    }
}

fn main() -> Result<()> {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let command = match cli::parse(&args, std::io::stdin().is_terminal()) {
        Ok(command) => command,
        Err(message) => {
            eprintln!("error: {message}\n\n{}", cli::HELP);
            std::process::exit(2);
        }
    };
    // Only `--serve` speaks JSON-RPC, so only it reserves stdout.
    match command {
        Command::Help => print!("{}", cli::HELP),
        Command::Version => println!("v{}", env!("CARGO_PKG_VERSION")),
        Command::Install { enforce, client } => install::install(enforce, client)?,
        Command::Uninstall => install::uninstall()?,
        Command::Serve(root) => serve(root)?,
        Command::Update { check_only } => update::run(check_only)?,
        Command::Doctor => doctor::run()?,
    }
    Ok(())
}

#[tokio::main]
async fn serve(root: Option<PathBuf>) -> Result<()> {
    init_tracing();
    let (root, explicit) = workspace_root(root)?;
    let base = Workspace::new(root)?;
    let base = if explicit {
        base
    } else {
        base.with_implicit_root()
    };
    let (persisted, sweep) = with_persistence(base);
    let mut workspace = persisted.with_libraries(libraries::default_roots());
    if let Some(max) = std::env::var("ASTLENS_MAX_FILES")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
    {
        workspace = workspace.with_max_files(max);
    }
    if let Some(mb) = std::env::var("ASTLENS_CACHE_MB")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
    {
        workspace = workspace.with_cache_bytes(mb.saturating_mul(1 << 20));
    }
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        root = %workspace.root().display(),
        "astlens starting"
    );
    if workspace.has_broad_implicit_root() {
        tracing::warn!(
            root = %workspace.root().display(),
            "no explicit root given and working directory is broad; writes and unscoped walks \
             are disabled unless the MCP client supplies roots"
        );
    }

    let service = AstLens::new(workspace)
        .serve(rmcp::transport::stdio())
        .await
        .inspect_err(|err| tracing::error!(%err, "failed to start MCP server"))?;
    service.waiting().await?;
    tracing::info!("client disconnected, shutting down");
    // Before the cache's last flush, so the two never walk and clear the store
    // at once. A sweep that panicked is no reason to keep the process alive.
    if let Some(sweep) = sweep {
        let _ = sweep.join();
    }
    Ok(())
}
