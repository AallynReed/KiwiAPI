//! BetterTroveTools rules: release channels, platform asset matching, version
//! comparison and commit prefixes from `app/trove/btt_releases.py`, and the
//! mod blocklist's value parsing from `app/btt_blocklist/service.py`.

use std::sync::LazyLock;

use regex::Regex;

pub const PLATFORMS: [&str; 3] = ["windows", "linux", "android"];
pub const CHANNELS: [&str; 2] = ["release", "beta"];

/// `CHANNELS[channel]`: the GitHub `prerelease` flag of a channel.
pub fn channel_prerelease(channel: &str) -> Option<bool> {
    match channel {
        "release" => Some(false),
        "beta" => Some(true),
        _ => None,
    }
}

/// `PLATFORM_EXTENSIONS[platform]`, highest priority first.
pub fn extensions(platform: &str) -> &'static [&'static str] {
    match platform {
        "windows" => &[".msi", ".exe"],
        "linux" => &[".appimage", ".deb", ".rpm", ".tar.gz"],
        "android" => &[".apk"],
        _ => &[],
    }
}

/// `_asset_priority`.
pub fn asset_priority(name: &str, platform: &str) -> Option<usize> {
    let lowered = name.to_lowercase();
    extensions(platform).iter().position(|ext| lowered.ends_with(ext))
}

/// `assets_for_platform` by asset name: indices of the matches, sorted
/// (stably) by priority, then name.
pub fn platform_order(names: &[&str], platform: &str) -> Vec<usize> {
    let mut matched: Vec<(usize, usize)> =
        names.iter().enumerate().filter_map(|(i, n)| asset_priority(n, platform).map(|p| (p, i))).collect();
    matched.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| names[a.1].cmp(names[b.1])));
    matched.into_iter().map(|(_, i)| i).collect()
}

/// `str.isspace()` for one char.
pub fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// `str.strip()`.
pub fn py_strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

static VERSION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^v?(\d{1,9}(?:\.\d{1,9}){0,9})(.*)$").expect("version regex"));
static DIGIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d$").expect("digit regex"));

fn is_decimal(c: char) -> bool {
    let mut buf = [0u8; 4];
    DIGIT.is_match(c.encode_utf8(&mut buf))
}

/// `int()` of one Unicode decimal digit: every Nd run is whole 0..9 sets.
fn digit_value(c: char) -> u64 {
    if let Some(d) = c.to_digit(10) {
        return d.into();
    }
    let mut zero = c as u32;
    while let Some(prev) = zero.checked_sub(1).and_then(char::from_u32)
        && is_decimal(prev)
    {
        zero -= 1;
    }
    ((c as u32 - zero) % 10).into()
}

/// `parse_version`: `'v1.2.3-beta.1'` -> `([1, 2, 3], "-beta.1")`.
pub fn parse_version(value: &str) -> Option<(Vec<u64>, String)> {
    if value.is_empty() || value.chars().count() > 64 {
        return None;
    }
    let caps = VERSION.captures(py_strip(value))?;
    let nums = caps[1].split('.').map(|part| part.chars().fold(0, |acc, c| acc * 10 + digit_value(c))).collect();
    Some((nums, caps[2].to_owned()))
}

/// `compare_versions`: -1, 0 or 1, `None` when either side doesn't parse.
pub fn compare_versions(a: &str, b: &str) -> Option<i32> {
    let (mut na, sa) = parse_version(a)?;
    let (mut nb, sb) = parse_version(b)?;
    let width = na.len().max(nb.len());
    na.resize(width, 0);
    nb.resize(width, 0);
    if na != nb {
        return Some(if na < nb { -1 } else { 1 });
    }
    Some(if sa == sb {
        0
    } else if sa.is_empty() {
        1
    } else if sb.is_empty() || sa < sb {
        -1
    } else {
        1
    })
}

/// `parse_conventional_prefix`: `'feat(api): add X'` -> `"feat"`.
pub fn conventional_prefix(message: &str) -> Option<String> {
    let s = py_strip(message);
    let n = s.bytes().take_while(u8::is_ascii_alphabetic).count();
    if n == 0 {
        return None;
    }
    let rest = &s[n..];
    let rest = match rest.strip_prefix('(') {
        Some(scope) => match scope.find(')') {
            Some(close) if close > 0 => &scope[close + 1..],
            _ => rest,
        },
        None => rest,
    };
    rest.starts_with(':').then(|| s[..n].to_ascii_lowercase())
}

pub const BLOCK_KINDS: [&str; 5] = ["author", "mod", "hash", "workshop", "workshop_author"];

static DIGITS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d+$").expect("digits regex"));
static MD5: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9a-f]{32}$").expect("md5 regex"));
static STEAM_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^7656\d{13}$").expect("steamid regex"));

/// `_LABEL[kind]`.
pub fn block_label(kind: &str) -> &'static str {
    match kind {
        "author" => "author id",
        "mod" => "mod id",
        "hash" => "md5 hash",
        "workshop" => "Steam Workshop item id",
        "workshop_author" => "SteamID64",
        _ => "",
    }
}

/// `_VALID[kind].fullmatch(value)`.
pub fn valid_block_value(kind: &str, value: &str) -> bool {
    match kind {
        "author" | "mod" | "workshop" => DIGITS.is_match(value),
        "hash" => MD5.is_match(value),
        "workshop_author" => STEAM_ID.is_match(value),
        _ => false,
    }
}

/// `parse_values`: split on runs of whitespace and commas, lowercased, first
/// occurrence kept.
pub fn parse_block_values(raw: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    raw.split(|c: char| c == ',' || is_space(c))
        .filter(|p| !p.is_empty())
        .map(str::to_lowercase)
        .filter(|v| seen.insert(v.clone()))
        .collect()
}

/// The checks `add` makes before inserting: the values, or the 400 message.
pub fn check_block_values(kind: &str, raw: &str) -> Result<Vec<String>, String> {
    let values = parse_block_values(raw);
    if values.is_empty() {
        return Err("Enter at least one value.".into());
    }
    let bad: Vec<&str> = values.iter().filter(|v| !valid_block_value(kind, v)).map(String::as_str).collect();
    if !bad.is_empty() {
        let shown: Vec<&str> = bad.into_iter().take(5).collect();
        return Err(format!("Not a valid {}: {}", block_label(kind), shown.join(", ")));
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v1.2.3-beta.1"), Some((vec![1, 2, 3], "-beta.1".into())));
        assert_eq!(parse_version(" 1.0 "), Some((vec![1, 0], String::new())));
        assert_eq!(parse_version("x1"), None);
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version(&"1".repeat(65)), None);
        assert_eq!(parse_version("1234567890"), Some((vec![123456789], "0".into())));
        assert_eq!(parse_version("\u{661}.\u{6f2}"), Some((vec![1, 2], String::new())));
        assert_eq!(parse_version("\u{1d7d9}"), Some((vec![1], String::new())));
        assert_eq!(compare_versions("1.0", "1.0.0"), Some(0));
        assert_eq!(compare_versions("1.0.0", "1.0.0-beta"), Some(1));
        assert_eq!(compare_versions("1.0.0-alpha", "1.0.0-beta"), Some(-1));
        assert_eq!(compare_versions("v1.2", "1.10"), Some(-1));
        assert_eq!(compare_versions("nope", "1"), None);
    }

    #[test]
    fn prefixes() {
        assert_eq!(conventional_prefix("feat(api): x").as_deref(), Some("feat"));
        assert_eq!(conventional_prefix("  Fix: y").as_deref(), Some("fix"));
        assert_eq!(conventional_prefix("feat(): x"), None);
        assert_eq!(conventional_prefix("feat(a)x: y"), None);
        assert_eq!(conventional_prefix("1fix: y"), None);
        assert_eq!(conventional_prefix(""), None);
    }

    #[test]
    fn platforms() {
        let names = ["b.EXE", "a.msi", "a.exe", "x.AppImage", "y.apk"];
        assert_eq!(platform_order(&names, "windows"), vec![1, 2, 0]);
        assert_eq!(platform_order(&names, "linux"), vec![3]);
        assert_eq!(platform_order(&names, "android"), vec![4]);
    }

    #[test]
    fn block_values() {
        assert_eq!(parse_block_values(" 1,2\u{1f}2 ,, 3\t"), vec!["1", "2", "3"]);
        assert_eq!(check_block_values("mod", " , "), Err("Enter at least one value.".into()));
        assert_eq!(check_block_values("hash", "ABC").unwrap_err(), "Not a valid md5 hash: abc");
        assert_eq!(check_block_values("workshop_author", "76561198000000000"), Ok(vec!["76561198000000000".into()]));
        assert_eq!(check_block_values("mod", "a b c d e f").unwrap_err(), "Not a valid mod id: a, b, c, d, e");
    }
}
