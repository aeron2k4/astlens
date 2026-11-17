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

/// `tag_name` from the `releases/latest` reply, without its quotes.
fn parse_tag(json: &str) -> Option<&str> {
    let at = json.find("\"tag_name\"")? + "\"tag_name\"".len();
    let rest = json[at..].trim_start().strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(&rest[..end])
}

/// `v0.7.0` or `0.7.0` as three numbers. Anything else, such as a
/// prerelease tag, is not a version this command can compare.
fn parse_version(tag: &str) -> Option<(u64, u64, u64)> {
    let tag = tag.strip_prefix('v').unwrap_or(tag);
    let mut parts = tag.split('.');
    let mut next = || {
        let part = parts.next()?;
        // Digits only: `1x` would otherwise parse as 1.
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        part.parse().ok()
    };
    let version = (next()?, next()?, next()?);
    parts.next().is_none().then_some(version)
}

/// Whether the release is a later version than the one running.
fn is_newer(tag: &str, current: &str) -> bool {
    match (parse_version(tag), parse_version(current)) {
        (Some(release), Some(current)) => release > current,
        _ => false,
    }
}

/// The hash `SHA256SUMS` gives the asset, from its second field: a binary
/// prefix `*` marks the line as binary mode, as `sha256sum -b` writes it.
fn expected_hash(sums: &str, asset: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let hash = fields.next()?;
        let same = fields.next().is_some_and(|name| {
            let name = name.strip_prefix('*').unwrap_or(name);
            name == asset
        });
        (same && is_sha256(hash)).then(|| hash.to_ascii_lowercase())
    })
}

/// 64 hexadecimal digits, the length a SHA-256 hash has.
fn is_sha256(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn run(check_only: bool) -> Result<()> {
    let _ = (
        check_only,
        current_asset(),
        parse_tag(""),
        is_newer("", ""),
        expected_hash("", ""),
    );
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
    fn the_tag_comes_out_of_the_release_reply() {
        let reply = r#"{
  "url": "https://api.github.com/repos/aeron2k4/astlens/releases/1",
  "tag_name": "v0.7.0",
  "name": "astlens 0.7.0",
  "draft": false
}"#;
        assert_eq!(parse_tag(reply), Some("v0.7.0"));
        assert_eq!(parse_tag(r#"{"tag_name":"v1.2.3"}"#), Some("v1.2.3"));
        assert_eq!(parse_tag(r#"{"tag_name" :  "v1.2.3" }"#), Some("v1.2.3"));
        assert_eq!(parse_tag(r#"{"name": "v1.2.3"}"#), None);
        assert_eq!(parse_tag("<html>"), None);
    }

    #[test]
    fn a_version_is_three_numbers() {
        assert_eq!(parse_version("v0.7.0"), Some((0, 7, 0)));
        assert_eq!(parse_version("0.6.13"), Some((0, 6, 13)));
        assert_eq!(parse_version("1.0.0"), Some((1, 0, 0)));
        for bad in [
            "v0.7",
            "v0.7.0.1",
            "v0.7.0-rc1",
            "vx.7.0",
            "v1x.0.0",
            "",
            "v",
        ] {
            assert_eq!(parse_version(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_later_release_is_newer() {
        assert!(is_newer("v0.7.0", "0.6.9"));
        assert!(is_newer("v0.10.0", "0.9.9"));
        assert!(is_newer("v1.0.0", "0.99.99"));
        assert!(!is_newer("v0.6.3", "0.6.3"));
        assert!(!is_newer("v0.6.2", "0.6.3"));
        // Nothing to compare against: never claims a newer version.
        assert!(!is_newer("garbage", "0.6.3"));
        assert!(!is_newer("v0.7.0", "garbage"));
    }

    #[test]
    fn the_sums_name_the_hash_of_one_asset() {
        let sums = concat!(
            "1111111111111111111111111111111111111111111111111111111111111111  astlens-macos\n",
            "2222222222222222222222222222222222222222222222222222222222222222  astlens-linux\n",
            "3333333333333333333333333333333333333333333333333333333333333333 *astlens-windows.exe\n",
        );
        assert_eq!(
            expected_hash(sums, "astlens-linux").as_deref(),
            Some("2222222222222222222222222222222222222222222222222222222222222222")
        );
        // The `*` of binary mode is not part of the name.
        assert_eq!(
            expected_hash(sums, "astlens-windows.exe").as_deref(),
            Some("3333333333333333333333333333333333333333333333333333333333333333")
        );
        assert_eq!(expected_hash(sums, "astlens-freebsd"), None);
    }

    #[test]
    fn a_sums_line_that_is_not_a_hash_is_no_answer() {
        let short = "abc  astlens-linux";
        assert_eq!(expected_hash(short, "astlens-linux"), None);
        let not_hex = format!("{}  astlens-linux", "z".repeat(64));
        assert_eq!(expected_hash(&not_hex, "astlens-linux"), None);
        // Upper case is the same hash.
        let upper = format!("{}  astlens-linux", "A".repeat(64));
        assert_eq!(expected_hash(&upper, "astlens-linux"), Some("a".repeat(64)));
        assert_eq!(expected_hash("", "astlens-linux"), None);
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
