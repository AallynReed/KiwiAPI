//! Datetimes as the Python app writes them. Three text formats are in use:
//!
//! - [`pydantic`]: response models, `2024-01-02T03:04:05.123000Z` (`Z` for
//!   UTC, fraction only when microseconds are nonzero, always 6 digits).
//! - [`isoformat`]: `datetime.isoformat()` / `jsonable_encoder` / `iso()`,
//!   `2024-01-02T03:04:05.123000+00:00`.
//! - [`str_dt`]: `str(datetime)` (`json.dumps(..., default=str)` in Redis and
//!   SSE payloads), `2024-01-02 03:04:05.123000+00:00`.
//!
//! Python datetimes have microsecond precision and BSON dates millisecond
//! precision, so a value read back from Mongo is floored to the millisecond
//! ([`mongo_round_trip`]) while a freshly created one ([`now`]) keeps
//! microseconds.

use std::fmt::Write;

use chrono::{
    DateTime, Datelike, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime, Offset, TimeZone,
    Timelike, Utc,
};
use mongodb::bson;

fn push_naive(out: &mut String, dt: &NaiveDateTime, sep: char) {
    let _ = write!(
        out,
        "{:04}-{:02}-{:02}{sep}{:02}:{:02}:{:02}",
        dt.year(),
        dt.month(),
        dt.day(),
        dt.hour(),
        dt.minute(),
        dt.second()
    );
    let us = dt.nanosecond().min(999_999_999) / 1000;
    if us != 0 {
        let _ = write!(out, ".{us:06}");
    }
}

/// Python's `utcoffset()` text: `+HH:MM`, with `:SS` only when needed.
fn push_offset(out: &mut String, seconds: i32) {
    let sign = if seconds < 0 { '-' } else { '+' };
    let s = seconds.unsigned_abs();
    let _ = write!(out, "{sign}{:02}:{:02}", s / 3600, s / 60 % 60);
    if !s.is_multiple_of(60) {
        let _ = write!(out, ":{:02}", s % 60);
    }
}

fn offset_seconds<Tz: TimeZone>(dt: &DateTime<Tz>) -> i32 {
    dt.offset().fix().local_minus_utc()
}

/// pydantic's datetime text (`...Z` for UTC, else `+HH:MM`; any seconds of
/// the offset are dropped).
pub fn pydantic<Tz: TimeZone>(dt: &DateTime<Tz>) -> String {
    let mut out = String::with_capacity(32);
    push_naive(&mut out, &dt.naive_local(), 'T');
    match offset_seconds(dt) {
        0 => out.push('Z'),
        s => push_offset(&mut out, s - s % 60),
    }
    out
}

/// `datetime.isoformat()` of an aware datetime (`+00:00` for UTC).
pub fn isoformat<Tz: TimeZone>(dt: &DateTime<Tz>) -> String {
    let mut out = String::with_capacity(32);
    push_naive(&mut out, &dt.naive_local(), 'T');
    push_offset(&mut out, offset_seconds(dt));
    out
}

/// `str(datetime)` of an aware datetime: isoformat with a space separator.
pub fn str_dt<Tz: TimeZone>(dt: &DateTime<Tz>) -> String {
    let mut out = String::with_capacity(32);
    push_naive(&mut out, &dt.naive_local(), ' ');
    push_offset(&mut out, offset_seconds(dt));
    out
}

/// A naive datetime in pydantic and `isoformat()` form (identical: no suffix).
pub fn isoformat_naive(dt: &NaiveDateTime) -> String {
    let mut out = String::with_capacity(26);
    push_naive(&mut out, dt, 'T');
    out
}

/// `str()` of a naive datetime.
pub fn str_naive(dt: &NaiveDateTime) -> String {
    let mut out = String::with_capacity(26);
    push_naive(&mut out, dt, ' ');
    out
}

/// `date.isoformat()` (also pydantic's `date` format).
pub fn date_iso(d: &NaiveDate) -> String {
    format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day())
}

/// Drops sub-microsecond precision, which Python datetimes cannot hold.
pub fn trunc_micros(dt: DateTime<Utc>) -> DateTime<Utc> {
    let ns = dt.timestamp_subsec_nanos();
    dt.with_nanosecond(ns - ns % 1000).unwrap_or(dt)
}

/// `datetime.now(timezone.utc)`.
pub fn now() -> DateTime<Utc> {
    trunc_micros(Utc::now())
}

/// What a datetime becomes after a BSON write and read: floored to the
/// millisecond (pymongo stores `seconds * 1000 + microsecond // 1000`).
pub fn mongo_round_trip(dt: DateTime<Utc>) -> DateTime<Utc> {
    from_bson(to_bson(dt))
}

pub fn to_bson(dt: DateTime<Utc>) -> bson::DateTime {
    bson::DateTime::from_millis(dt.timestamp_millis())
}

/// A BSON date as pymongo (`tz_aware=True`) hands it to the app.
pub fn from_bson(b: bson::DateTime) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(b.timestamp_millis()).unwrap_or_default()
}

/// `datetime.fromtimestamp(ts, timezone.utc)`: microseconds rounded half to
/// even, as CPython's `_PyTime_ObjectToTimeval` does.
pub fn from_timestamp(ts: f64) -> Option<DateTime<Utc>> {
    if !ts.is_finite() {
        return None;
    }
    let mut int = ts.trunc();
    let mut us = ((ts - int) * 1e6).round_ties_even();
    if us >= 1e6 {
        us -= 1e6;
        int += 1.0;
    } else if us < 0.0 {
        us += 1e6;
        int -= 1.0;
    }
    DateTime::from_timestamp(int as i64, us as u32 * 1000)
}

/// `#[serde(serialize_with = "dt::ser_pydantic")]` for response-model fields.
pub fn ser_pydantic<S: serde::Serializer>(d: &DateTime<Utc>, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&pydantic(d))
}

/// [`ser_pydantic`] for `Option` fields (`None` stays `null`).
pub fn ser_pydantic_opt<S: serde::Serializer>(d: &Option<DateTime<Utc>>, s: S) -> Result<S::Ok, S::Error> {
    match d {
        Some(d) => s.serialize_str(&pydantic(d)),
        None => s.serialize_none(),
    }
}

/// `#[serde(serialize_with = "dt::ser_isoformat")]` for `jsonable_encoder` /
/// `iso()` bodies.
pub fn ser_isoformat<S: serde::Serializer>(d: &DateTime<Utc>, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&isoformat(d))
}

/// [`ser_isoformat`] for `Option` fields.
pub fn ser_isoformat_opt<S: serde::Serializer>(d: &Option<DateTime<Utc>>, s: S) -> Result<S::Ok, S::Error> {
    match d {
        Some(d) => s.serialize_str(&isoformat(d)),
        None => s.serialize_none(),
    }
}

/// Reads any of the three formats back (`T` or space separator, optional
/// fraction, `Z` / `±HH:MM[:SS]` / `±HHMM` / no offset). A naive value is
/// taken as UTC, which is what the app writes everywhere.
pub fn parse(s: &str) -> Option<DateTime<FixedOffset>> {
    let s = s.trim();
    if s.len() < 19 || !s.is_char_boundary(10) || !s.is_char_boundary(19) {
        return None;
    }
    let date = NaiveDate::parse_from_str(&s[..10], "%Y-%m-%d").ok()?;
    if !matches!(s.as_bytes()[10], b'T' | b't' | b' ') {
        return None;
    }
    let mut time = NaiveTime::parse_from_str(&s[11..19], "%H:%M:%S").ok()?;
    let mut rest = &s[19..];
    if let Some(frac) = rest.strip_prefix('.') {
        let digits = frac.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 || digits > 9 {
            return None;
        }
        let mut ns: u32 = frac[..digits].parse().ok()?;
        ns *= 10u32.pow(9 - digits as u32);
        time = time.with_nanosecond(ns - ns % 1000)?;
        rest = &frac[digits..];
    }
    let offset = match rest {
        "" | "Z" | "z" => 0,
        _ => {
            let sign = match rest.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let hms = rest[1..].replace(':', "");
            if !matches!(hms.len(), 4 | 6) || !hms.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let part = |i: usize| hms.get(i..i + 2).map_or(Some(0), |p| p.parse::<i32>().ok());
            sign * (part(0)? * 3600 + part(2)? * 60 + part(4)?)
        }
    };
    let tz = FixedOffset::east_opt(offset)?;
    tz.from_local_datetime(&date.and_time(time)).single()
}

/// [`parse`] converted to UTC.
pub fn parse_utc(s: &str) -> Option<DateTime<Utc>> {
    parse(s).map(|d| d.with_timezone(&Utc))
}
