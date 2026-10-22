//! `app/trove/leaderboards/parser.py` `parse_dump` with Python's exact
//! semantics, plus the pure parts of `service.insert_dump`: the trove-day
//! anchor, `normalize_timestamp`, reset boundaries and the capture
//! completeness verdict. The fast path is `lbdump::parse`; where that needs
//! Python (Unicode digits or spaces, a score only `float()` reads) the dump is
//! parsed again here, `_BOARD_RE` and `_parse_entries` rule for rule.

use std::collections::{HashMap, HashSet};

use crate::{lbdump, pynum, pystr};

pub const RESET_HOUR_SECS: i64 = 11 * 3600;
pub const DAY: i64 = 86400;

/// One board of a dump (`ParsedBoard`), borrowing the dump text.
#[derive(Clone, Debug, PartialEq)]
pub struct Board<'a> {
    pub name_id: &'a str,
    pub category_id: &'a str,
    /// `int(uuid)`, saturated (anything past `int4` fails the write anyway).
    pub uuid: i64,
    pub name: &'a str,
    /// `category.strip()`.
    pub category: &'a str,
    /// `(rank, player_name, score)`; rank saturated like `uuid`.
    pub entries: Vec<(i64, &'a str, f64)>,
    pub contest: Option<&'static str>,
}

/// `contest_type_for`.
pub fn contest_type_for(category_id: &str) -> Option<&'static str> {
    match category_id {
        "Leaderboard_Category_Contests_Daily" => Some("daily"),
        "Leaderboard_Category_Contests" => Some("weekly"),
        _ => None,
    }
}

struct Line<'a> {
    name_id: &'a str,
    category_id: &'a str,
    uuid: &'a str,
    name: &'a str,
    category: &'a str,
    entries: Vec<(i64, &'a str, f64)>,
}

/// `[^<stop>\r\n]+<stop>`: the field and the rest after `stop`.
fn field(s: &str, stop: char) -> Option<(&str, &str)> {
    let end = s.find(stop)?;
    let f = &s[..end];
    (!f.is_empty() && !f.contains('\r')).then(|| (f, &s[end + stop.len_utf8()..]))
}

/// `_BOARD_RE` at the start of one line (`\d` is any Unicode decimal).
fn match_line(line: &str) -> Option<(&str, &str, &str, &str, &str, &str)> {
    let (name_id, rest) = field(line, '$')?;
    let (category_id, rest) = field(rest, '$')?;
    let digits = rest.char_indices().find(|(_, c)| pynum::decimal(*c).is_none()).map_or(rest.len(), |(i, _)| i);
    let (uuid, rest) = rest.split_at(digits);
    if uuid.is_empty() {
        return None;
    }
    let rest = rest.strip_prefix(" = ")?;
    let (name, rest) = field(rest, '$')?;
    let hash = rest.find('#')?;
    let category = &rest[..hash];
    if category.is_empty() || category.contains('\r') {
        return None;
    }
    let entries = rest[hash..].strip_prefix("##")?;
    (!entries.is_empty()).then_some((name_id, category_id, uuid, name, category, entries))
}

/// `_parse_entries`. Err is the `ValueError` of `int()` on a rank past 4300 digits.
fn parse_entries(raw: &str) -> Result<Vec<(i64, &str, f64)>, String> {
    let mut out = Vec::new();
    for chunk in raw.split('|') {
        let chunk = pystr::strip(chunk);
        if chunk.is_empty() {
            continue;
        }
        let mut parts = chunk.split(';');
        let (Some(rank_s), Some(player), Some(score_s), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        if !pynum::is_decimal(rank_s) {
            continue;
        }
        let Some(score) = pynum::float(score_s) else { continue };
        out.push((pynum::int_of_digits(rank_s)?, player, score));
    }
    Ok(out)
}

fn exact_lines(text: &str) -> Result<Vec<Line<'_>>, String> {
    let mut out = Vec::new();
    for line in text.split('\n') {
        if let Some((name_id, category_id, uuid, name, category, entries)) = match_line(line) {
            out.push(Line { name_id, category_id, uuid, name, category, entries: parse_entries(entries)? });
        }
    }
    Ok(out)
}

/// `parse_dump`. Err is an exception Python would raise (only `int()` past
/// 4300 digits on a rank).
pub fn parse_dump(text: &str) -> Result<Vec<Board<'_>>, String> {
    let lines = match lbdump::parse(text) {
        Some(lines) => lines
            .into_iter()
            .map(|l| Line {
                name_id: l.name_id,
                category_id: l.category_id,
                uuid: l.uuid,
                name: l.name,
                category: l.category,
                entries: l.entries,
            })
            .collect(),
        None => exact_lines(text)?,
    };
    let mut real: HashMap<i64, Line<'_>> = HashMap::new();
    let mut contest: HashMap<i64, &'static str> = HashMap::new();
    let mut order: Vec<i64> = Vec::new();
    let mut seen: HashSet<i64> = HashSet::new();
    for l in lines {
        if pystr::strip(l.category).to_uppercase() == "FAVORITES" {
            continue;
        }
        let Ok(uuid) = pynum::int_of_digits(l.uuid) else { continue };
        if seen.insert(uuid) {
            order.push(uuid);
        }
        match contest_type_for(l.category_id) {
            Some(kind) => {
                contest.entry(uuid).or_insert(kind);
            }
            None => {
                real.entry(uuid).or_insert(l);
            }
        }
    }
    Ok(order
        .into_iter()
        .filter_map(|uuid| {
            let l = real.remove(&uuid)?;
            Some(Board {
                name_id: l.name_id,
                category_id: l.category_id,
                uuid,
                name: l.name,
                category: pystr::strip(l.category),
                entries: l.entries,
                contest: contest.get(&uuid).copied(),
            })
        })
        .collect())
}

/// `pg_store.trove_day_start`: the 11:00 UTC reset opening `anchor`'s trove-day.
pub fn trove_day_start(anchor: i64) -> i64 {
    (anchor - RESET_HOUR_SECS).div_euclid(DAY) * DAY + RESET_HOUR_SECS
}

/// The year `datetime.fromtimestamp(ts, UTC)` lands in.
fn utc_year(ts: i64) -> i64 {
    let days = ts.div_euclid(DAY);
    // civil_from_days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    yoe + era * 400 + i64::from(m <= 2)
}

/// `datetime.fromtimestamp(ts, UTC)` on Linux: Err is the exception text.
fn check_timestamp(ts: i64) -> Result<(), String> {
    let year = utc_year(ts);
    if year - 1900 > i64::from(i32::MAX) || year - 1900 < i64::from(i32::MIN) {
        return Err("[Errno 75] Value too large for defined data type".into());
    }
    if !(1..=9999).contains(&year) {
        return Err(format!("year {year} is out of range"));
    }
    Ok(())
}

/// `normalize_timestamp(ts, allow_backfill=)` at `now`: -1 when out of
/// range, Err when `datetime.fromtimestamp` raises.
pub fn normalize_timestamp(ts: i64, now: i64, allow_backfill: bool) -> Result<i64, String> {
    if ts <= 0 {
        return Ok(-1);
    }
    check_timestamp(ts)?;
    let parsed = ts - ts.rem_euclid(60);
    let now = now - now.rem_euclid(60);
    if parsed > now + 300 {
        return Ok(-1);
    }
    if !allow_backfill && parsed < now - 14 * DAY {
        return Ok(-1);
    }
    if parsed.rem_euclid(DAY) == 0 {
        return Ok(parsed + RESET_HOUR_SECS);
    }
    Ok(parsed)
}

/// `reset_boundaries_for_kind(kind, t_start, t_end)`: reset moments in `(t_start, t_end]`.
pub fn reset_boundaries(kind: &str, t_start: i64, t_end: i64) -> Vec<i64> {
    let weekly = match kind {
        "daily" => false,
        "weekly" => true,
        _ => return Vec::new(),
    };
    if t_end <= t_start {
        return Vec::new();
    }
    let mut cand = t_start.div_euclid(DAY) * DAY + RESET_HOUR_SECS;
    if cand <= t_start {
        cand += DAY;
    }
    let step = if weekly {
        // weekday(): 1970-01-01 was a Thursday (3); Monday is 0.
        while (cand.div_euclid(DAY) + 3).rem_euclid(7) != 0 {
            cand += DAY;
        }
        7 * DAY
    } else {
        DAY
    };
    let mut out = Vec::new();
    while cand <= t_end {
        out.push(cand);
        cand += step;
    }
    out
}

/// `_COLLAPSE_FLOOR`.
pub const COLLAPSE_FLOOR: i64 = 100;

/// `_completeness_verdict`'s reject reason.
#[derive(Clone, Debug, PartialEq)]
pub struct Verdict {
    pub reason: String,
    pub missing_boards: Vec<i64>,
    pub collapsed_boards: Vec<i64>,
}

/// `f"{x:.0%}"`.
pub fn percent0(x: f64) -> String {
    let v = x * 100.0;
    if v.is_nan() {
        return "nan%".into();
    }
    if v.is_infinite() {
        return if v > 0.0 { "inf%".into() } else { "-inf%".into() };
    }
    format!("{v:.0}%")
}

/// `_completeness_verdict(cur, prev, max_missing=, collapse_frac=)`.
pub fn completeness_verdict(
    cur: &HashMap<i64, i64>,
    prev: &HashMap<i64, i64>,
    max_missing: i64,
    collapse_frac: f64,
) -> Option<Verdict> {
    let mut missing: Vec<i64> = prev.keys().filter(|u| !cur.contains_key(u)).copied().collect();
    missing.sort_unstable();
    let mut collapsed = Vec::new();
    if collapse_frac > 0.0 {
        for (uuid, &pc) in prev {
            if let Some(&cc) = cur.get(uuid)
                && pc >= COLLAPSE_FLOOR
                && (cc as f64) < pc as f64 * collapse_frac
            {
                collapsed.push(*uuid);
            }
        }
        collapsed.sort_unstable();
    }
    let too_many = missing.len() as i64 > max_missing;
    if !too_many && collapsed.is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    if too_many {
        parts.push(format!("{} board(s) missing vs previous capture", missing.len()));
    }
    if !collapsed.is_empty() {
        parts.push(format!("{} board(s) collapsed below {} of previous", collapsed.len(), percent0(collapse_frac)));
    }
    Some(Verdict { reason: parts.join("; "), missing_boards: missing, collapsed_boards: collapsed })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_fallback_matches_fast_path() {
        let text = "x=1\nN$C$12 = Name$ Cat ##1;Bob;10|2;Al ice;9.5| |bad\r\n";
        let fast = parse_dump(text).unwrap();
        let slow = exact_lines(text).unwrap();
        assert_eq!(fast.len(), 1);
        assert_eq!(fast[0].category, "Cat");
        assert_eq!(slow[0].entries, fast[0].entries);
    }

    #[test]
    fn unicode_digits_and_contests() {
        let text =
            "A$Leaderboard_Category_Contests$0١ = N$C##١;x;1\nA$S$1 = N$Real##2;y;1_0\nF$S$2 = F$ favorıteſ ##1;z;1";
        let boards = parse_dump(text).unwrap();
        assert_eq!(boards.len(), 1);
        assert_eq!((boards[0].uuid, boards[0].category, boards[0].contest), (1, "Real", Some("weekly")));
        assert_eq!(boards[0].entries, vec![(2, "y", 10.0)]);
    }

    #[test]
    fn timestamps() {
        let now = 1_761_130_000;
        assert_eq!(normalize_timestamp(0, now, false), Ok(-1));
        assert_eq!(normalize_timestamp(now + 400, now, false), Ok(-1));
        assert_eq!(normalize_timestamp(1_761_091_200, now, false), Ok(1_761_091_200 + RESET_HOUR_SECS));
        assert_eq!(normalize_timestamp(now - 15 * DAY, now, false), Ok(-1));
        assert_eq!(normalize_timestamp(253_402_300_800, now, true), Err("year 10000 is out of range".into()));
        assert_eq!(trove_day_start(1_761_130_000), 1_761_130_800 - DAY);
    }

    #[test]
    fn boundaries_and_verdict() {
        let monday_11 = 1_760_958_000; // 2025-10-20 11:00 UTC
        assert_eq!(
            reset_boundaries("weekly", monday_11 - 10, monday_11 + 8 * DAY),
            vec![monday_11, monday_11 + 7 * DAY]
        );
        assert_eq!(reset_boundaries("daily", monday_11, monday_11 + DAY), vec![monday_11 + DAY]);
        assert!(reset_boundaries("default", 0, DAY * 9).is_empty());
        assert_eq!(percent0(0.125), "12%");
        assert_eq!(percent0(0.335), "34%");
        let prev: HashMap<i64, i64> = [(1, 500), (2, 10), (3, 200)].into();
        let cur: HashMap<i64, i64> = [(1, 100), (3, 200)].into();
        let v = completeness_verdict(&cur, &prev, 0, 0.5).unwrap();
        assert_eq!(v.reason, "1 board(s) missing vs previous capture; 1 board(s) collapsed below 50% of previous");
        assert_eq!((v.missing_boards, v.collapsed_boards), (vec![2], vec![1]));
        assert!(completeness_verdict(&cur, &prev, 1, 0.0).is_none());
    }
}
