//! CPython's `datetime` arithmetic and `datetime.fromisoformat` (the C
//! implementation of 3.13, `Modules/_datetimemodule.c`), byte for byte,
//! including its error messages.

use crate::pystr;

pub const MINYEAR: i64 = 1;
pub const MAXYEAR: i64 = 9999;
/// `date(1970, 1, 1).toordinal()`.
pub const EPOCH_ORD: i64 = 719_163;

const DAYS_IN_MONTH: [i64; 13] = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
const DAYS_BEFORE_MONTH: [i64; 13] = [0, 0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
const MONTH_ABBR: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

pub fn is_leap(y: i64) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

pub fn days_in_month(y: i64, m: i64) -> i64 {
    if m == 2 && is_leap(y) { 29 } else { DAYS_IN_MONTH[m as usize] }
}

/// `ymd_to_ord`: day 1 is 0001-01-01.
pub fn ymd_to_ord(y: i64, m: i64, d: i64) -> i64 {
    let y1 = y - 1;
    let mut before = y1 * 365 + y1.div_euclid(4) - y1.div_euclid(100) + y1.div_euclid(400);
    before += DAYS_BEFORE_MONTH[m as usize] + i64::from(m > 2 && is_leap(y));
    before + d
}

/// Proleptic Gregorian date of a day count since 1970-01-01, any range.
pub fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

pub fn ord_to_ymd(ord: i64) -> (i64, i64, i64) {
    civil_from_days(ord - EPOCH_ORD)
}

/// A wall clock time as seconds since 1970-01-01T00:00 on the same clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wall {
    pub year: i64,
    pub month: i64,
    pub day: i64,
    pub hour: i64,
    pub minute: i64,
    pub second: i64,
}

impl Wall {
    pub fn from_secs(secs: i64) -> Wall {
        let (year, month, day) = civil_from_days(secs.div_euclid(86_400));
        let s = secs.rem_euclid(86_400);
        Wall { year, month, day, hour: s / 3600, minute: s / 60 % 60, second: s % 60 }
    }

    pub fn secs(&self) -> i64 {
        (ymd_to_ord(self.year, self.month, self.day) - EPOCH_ORD) * 86_400
            + self.hour * 3600
            + self.minute * 60
            + self.second
    }

    pub fn in_range(&self) -> bool {
        (MINYEAR..=MAXYEAR).contains(&self.year)
    }

    /// `isoformat()` of this wall time with a whole-second UTC offset.
    pub fn isoformat(&self, offset: i64) -> String {
        let mut out = format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        );
        out.push_str(&format_offset(offset * 1_000_000));
        out
    }

    /// glibc `strftime("%b %d, %Y")` in the C locale (`%Y` is not padded).
    pub fn strftime_date(&self) -> String {
        format!("{} {:02}, {}", MONTH_ABBR[(self.month - 1) as usize], self.day, self.year)
    }

    /// `strftime("%H:%M:%S")`.
    pub fn strftime_time(&self) -> String {
        format!("{:02}:{:02}:{:02}", self.hour, self.minute, self.second)
    }
}

/// The `+HH:MM[:SS[.ffffff]]` suffix `isoformat()` gives a UTC offset.
pub fn format_offset(us: i64) -> String {
    let sign = if us < 0 { '-' } else { '+' };
    let a = us.unsigned_abs();
    let (secs, frac) = (a / 1_000_000, a % 1_000_000);
    let mut out = format!("{sign}{:02}:{:02}", secs / 3600, secs / 60 % 60);
    if secs % 60 != 0 || frac != 0 {
        out.push_str(&format!(":{:02}", secs % 60));
        if frac != 0 {
            out.push_str(&format!(".{frac:06}"));
        }
    }
    out
}

/// `repr(timedelta(microseconds=us))`.
pub fn timedelta_repr(us: i64) -> String {
    let days = us.div_euclid(86_400_000_000);
    let rest = us.rem_euclid(86_400_000_000);
    let (secs, micros) = (rest / 1_000_000, rest % 1_000_000);
    let mut args = Vec::new();
    if days != 0 {
        args.push(format!("days={days}"));
    }
    if secs != 0 {
        args.push(format!("seconds={secs}"));
    }
    if micros != 0 {
        args.push(format!("microseconds={micros}"));
    }
    if args.is_empty() {
        args.push("0".into());
    }
    format!("datetime.timedelta({})", args.join(", "))
}

/// What `fromisoformat` produced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IsoDateTime {
    pub year: i64,
    pub month: i64,
    pub day: i64,
    pub hour: i64,
    pub minute: i64,
    pub second: i64,
    pub microsecond: i64,
    /// UTC offset in microseconds; `None` for a naive datetime.
    pub offset_us: Option<i64>,
}

impl IsoDateTime {
    /// Microseconds since 1970-01-01T00:00 on the datetime's own clock.
    pub fn wall_us(&self) -> i128 {
        let days = ymd_to_ord(self.year, self.month, self.day) - EPOCH_ORD;
        let secs = days * 86_400 + self.hour * 3600 + self.minute * 60 + self.second;
        i128::from(secs) * 1_000_000 + i128::from(self.microsecond)
    }
}

/// `int(timedelta(microseconds=us).total_seconds())`: the quotient is
/// rounded to a double first, then truncated toward zero.
pub fn total_seconds_int(us: i128) -> i64 {
    let a = us.unsigned_abs();
    let text = format!("{}{}.{:06}", if us < 0 { "-" } else { "" }, a / 1_000_000, a % 1_000_000);
    text.parse::<f64>().map(|f| f as i64).unwrap_or(0)
}

// --- fromisoformat ---------------------------------------------------------

/// NUL-terminated byte access, as the C parser reads past its spans.
struct Buf<'a>(&'a [u8]);

impl Buf<'_> {
    fn at(&self, i: usize) -> u8 {
        self.0.get(i).copied().unwrap_or(0)
    }

    fn digits(&self, p: usize, var: &mut i64, n: usize) -> Option<usize> {
        for k in 0..n {
            let c = self.at(p + k);
            if !c.is_ascii_digit() {
                return None;
            }
            *var = *var * 10 + i64::from(c - b'0');
        }
        Some(p + n)
    }
}

fn weekday(y: i64, m: i64, d: i64) -> i64 {
    (ymd_to_ord(y, m, d) + 6) % 7
}

fn iso_week1_monday(year: i64) -> i64 {
    let first_day = ymd_to_ord(year, 1, 1);
    let first_weekday = (first_day + 6) % 7;
    let mut monday = first_day - first_weekday;
    if first_weekday > 3 {
        monday += 7;
    }
    monday
}

fn iso_to_ymd(iso_year: i64, week: i64, day: i64) -> Result<(i64, i64, i64), i32> {
    if !(MINYEAR..=MAXYEAR).contains(&iso_year) {
        return Err(-4);
    }
    if week <= 0 || week >= 53 {
        let mut out_of_range = true;
        if week == 53 {
            let first = weekday(iso_year, 1, 1);
            if first == 3 || (first == 2 && is_leap(iso_year)) {
                out_of_range = false;
            }
        }
        if out_of_range {
            return Err(-2);
        }
    }
    if day <= 0 || day >= 8 {
        return Err(-3);
    }
    Ok(ord_to_ymd(iso_week1_monday(iso_year) + (week - 1) * 7 + day - 1))
}

fn parse_date(b: &Buf<'_>, len: usize) -> Result<(i64, i64, i64), i32> {
    let mut year = 0;
    let mut p = b.digits(0, &mut year, 4).ok_or(-1)?;
    let sep = b.at(p) == b'-';
    if sep {
        p += 1;
    }
    if b.at(p) == b'W' {
        p += 1;
        let (mut week, mut day) = (0, 0);
        p = b.digits(p, &mut week, 2).ok_or(-3)?;
        if p < len {
            if sep {
                let c = b.at(p);
                p += 1;
                if c != b'-' {
                    return Err(-2);
                }
            }
            b.digits(p, &mut day, 1).ok_or(-4)?;
        } else {
            day = 1;
        }
        return iso_to_ymd(year, week, day).map_err(|rv| -3 + rv);
    }
    let (mut month, mut day) = (0, 0);
    p = b.digits(p, &mut month, 2).ok_or(-1)?;
    if sep {
        let c = b.at(p);
        p += 1;
        if c != b'-' {
            return Err(-2);
        }
    }
    b.digits(p, &mut day, 2).ok_or(-1)?;
    Ok((year, month, day))
}

/// `parse_hh_mm_ss_ff` over `[p, end)`: `Ok(true)` when text remains.
fn parse_hms(b: &Buf<'_>, mut p: usize, end: usize, vals: &mut [i64; 4]) -> Result<bool, i32> {
    *vals = [0; 4];
    let mut has_sep = true;
    for (i, val) in vals.iter_mut().take(3).enumerate() {
        p = b.digits(p, val, 2).ok_or(-3)?;
        let c = b.at(p);
        p += 1;
        if i == 0 {
            has_sep = c == b':';
        }
        if c == b'.' || c == b',' {
            if p >= end {
                return Err(-3);
            }
            break;
        } else if p >= end {
            return Ok(c != 0);
        } else if has_sep && c == b':' {
            continue;
        } else if !has_sep {
            p -= 1;
        } else {
            return Err(-4);
        }
    }
    let to_parse = end.saturating_sub(p).min(6);
    p = b.digits(p, &mut vals[3], to_parse).ok_or(-3)?;
    if (1..6).contains(&to_parse) {
        vals[3] *= [100_000, 10_000, 1000, 100, 10][to_parse - 1];
    }
    while b.at(p).is_ascii_digit() {
        p += 1;
    }
    Ok(b.at(p) != 0)
}

/// `[hour, minute, second, microsecond]` and the offset as (seconds, microseconds).
type TimeParts = ([i64; 4], Option<(i64, i64)>);

/// `parse_isoformat_time`, error codes as in C.
fn parse_time(b: &Buf<'_>, start: usize, len: usize) -> Result<TimeParts, i32> {
    let end = start + len;
    let mut tz = start;
    loop {
        if matches!(b.at(tz), b'Z' | b'+' | b'-') {
            break;
        }
        tz += 1;
        if tz >= end {
            break;
        }
    }
    let mut time = [0; 4];
    let rest = parse_hms(b, start, tz, &mut time)?;
    if tz == end {
        return if rest { Err(-5) } else { Ok((time, None)) };
    }
    if b.at(tz) == b'Z' {
        return if b.at(tz + 1) != 0 { Err(-5) } else { Ok((time, Some((0, 0)))) };
    }
    let sign = if b.at(tz) == b'-' { -1 } else { 1 };
    let mut off = [0; 4];
    let rest = parse_hms(b, tz + 1, end, &mut off).map_err(|_| -5)?;
    if rest {
        return Err(-5);
    }
    Ok((time, Some((sign * (off[0] * 3600 + off[1] * 60 + off[2]), sign * off[3]))))
}

fn find_separator(b: &[u8]) -> isize {
    let len = b.len();
    if len == 7 {
        return 7;
    }
    if b[4] == b'-' {
        if b[5] == b'W' {
            if len < 8 {
                return -1;
            }
            if len > 8 && b[8] == b'-' {
                if len == 9 {
                    return -1;
                }
                if len > 10 && b[10].is_ascii_digit() {
                    return 8;
                }
                return 10;
            }
            return 8;
        }
        return 10;
    }
    if b[4] == b'W' {
        let mut idx = 7;
        while idx < len && b[idx].is_ascii_digit() {
            idx += 1;
        }
        if idx < 9 {
            return idx as isize;
        }
        return if idx % 2 == 0 { 7 } else { 8 };
    }
    8
}

fn check_date(y: i64, m: i64, d: i64) -> Result<(), String> {
    if !(MINYEAR..=MAXYEAR).contains(&y) {
        return Err(format!("year {y} is out of range"));
    }
    if !(1..=12).contains(&m) {
        return Err("month must be in 1..12".into());
    }
    if d < 1 || d > days_in_month(y, m) {
        return Err("day is out of range for month".into());
    }
    Ok(())
}

fn check_time(h: i64, mi: i64, s: i64) -> Result<(), String> {
    if !(0..=23).contains(&h) {
        return Err("hour must be in 0..23".into());
    }
    if !(0..=59).contains(&mi) {
        return Err("minute must be in 0..59".into());
    }
    if !(0..=59).contains(&s) {
        return Err("second must be in 0..59".into());
    }
    Ok(())
}

/// `datetime.fromisoformat(s)`; `Err` is the `ValueError` message.
pub fn fromisoformat(s: &str) -> Result<IsoDateTime, String> {
    let invalid = || format!("Invalid isoformat string: {}", pystr::repr(s));
    if s.chars().count() < 7 {
        return Err(invalid());
    }
    let bytes = s.as_bytes();
    let b = Buf(bytes);
    let len = bytes.len();
    let sep = find_separator(bytes);
    let date_len = if sep < 0 { usize::MAX } else { sep as usize };
    let (year, month, day) = parse_date(&b, date_len).map_err(|_| invalid())?;
    let mut time = [0; 4];
    let mut tz = None;
    if sep >= 0 && len > date_len {
        let mut p = date_len;
        let lead = b.at(p);
        p += if lead & 0x80 == 0 {
            1
        } else {
            match lead & 0xf0 {
                0xe0 => 3,
                0xf0 => 4,
                _ => 2,
            }
        };
        let (t, z) = parse_time(&b, p, len.saturating_sub(p)).map_err(|_| invalid())?;
        time = t;
        tz = z;
    } else if sep < 0 {
        return Err(invalid());
    }
    let offset_us = match tz {
        None => None,
        Some((0, 0)) => Some(0),
        Some((secs, us)) => {
            let total = secs * 1_000_000 + us;
            if total != 0 && total.abs() >= 86_400_000_000 {
                return Err(format!(
                    "offset must be a timedelta strictly between -timedelta(hours=24) and \
                     timedelta(hours=24), not {}.",
                    timedelta_repr(total)
                ));
            }
            Some(total)
        }
    };
    check_date(year, month, day)?;
    check_time(time[0], time[1], time[2])?;
    Ok(IsoDateTime {
        year,
        month,
        day,
        hour: time[0],
        minute: time[1],
        second: time[2],
        microsecond: time[3],
        offset_us,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(s: &str) -> IsoDateTime {
        fromisoformat(s).unwrap()
    }

    #[test]
    fn civil_round_trip() {
        for days in [-719_162, -1, 0, 1, 10_957, 2_932_896] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(ymd_to_ord(y, m, d) - EPOCH_ORD, days);
        }
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(ord_to_ymd(1), (1, 1, 1));
    }

    #[test]
    fn formats() {
        let iso = ok("2026-06-05T14:30:15.5+05:30");
        assert_eq!((iso.hour, iso.microsecond, iso.offset_us), (14, 500_000, Some(19_800_000_000)));
        assert_eq!(ok("2026W235").day, 5);
        assert_eq!(ok("2026-06-05 14").offset_us, None);
        assert_eq!(ok("20260605T143000Z").offset_us, Some(0));
    }

    #[test]
    fn python_313_messages() {
        assert_eq!(fromisoformat("2026-06-05T24:00").unwrap_err(), "hour must be in 0..23");
        assert_eq!(fromisoformat("2026-02-29").unwrap_err(), "day is out of range for month");
        assert_eq!(fromisoformat("x").unwrap_err(), "Invalid isoformat string: 'x'");
        assert_eq!(
            fromisoformat("2026-06-05T14:30+24:00").unwrap_err(),
            "offset must be a timedelta strictly between -timedelta(hours=24) and timedelta(hours=24), \
             not datetime.timedelta(days=1)."
        );
        // gh-152157 / gh-152079 (3.13.16+): empty fractions are rejected and a
        // sub-second offset is kept.
        assert!(fromisoformat("2026-06-05T14:30:00.+01:00").is_err());
        assert_eq!(ok("2026-06-05T14:30+00:00:00.5").offset_us, Some(500_000));
        assert_eq!(fromisoformat("0000-W01-1").unwrap_err(), "Invalid isoformat string: '0000-W01-1'");
    }

    #[test]
    fn total_seconds_rounds_then_truncates() {
        assert_eq!(total_seconds_int(-500_000), 0);
        assert_eq!(total_seconds_int(-1_500_000), -1);
        assert_eq!(total_seconds_int(253_402_300_799_999_999), 253_402_300_800);
        assert_eq!(total_seconds_int(1_999_999), 1);
    }

    #[test]
    fn offsets_and_reprs() {
        assert_eq!(format_offset(-39_600_000_000), "-11:00");
        assert_eq!(format_offset(-17_762_000_000), "-04:56:02");
        assert_eq!(timedelta_repr(-90_000_000_000), "datetime.timedelta(days=-2, seconds=82800)");
        assert_eq!(Wall::from_secs(-30_610_224_001).strftime_date(), "Dec 31, 999");
    }
}
