//! The capture bot's LeaderBot.cfg dump, mirroring
//! `app/trove/leaderboards/parser.py`: board lines and their `rank;player;score`
//! entries. `None` from `parse` means "something here needs Python's exact
//! semantics" (Unicode digits or spaces, a score only Python reads), and the
//! caller parses the whole dump in Python instead.

/// One `name_id$category_id$uuid = name$category##entries` line.
pub struct Line<'a> {
    pub name_id: &'a str,
    pub category_id: &'a str,
    pub uuid: &'a str,
    pub name: &'a str,
    pub category: &'a str,
    pub entries: Vec<(i64, &'a str, f64)>,
}

/// Python `str.isspace` for one character.
fn py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// `[^<stop>\r\n]+` followed by `<stop>`: the field and what follows it.
fn field<'a>(s: &'a str, stop: &str) -> Option<(&'a str, &'a str)> {
    let end = s.find(stop)?;
    let f = &s[..end];
    if f.is_empty() || f.contains('\r') {
        return None;
    }
    Some((f, &s[end + stop.len()..]))
}

enum LineMatch<'a> {
    Board(&'a str, &'a str, &'a str, &'a str, &'a str, &'a str),
    NoMatch,
    Unsure,
}

/// `_BOARD_RE` on one line (no `\n` in it).
fn match_line(line: &str) -> LineMatch<'_> {
    let Some((name_id, rest)) = field(line, "$") else { return LineMatch::NoMatch };
    let Some((category_id, rest)) = field(rest, "$") else { return LineMatch::NoMatch };
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    let (uuid, rest) = rest.split_at(digits);
    if !rest.starts_with(" = ") {
        // A Unicode digit would still satisfy Python's `\d`.
        return if rest.chars().next().is_some_and(|c| !c.is_ascii()) { LineMatch::Unsure } else { LineMatch::NoMatch };
    }
    if uuid.is_empty() {
        return LineMatch::NoMatch;
    }
    let rest = &rest[3..];
    let Some((name, rest)) = field(rest, "$") else { return LineMatch::NoMatch };
    let Some(hash) = rest.find('#') else { return LineMatch::NoMatch };
    let category = &rest[..hash];
    if category.is_empty() || category.contains('\r') || !rest[hash..].starts_with("##") {
        return LineMatch::NoMatch;
    }
    let entries = &rest[hash + 2..];
    if entries.is_empty() {
        return LineMatch::NoMatch;
    }
    LineMatch::Board(name_id, category_id, uuid, name, category, entries)
}

/// `_parse_entries`; None when Python's answer can't be reproduced exactly.
fn parse_entries(raw: &str) -> Option<Vec<(i64, &str, f64)>> {
    let mut out = Vec::with_capacity(raw.len() / 24);
    for chunk in raw.split('|') {
        let chunk = chunk.trim_matches(py_space);
        if chunk.is_empty() {
            continue;
        }
        let mut parts = chunk.split(';');
        let (Some(rank_s), Some(player), Some(score_s), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        if rank_s.is_empty() || !rank_s.bytes().all(|b| b.is_ascii_digit()) {
            if rank_s.is_ascii() {
                continue;
            }
            return None;
        }
        let rank: i64 = rank_s.parse().ok()?;
        let score: f64 = match score_s.parse() {
            Ok(v) => v,
            Err(_) => return None,
        };
        out.push((rank, player, score));
    }
    Some(out)
}

/// Every board line in order, entries parsed.
pub fn parse(text: &str) -> Option<Vec<Line<'_>>> {
    let mut out = Vec::new();
    for line in text.split('\n') {
        match match_line(line) {
            LineMatch::NoMatch => {}
            LineMatch::Unsure => return None,
            LineMatch::Board(name_id, category_id, uuid, name, category, entries) => {
                out.push(Line { name_id, category_id, uuid, name, category, entries: parse_entries(entries)? });
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_board_line() {
        let text = "x=1\nN$C$12 = Name$Cat##1;Bob;10|2;Al ice;9.5| |bad\r\n";
        let lines = parse(text).unwrap();
        assert_eq!(lines.len(), 1);
        let l = &lines[0];
        assert_eq!((l.name_id, l.category_id, l.uuid, l.name, l.category), ("N", "C", "12", "Name", "Cat"));
        assert_eq!(l.entries, vec![(1, "Bob", 10.0), (2, "Al ice", 9.5)]);
    }

    #[test]
    fn unicode_digit_rank_falls_back() {
        assert!(parse("N$C$1 = A$B##١;x;1").is_none());
    }
}
