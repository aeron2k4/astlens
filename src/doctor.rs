//! `doctor`: report what `install` and `update` make of the running binary.
//!
//! It only reads: it writes no file and sends no request. `astlens update
//! --check` is the command that asks GitHub for the latest tag.

use std::env;

use anyhow::{Context, Result, bail};

use crate::install::{self, Owner};
use crate::update;

/// Print where this binary came from and what each installer would do with it,
/// then fail when something would stop `update` from replacing it.
pub fn run() -> Result<()> {
    let exe = env::current_exe().context("cannot name the running binary")?;
    let exe = exe.canonicalize().unwrap_or(exe);
    let version = env!("CARGO_PKG_VERSION");
    let owner = install::owner(&exe);

    println!("astlens v{version}");
    println!("binary: {}", exe.display());
    println!("platform: {} {}", env::consts::OS, env::consts::ARCH);
    println!("installed by: {}", installed_by(owner));
    println!("update: {}", update_action(owner));
    match install::other_copy_of(&exe) {
        Some(other) => println!("other copy: {} ({})", other.display(), other_hint(owner)),
        None => println!("other copy: none"),
    }

    let Some(asset) = update::current_asset() else {
        println!("release asset: none for this platform");
        bail!(
            "no prebuilt binary for {} {}; build from source: scripts/install.sh --from-source",
            env::consts::OS,
            env::consts::ARCH
        );
    };
    println!("release asset: {asset}");
    Ok(())
}

fn installed_by(owner: Owner) -> &'static str {
    match owner {
        Owner::Script => "scripts/install.sh",
        Owner::Cargo => "cargo install",
        Owner::PackageManager => "a package manager",
        Owner::User => "none of the installers this project ships",
    }
}

fn update_action(owner: Owner) -> &'static str {
    match owner {
        Owner::Script | Owner::User => "replaces this binary in place",
        Owner::Cargo => "leaves this binary alone; run: cargo install astlens",
        Owner::PackageManager => "leaves this binary alone; use the package manager",
    }
}

/// The copy `other_copy_of` finds is the one this binary is not: cargo's when
/// the script installed this one, and the other way round.
fn other_hint(owner: Owner) -> &'static str {
    match owner {
        Owner::Script => "cargo's copy; remove it with: cargo uninstall astlens",
        _ => "the install script's copy; remove it by hand",
    }
}
