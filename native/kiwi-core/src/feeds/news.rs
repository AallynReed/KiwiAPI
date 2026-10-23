//! `app/trove/news.py`'s pure half: the RSS parse (ElementTree semantics),
//! `email.utils.parsedate_to_datetime`, `_strip_html` and `_truncate`.

use std::sync::LazyLock;

use regex::Regex;

use super::{py, unescape::unescape};
use crate::pystr;

pub const MAX_ITEMS: usize = 30;
const DC: &str = "http://purl.org/dc/elements/1.1/";
const MEDIA: &str = "http://search.yahoo.com/mrss/";

/// `[\s]` of a Python str pattern (`str.isspace`).
const PY_SPACE: &str =
    r"\t\n\x0B\x0C\r\x1C-\x1F \x{85}\x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}";

static BR: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!("(?i)<br[{PY_SPACE}]*/?>")).expect("br regex"));
static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new("<[^>]+>").expect("tag regex"));

/// `str.split()`.
pub fn split_ws(s: &str) -> Vec<&str> {
    s.split(pystr::is_space).filter(|p| !p.is_empty()).collect()
}

/// `_strip_html`.
pub fn strip_html(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    let stripped = BR.replace_all(value, " ");
    let stripped = TAG.replace_all(&stripped, " ");
    let text = unescape(&stripped);
    pystr::strip(&split_ws(&text).join(" ")).to_owned()
}

/// `_truncate(value, limit)`: cut at the last space, then `...`.
pub fn truncate(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_owned();
    }
    let head = pystr::prefix(value, limit);
    let kept = head.rsplit_once(' ').map_or(head, |(a, _)| a);
    format!("{}...", pystr::strip(kept))
}

/// A parsed date: wall-clock fields plus the offset in seconds (`None` is a
/// naive datetime: `-0000` or an unknown zone).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MailDate {
    pub year: i64,
    pub month: i64,
    pub day: i64,
    pub hour: i64,
    pub minute: i64,
    pub second: i64,
    pub offset: Option<i64>,
}

const MONTHS: [&str; 24] = [
    "jan",
    "feb",
    "mar",
    "apr",
    "may",
    "jun",
    "jul",
    "aug",
    "sep",
    "oct",
    "nov",
    "dec",
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
];
const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
const ZONES: [(&str, i128); 14] = [
    ("UT", 0),
    ("UTC", 0),
    ("GMT", 0),
    ("Z", 0),
    ("AST", -400),
    ("ADT", -300),
    ("EST", -500),
    ("EDT", -400),
    ("CST", -600),
    ("CDT", -500),
    ("MST", -700),
    ("MDT", -600),
    ("PST", -800),
    ("PDT", -700),
];

/// `str.isdigit()` for the first character of the year.
fn is_digit(c: char) -> bool {
    py::decimal(c).is_some() || matches!(c, '²' | '³' | '¹' | '\u{2070}'..='\u{2079}' | '\u{2080}'..='\u{2089}')
}

fn strip_last(s: &str, c: char) -> &str {
    s.strip_suffix(c).unwrap_or(s)
}

/// Year, month, day, hour, minute, second and the offset in seconds.
type RawDate = (i128, usize, i128, i128, i128, i128, Option<i128>);

/// `email._parseaddr._parsedate_tz` (the fields, before `datetime()`).
fn parsedate_tz(data: &str) -> Option<RawDate> {
    let owned: Vec<String> = split_ws(data).into_iter().map(str::to_owned).collect();
    let mut data = owned;
    if data.is_empty() {
        return None;
    }
    if data[0].ends_with(',') || DAYS.contains(&data[0].to_lowercase().as_str()) {
        data.remove(0);
    } else if let Some(i) = data[0].rfind(',') {
        data[0] = data[0][i + 1..].to_owned();
    }
    if data.len() == 3 {
        let stuff: Vec<String> = data[0].split('-').map(str::to_owned).collect();
        if stuff.len() == 3 {
            data = stuff.into_iter().chain(data[1..].iter().cloned()).collect();
        }
    }
    if data.len() == 4 {
        let s = data[3].clone();
        let i = s.find('+').or_else(|| s.find('-'));
        match i {
            Some(i) if i > 0 => {
                data.truncate(3);
                data.push(s[..i].to_owned());
                data.push(s[i..].to_owned());
            }
            _ => data.push(String::new()),
        }
    }
    if data.len() < 5 {
        return None;
    }
    data.truncate(5);
    let [dd, mm, yy, tm, tz]: [String; 5] = data.try_into().ok()?;
    let (mut dd, mut mm, mut yy, mut tm, mut tz) = (dd, mm, yy, tm, tz);
    if dd.is_empty() || mm.is_empty() || yy.is_empty() {
        return None;
    }
    mm = mm.to_lowercase();
    if !MONTHS.contains(&mm.as_str()) {
        (dd, mm) = (mm, dd.to_lowercase());
        if !MONTHS.contains(&mm.as_str()) {
            return None;
        }
    }
    let mut month = MONTHS.iter().position(|m| *m == mm)? + 1;
    if month > 12 {
        month -= 12;
    }
    dd = strip_last(&dd, ',').to_owned();
    if yy.find(':').is_some_and(|i| i > 0) {
        (yy, tm) = (tm, yy);
    }
    if yy.ends_with(',') {
        yy.pop();
        if yy.is_empty() {
            return None;
        }
    }
    if !yy.chars().next().is_some_and(is_digit) {
        (yy, tz) = (tz, yy);
    }
    if tm.ends_with(',') {
        tm.pop();
    }
    let parts: Vec<&str> = tm.split(':').collect();
    let (thh, tmm, tss) = match parts.as_slice() {
        [h, m] => (*h, *m, "0"),
        [h, m, s] => (*h, *m, *s),
        [one] if one.contains('.') => match one.split('.').collect::<Vec<_>>().as_slice() {
            [h, m] => (*h, *m, "0"),
            [h, m, s] => (*h, *m, *s),
            _ => return None,
        },
        _ => return None,
    };
    let int = |s: &str| py::int(s, 10);
    let (mut year, day, hour, minute, second) = (int(&yy)?, int(&dd)?, int(thh)?, int(tmm)?, int(tss)?);
    if year < 100 {
        year += if year > 68 { 1900 } else { 2000 };
    }
    let upper = tz.to_uppercase();
    let mut offset = match ZONES.iter().find(|(z, _)| *z == upper) {
        Some((_, o)) => Some(*o),
        None => {
            let o = int(&tz);
            if o == Some(0) && tz.starts_with('-') { None } else { o }
        }
    };
    if let Some(o) = offset.filter(|o| *o != 0) {
        let (sign, o) = if o < 0 { (-1, o.saturating_neg()) } else { (1, o) };
        offset = Some(sign * (o / 100).saturating_mul(3600).saturating_add((o % 100) * 60));
    }
    Some((year, month, day, hour, minute, second, offset))
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// `parsedate_to_datetime(data)`; `None` where it raises ValueError.
pub fn parsedate_to_datetime(data: &str) -> Option<MailDate> {
    let (year, month, day, hour, minute, second, offset) = parsedate_tz(data)?;
    let small = |v: i128| i64::try_from(v).ok().filter(|v| i32::try_from(*v).is_ok());
    let (year, day, hour, minute, second) = (small(year)?, small(day)?, small(hour)?, small(minute)?, small(second)?);
    let month = month as i64;
    let valid = (1..=9999).contains(&year)
        && (1..=days_in_month(year, month)).contains(&day)
        && (0..24).contains(&hour)
        && (0..60).contains(&minute)
        && (0..60).contains(&second);
    if !valid {
        return None;
    }
    let offset = match offset {
        None => None,
        Some(o) if o.abs() < 86_400 => Some(o as i64),
        Some(_) => return None,
    };
    Some(MailDate { year, month, day, hour, minute, second, offset })
}

impl MailDate {
    /// `.astimezone(UTC)` as Unix seconds (a naive value is read as UTC, the
    /// container's zone); `Err` is the OverflowError past year 1..9999.
    pub fn utc_timestamp(&self) -> Result<i64, &'static str> {
        let days = crate::rotations::days_from_civil(self.year, self.month, self.day);
        let wall = days * 86_400 + self.hour * 3600 + self.minute * 60 + self.second;
        let ts = wall - self.offset.unwrap_or(0);
        const MIN: i64 = -62_135_596_800;
        const MAX: i64 = 253_402_300_799;
        if (MIN..=MAX).contains(&ts) { Ok(ts) } else { Err("OverflowError: date value out of range") }
    }
}

/// One `parse_feed` item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewsItem {
    pub url: String,
    pub title: String,
    pub author: String,
    pub summary: String,
    pub category: String,
    pub categories: Vec<String>,
    pub image: Option<String>,
    /// Unix seconds, UTC.
    pub published_at: Option<i64>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum FeedError {
    /// `ET.ParseError`.
    Xml(String),
    /// An exception `parse_feed` lets escape.
    Raised(&'static str),
}

fn is_tag(n: &roxmltree::Node<'_, '_>, ns: Option<&str>, name: &str) -> bool {
    n.is_element() && n.tag_name().name() == name && n.tag_name().namespace() == ns
}

fn child<'a, 'i>(n: roxmltree::Node<'a, 'i>, ns: Option<&str>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    n.children().find(|c| is_tag(c, ns, name))
}

/// `elem.text`: the character data before the first child element
/// (comments and processing instructions are dropped, their neighbours join).
fn text(n: roxmltree::Node<'_, '_>) -> String {
    let mut out = String::new();
    for c in n.children() {
        if c.is_element() {
            break;
        }
        if c.is_text() {
            out.push_str(c.text().unwrap_or(""));
        }
    }
    out
}

/// `item.findtext(tag) or ""`.
fn findtext(n: roxmltree::Node<'_, '_>, ns: Option<&str>, name: &str) -> String {
    child(n, ns, name).map(text).unwrap_or_default()
}

/// `parse_feed(xml_text)`.
pub fn parse_feed(xml: &str) -> Result<Vec<NewsItem>, FeedError> {
    let opt = roxmltree::ParsingOptions { allow_dtd: true, ..roxmltree::ParsingOptions::default() };
    let doc = roxmltree::Document::parse_with_options(xml, opt).map_err(|e| FeedError::Xml(e.to_string()))?;
    let root = doc.root_element();
    let mut items = Vec::new();
    for channel in root.children().filter(|c| is_tag(c, None, "channel")) {
        for item in channel.children().filter(|c| is_tag(c, None, "item")) {
            let link = pystr::strip(&findtext(item, None, "link")).to_owned();
            if link.is_empty() {
                continue;
            }
            let title = unescape(pystr::strip(&findtext(item, None, "title"))).into_owned();
            let author = pystr::strip(&findtext(item, Some(DC), "creator")).to_owned();
            let description = findtext(item, None, "description");
            let categories: Vec<String> = item
                .children()
                .filter(|c| is_tag(c, None, "category"))
                .map(text)
                .filter(|t| !pystr::strip(t).is_empty())
                .map(|t| unescape(pystr::strip(&t)).into_owned())
                .collect();
            let published_at = match parsedate_to_datetime(pystr::strip(&findtext(item, None, "pubDate"))) {
                Some(d) => Some(d.utc_timestamp().map_err(FeedError::Raised)?),
                None => None,
            };
            let image = ["content", "thumbnail"].iter().find_map(|tag| {
                let el = child(item, Some(MEDIA), tag)?;
                el.attribute("url").filter(|u| !u.is_empty()).map(str::to_owned)
            });
            items.push(NewsItem {
                url: link,
                title,
                author: if author.is_empty() { "Team Trove".into() } else { author },
                summary: truncate(&strip_html(&unescape(&description)), 220),
                category: categories.first().cloned().unwrap_or_else(|| "News".into()),
                categories,
                image,
                published_at,
            });
            if items.len() >= MAX_ITEMS {
                return Ok(items);
            }
        }
    }
    Ok(items)
}
