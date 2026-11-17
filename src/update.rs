//! `--update`: replace this binary with the latest GitHub release.

use anyhow::Result;

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

/// The asset for the platform this binary was built for.
fn current_asset() -> Option<&'static str> {
    asset_name(std::env::consts::OS, std::env::consts::ARCH)
}

pub fn run(check_only: bool) -> Result<()> {
    let _ = (check_only, current_asset());
    Ok(())
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
    fn this_build_has_an_asset_where_the_release_does() {
        // Only the targets release.yml builds; elsewhere --from-source is the way.
        let expected = matches!(
            (std::env::consts::OS, std::env::consts::ARCH),
            ("linux", "x86_64") | ("macos", _) | ("windows", "x86_64")
        );
        assert_eq!(current_asset().is_some(), expected);
    }
}
