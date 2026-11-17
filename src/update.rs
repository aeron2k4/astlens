//! `--update`: replace this binary with the latest GitHub release.

use anyhow::Result;

pub fn run(check_only: bool) -> Result<()> {
    let _ = check_only;
    Ok(())
}
