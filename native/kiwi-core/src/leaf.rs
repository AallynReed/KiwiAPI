//! Pure parts of the small leaf routes: the supporters list cleanup
//! (`app/supporters/service.py`) and the design-feedback tallies
//! (`app/design_feedback/service.py`).

/// `str.isspace()` for one char (Rust's `is_whitespace` lacks U+001C..U+001F).
fn is_py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// `str.strip()` with no arguments.
pub fn py_strip(s: &str) -> &str {
    s.trim_matches(is_py_space)
}

/// `str.strip().lower()`.
pub fn normalize_handle(s: &str) -> String {
    py_strip(s).to_lowercase()
}

/// `replace()`'s cleanup: trimmed, blanks dropped, case-insensitive
/// de-dupe keeping the first spelling.
pub fn clean_supporter_names<S: AsRef<str>>(names: &[S]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for raw in names {
        let n = py_strip(raw.as_ref());
        if n.is_empty() {
            continue;
        }
        let key = n.to_lowercase();
        if !seen.contains(&key) {
            seen.push(key);
            out.push(n.to_owned());
        }
    }
    out
}

pub const DESIGNS: [&str; 2] = ["voxel", "classic"];
pub const THEMES: [&str; 3] = ["auto", "light", "dark"];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Votes {
    pub up: i64,
    pub down: i64,
}

impl Votes {
    fn add(&mut self, up: bool, n: i64) {
        if up {
            self.up += n;
        } else {
            self.down += n;
        }
    }
}

/// One stored counter. `design`/`theme`/`day` are already `value or "?"`.
#[derive(Clone, Debug)]
pub struct FeedbackRow<'a> {
    pub vote: Option<&'a str>,
    pub count: i64,
    pub design: &'a str,
    pub theme: &'a str,
    pub day: &'a str,
}

/// `summary()`'s aggregates, maps as ordered `(key, votes)` lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tally {
    pub total: Votes,
    pub by_design: Vec<(String, Votes)>,
    pub by_theme: Vec<(String, Votes)>,
    /// Sorted by day.
    pub series: Vec<(String, Votes)>,
}

fn bump(list: &mut Vec<(String, Votes)>, key: &str, up: bool, n: i64) {
    match list.iter_mut().find(|(k, _)| k == key) {
        Some((_, v)) => v.add(up, n),
        None => {
            let mut v = Votes::default();
            v.add(up, n);
            list.push((key.to_owned(), v));
        }
    }
}

/// Up/down totals split by design, theme and day; rows with any other vote
/// are skipped. Known designs and themes are listed first, in order.
pub fn tally<'a>(rows: impl IntoIterator<Item = FeedbackRow<'a>>) -> Tally {
    let seed = |keys: &[&str]| keys.iter().map(|k| (k.to_string(), Votes::default())).collect::<Vec<_>>();
    let mut t =
        Tally { total: Votes::default(), by_design: seed(&DESIGNS), by_theme: seed(&THEMES), series: Vec::new() };
    for r in rows {
        let up = match r.vote {
            Some("up") => true,
            Some("down") => false,
            _ => continue,
        };
        t.total.add(up, r.count);
        bump(&mut t.by_design, r.design, up, r.count);
        bump(&mut t.by_theme, r.theme, up, r.count);
        bump(&mut t.series, r.day, up, r.count);
    }
    t.series.sort_by(|a, b| a.0.cmp(&b.0));
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_matches_python() {
        assert_eq!(py_strip("\u{3000} a b\u{1c}\u{1d}\u{85}"), "a b");
        assert_eq!(py_strip("\u{200b}x"), "\u{200b}x");
        assert_eq!(normalize_handle(" ΣΑΣ "), "σας");
        assert_eq!(normalize_handle("İ"), "i\u{307}");
    }

    #[test]
    fn clean_keeps_first_spelling() {
        assert_eq!(clean_supporter_names(&["  Nz ", "nz", "NZ", "Wahoo"]), ["Nz", "Wahoo"]);
        assert_eq!(clean_supporter_names(&["Straße", "STRASSE", "strasse"]), ["Straße", "STRASSE"]);
        assert!(clean_supporter_names(&["", " \t"]).is_empty());
    }

    #[test]
    fn tally_orders_and_skips() {
        let row = |vote, count, design, theme, day| FeedbackRow { vote, count, design, theme, day };
        let t = tally([
            row(Some("up"), 3, "voxel", "auto", "2026-02-28"),
            row(Some("down"), 2, "?", "sepia", "2026-02-27"),
            row(Some("maybe"), 9, "x", "y", "2026-01-01"),
            row(None, 9, "x", "y", "2026-01-01"),
        ]);
        assert_eq!(t.total, Votes { up: 3, down: 2 });
        let keys: Vec<_> = t.by_design.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["voxel", "classic", "?"]);
        let keys: Vec<_> = t.by_theme.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["auto", "light", "dark", "sepia"]);
        let days: Vec<_> = t.series.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(days, ["2026-02-27", "2026-02-28"]);
    }
}
