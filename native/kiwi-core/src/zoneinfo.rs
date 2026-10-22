//! Python's `zoneinfo.ZoneInfo` as the C module (`Modules/_zoneinfo.c`, 3.13)
//! computes it: TZif v1-v4 parsing, the POSIX TZ footer, `utcoffset()` with
//! `fold` and `fromutc()`. Only offsets are kept; abbreviations and `dst()`
//! are not needed by the API.

/// One `Mm.w.d` or `[J]n` rule plus its local transition time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TransitionRule {
    Calendar { month: i64, week: i64, day: i64, secs: i64 },
    Day { julian: bool, day: i64, secs: i64 },
}

impl TransitionRule {
    fn year_to_timestamp(self, year: i64) -> i64 {
        use crate::pydt::{EPOCH_ORD, days_in_month, is_leap, ymd_to_ord};
        match self {
            TransitionRule::Calendar { month, week, day, secs } => {
                let first_day = (ymd_to_ord(year, month, 1) + 6) % 7;
                let mut month_day = (day - (first_day + 1)) % 7;
                if month_day < 0 {
                    month_day += 7;
                }
                month_day += 1 + (week - 1) * 7;
                if month_day > days_in_month(year, month) {
                    month_day -= 7;
                }
                (ymd_to_ord(year, month, month_day) - EPOCH_ORD) * 86_400 + secs
            }
            TransitionRule::Day { julian, day, secs } => {
                let before = ymd_to_ord(year, 1, 1) - EPOCH_ORD - 1;
                let day = if julian && day >= 59 && is_leap(year) { day + 1 } else { day };
                (before + day) * 86_400 + secs
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DstRule {
    dst_off: i64,
    dst_diff: i64,
    start: TransitionRule,
    end: TransitionRule,
}

/// `_tzrule`: the offsets after the last explicit transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TzRule {
    std_off: i64,
    dst: Option<DstRule>,
}

impl TzRule {
    fn transitions(r: &DstRule, year: i64) -> (i64, i64) {
        (r.start.year_to_timestamp(year), r.end.year_to_timestamp(year))
    }

    fn ttinfo(&self, ts: i64, fold: bool, year: i64) -> i64 {
        let Some(r) = &self.dst else { return self.std_off };
        let (mut start, mut end) = Self::transitions(r, year);
        if fold == (r.dst_diff >= 0) {
            end -= r.dst_diff;
        } else {
            start += r.dst_diff;
        }
        let isdst = if start < end { ts >= start && ts < end } else { ts < end || ts >= start };
        if isdst { r.dst_off } else { self.std_off }
    }

    fn ttinfo_fromutc(&self, ts: i64, year: i64) -> (i64, bool) {
        let Some(r) = &self.dst else { return (self.std_off, false) };
        let (mut start, mut end) = Self::transitions(r, year);
        start -= self.std_off;
        end -= r.dst_off;
        let isdst = if start < end { ts >= start && ts < end } else { ts < end || ts >= start };
        let (amb_start, amb_end) = if r.dst_diff > 0 { (end, end + r.dst_diff) } else { (start, start - r.dst_diff) };
        let fold = ts >= amb_start && ts < amb_end;
        (if isdst { r.dst_off } else { self.std_off }, fold)
    }
}

/// A loaded zone: what `ZoneInfo(key)` keeps for offset lookups.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Zone {
    trans_utc: Vec<i64>,
    trans_wall: [Vec<i64>; 2],
    trans_off: Vec<i64>,
    before: i64,
    after: TzRule,
}

fn bisect_right(arr: &[i64], value: i64) -> usize {
    arr.partition_point(|&x| x <= value)
}

impl Zone {
    /// `utcoffset()` in seconds for a local wall time (`local_ts` as in
    /// `get_local_timestamp`, `year` the datetime's year).
    pub fn utcoffset(&self, local_ts: i64, year: i64, fold: bool) -> i64 {
        let wall = &self.trans_wall[usize::from(fold)];
        let n = self.trans_utc.len();
        if n > 0 && local_ts < wall[0] {
            self.before
        } else if n == 0 || local_ts > wall[n - 1] {
            self.after.ttinfo(local_ts, fold, year)
        } else {
            self.trans_off[bisect_right(wall, local_ts) - 1]
        }
    }

    /// `fromutc()`: the offset to add to a UTC time and the resulting `fold`.
    pub fn fromutc(&self, utc_ts: i64, utc_year: i64) -> (i64, bool) {
        let n = self.trans_utc.len();
        if n >= 1 && utc_ts < self.trans_utc[0] {
            return (self.before, false);
        }
        if n == 0 || utc_ts > self.trans_utc[n - 1] {
            let (off, mut fold) = self.after.ttinfo_fromutc(utc_ts, utc_year);
            if n > 0 {
                let prev = if n == 1 { self.before } else { self.trans_off[n - 2] };
                let diff = prev - off;
                if diff > 0 && utc_ts < self.trans_utc[n - 1] + diff {
                    fold = true;
                }
            }
            return (off, fold);
        }
        let idx = bisect_right(&self.trans_utc, utc_ts);
        let (prev, off) = if idx >= 2 {
            (self.trans_off[idx - 2], self.trans_off[idx - 1])
        } else {
            (self.before, self.trans_off[0])
        };
        (off, prev - off > utc_ts - self.trans_utc[idx - 1])
    }
}

/// Why a file is not a usable zone (Python raises `ValueError` and the like).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidZone(pub &'static str);

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], InvalidZone> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.b.len()).ok_or(InvalidZone("truncated TZif"))?;
        let s = &self.b[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn i32(&mut self) -> Result<i64, InvalidZone> {
        let s = self.take(4)?;
        Ok(i64::from(i32::from_be_bytes([s[0], s[1], s[2], s[3]])))
    }

    fn header(&mut self) -> Result<(u32, [usize; 6]), InvalidZone> {
        if self.take(4)? != b"TZif" {
            return Err(InvalidZone("Invalid TZif file: magic not found"));
        }
        let v = self.take(1)?[0];
        let version = match v {
            0 => 1,
            b'0'..=b'9' => u32::from(v - b'0'),
            _ => return Err(InvalidZone("bad TZif version")),
        };
        self.take(15)?;
        let mut counts = [0usize; 6];
        for c in &mut counts {
            *c = usize::try_from(self.i32()?).map_err(|_| InvalidZone("negative TZif count"))?;
        }
        Ok((version, counts))
    }
}

/// Parses a TZif file the way `ZoneInfo` loads it.
pub fn load(bytes: &[u8]) -> Result<Zone, InvalidZone> {
    let mut r = Reader { b: bytes, pos: 0 };
    let (version, mut c) = r.header()?;
    let mut time_size = 4;
    if version != 1 {
        time_size = 8;
        let [isut, isstd, leap, timecnt, typecnt, charcnt] = c;
        r.take(timecnt * 5 + typecnt * 6 + charcnt + leap * 8 + isstd + isut)?;
        c = r.header()?.1;
    }
    let [isut, isstd, leap, timecnt, typecnt, charcnt] = c;
    let mut trans_utc = Vec::with_capacity(timecnt);
    for _ in 0..timecnt {
        let s = r.take(time_size)?;
        trans_utc.push(if time_size == 4 {
            i64::from(i32::from_be_bytes([s[0], s[1], s[2], s[3]]))
        } else {
            i64::from_be_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]])
        });
    }
    let trans_idx: Vec<usize> = r.take(timecnt)?.iter().map(|&i| usize::from(i)).collect();
    let mut utcoff = Vec::with_capacity(typecnt);
    let mut isdst = Vec::with_capacity(typecnt);
    for _ in 0..typecnt {
        utcoff.push(r.i32()?);
        let rest = r.take(2)?;
        isdst.push(rest[0] != 0);
    }
    r.take(charcnt)?;
    let tz_str: Option<Vec<u8>> = if version >= 2 {
        r.take(isut + isstd + leap * 12)?;
        if r.take(1)? != b"\n" {
            return Err(InvalidZone("missing TZ string newline"));
        }
        let rest = &bytes[r.pos..];
        let nl =
            rest.iter().position(|&b| b == b'\n').ok_or(InvalidZone("Invalid TZif file: unexpected end of file"))?;
        Some(rest[..nl].to_vec())
    } else {
        None
    };

    if trans_idx.iter().any(|&i| i >= typecnt) {
        return Err(InvalidZone("Invalid transition index found while reading TZif"));
    }
    let trans_off: Vec<i64> = trans_idx.iter().map(|&i| utcoff[i]).collect();
    let trans_wall = ts_to_local(&trans_idx, &trans_utc, &utcoff);
    let before = isdst.iter().position(|d| !d).or(if typecnt > 0 { Some(0) } else { None }).map(|i| utcoff[i]);
    let after = match tz_str.as_deref() {
        Some(s) if !s.is_empty() => parse_tz_str(s)?,
        _ => {
            if typecnt == 0 {
                return Err(InvalidZone("No time zone information found."));
            }
            let idx = trans_idx.last().copied().unwrap_or(typecnt - 1);
            TzRule { std_off: utcoff[idx], dst: None }
        }
    };
    Ok(Zone { trans_utc, trans_wall, trans_off, before: before.unwrap_or(0), after })
}

fn ts_to_local(trans_idx: &[usize], trans_utc: &[i64], utcoff: &[i64]) -> [Vec<i64>; 2] {
    if trans_utc.is_empty() {
        return [Vec::new(), Vec::new()];
    }
    let mut wall = [trans_utc.to_vec(), trans_utc.to_vec()];
    let (mut o0, mut o1) = if utcoff.len() > 1 { (utcoff[0], utcoff[trans_idx[0]]) } else { (utcoff[0], utcoff[0]) };
    if o1 > o0 {
        std::mem::swap(&mut o0, &mut o1);
    }
    wall[0][0] += o0;
    wall[1][0] += o1;
    for i in 1..trans_idx.len() {
        let (mut o0, mut o1) = (utcoff[trans_idx[i - 1]], utcoff[trans_idx[i]]);
        if o1 > o0 {
            std::mem::swap(&mut o0, &mut o1);
        }
        wall[0][i] += o0;
        wall[1][i] += o1;
    }
    wall
}

// --- POSIX TZ strings ------------------------------------------------------

struct Tz<'a> {
    s: &'a [u8],
    p: usize,
}

const BAD: InvalidZone = InvalidZone("invalid TZ string");

impl Tz<'_> {
    fn at(&self) -> u8 {
        self.s.get(self.p).copied().unwrap_or(0)
    }

    fn digits(&mut self, min: usize, max: usize) -> Result<i64, InvalidZone> {
        let mut v = 0;
        for i in 0..max {
            let c = self.at();
            if !c.is_ascii_digit() {
                return if i < min { Err(BAD) } else { Ok(v) };
            }
            v = v * 10 + i64::from(c - b'0');
            self.p += 1;
        }
        Ok(v)
    }

    fn abbr(&mut self) -> Result<(), InvalidZone> {
        if self.at() == b'<' {
            self.p += 1;
            let start = self.p;
            while self.at() != b'>' {
                let c = self.at();
                if !c.is_ascii_alphanumeric() && c != b'+' && c != b'-' {
                    return Err(BAD);
                }
                self.p += 1;
            }
            if self.p == start {
                return Err(BAD);
            }
            self.p += 1;
        } else {
            let start = self.p;
            while self.at().is_ascii_alphabetic() {
                self.p += 1;
            }
            if self.p == start {
                return Err(BAD);
            }
        }
        Ok(())
    }

    fn time(&mut self) -> Result<(i64, i64, i64), InvalidZone> {
        let mut sign = 1;
        if matches!(self.at(), b'-' | b'+') {
            if self.at() == b'-' {
                sign = -1;
            }
            self.p += 1;
        }
        let h = self.digits(1, 3)? * sign;
        let (mut m, mut s) = (0, 0);
        if self.at() == b':' {
            self.p += 1;
            m = self.digits(2, 2)? * sign;
            if self.at() == b':' {
                self.p += 1;
                s = self.digits(2, 2)? * sign;
            }
        }
        Ok((h, m, s))
    }

    fn delta(&mut self) -> Result<i64, InvalidZone> {
        let (h, m, s) = self.time()?;
        if !(-24..=24).contains(&h) {
            return Err(BAD);
        }
        Ok(-(h * 3600 + m * 60 + s))
    }

    fn rule(&mut self) -> Result<TransitionRule, InvalidZone> {
        let (mut h, mut m, mut s) = (2, 0, 0);
        if self.at() == b'M' {
            self.p += 1;
            let month = self.digits(1, 2)?;
            if self.at() != b'.' {
                return Err(BAD);
            }
            self.p += 1;
            let week = self.digits(1, 1)?;
            if self.at() != b'.' {
                return Err(BAD);
            }
            self.p += 1;
            let day = self.digits(1, 1)?;
            if self.at() == b'/' {
                self.p += 1;
                (h, m, s) = self.time()?;
            }
            if !(1..=12).contains(&month) || !(1..=5).contains(&week) || !(0..=6).contains(&day) {
                return Err(BAD);
            }
            if !(-167..=167).contains(&h) {
                return Err(BAD);
            }
            return Ok(TransitionRule::Calendar { month, week, day, secs: h * 3600 + m * 60 + s });
        }
        let julian = self.at() == b'J';
        if julian {
            self.p += 1;
        }
        let day = self.digits(1, 3)?;
        if self.at() == b'/' {
            self.p += 1;
            (h, m, s) = self.time()?;
        }
        if day < i64::from(julian) || day > 365 || !(-167..=167).contains(&h) {
            return Err(BAD);
        }
        Ok(TransitionRule::Day { julian, day, secs: h * 3600 + m * 60 + s })
    }
}

fn parse_tz_str(s: &[u8]) -> Result<TzRule, InvalidZone> {
    let s = &s[..s.iter().position(|&b| b == 0).unwrap_or(s.len())];
    let mut t = Tz { s, p: 0 };
    t.abbr()?;
    let std_off = t.delta()?;
    if t.at() == 0 {
        return Ok(TzRule { std_off, dst: None });
    }
    t.abbr()?;
    let dst_off = if t.at() == b',' { std_off + 3600 } else { t.delta()? };
    let mut rules = [None, None];
    for slot in &mut rules {
        if t.at() != b',' {
            return Err(BAD);
        }
        t.p += 1;
        *slot = Some(t.rule()?);
    }
    if t.at() != 0 {
        return Err(BAD);
    }
    let [Some(start), Some(end)] = rules else { return Err(BAD) };
    Ok(TzRule { std_off, dst: Some(DstRule { dst_off, dst_diff: dst_off - std_off, start, end }) })
}

/// `_tzpath._validate_tzfile_path` on POSIX, plus the NUL check `os.stat`
/// makes: a key is usable only as a normalized relative path.
pub fn valid_key(key: &str) -> bool {
    !key.is_empty() && !key.contains('\0') && key.split('/').all(|c| !matches!(c, "" | "." | ".."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tz_strings() {
        let r = parse_tz_str(b"EST5EDT,M3.2.0,M11.1.0").unwrap();
        assert_eq!(r.std_off, -18_000);
        assert_eq!(r.dst.unwrap().dst_off, -14_400);
        assert_eq!(parse_tz_str(b"<-03>3").unwrap().std_off, -10_800);
        assert_eq!(parse_tz_str(b"<+1030>-10:30<+11>-11,M10.1.0,M4.1.0").unwrap().dst.unwrap().dst_diff, 1800);
        assert!(parse_tz_str(b"EST5EDT").is_err());
        assert!(parse_tz_str(b"EST25").is_err());
    }

    #[test]
    fn keys() {
        assert!(valid_key("America/New_York"));
        assert!(valid_key("America.Argentina/x"));
        for bad in ["", "/etc/x", "a//b", "a/./b", "../x", "a/", ".", ".."] {
            assert!(!valid_key(bad), "{bad}");
        }
    }
}
