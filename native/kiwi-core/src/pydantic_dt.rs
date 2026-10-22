//! pydantic-core 2.50's lax `datetime` validation of a value FastAPI decoded
//! from a JSON body (`validate_python`), on the speedate release it ships.

use speedate::{
    Date, DateConfig, DateTime, DateTimeConfig, MicrosecondsPrecisionOverflowBehavior, ParseError, TimeConfig,
    TimestampUnit,
};
use strum::EnumMessage;

/// The Python value being validated.
#[derive(Clone, Copy, Debug)]
pub enum Input<'a> {
    Str(&'a str),
    /// An `int` that fits in i64.
    Int(i64),
    /// A `float`, or an `int` beyond i64 (pydantic falls back to `float(x)`).
    Float(f64),
    Bool,
    /// `None`, a list or a dict.
    Other,
}

/// A validated datetime: wall clock plus the UTC offset in seconds, if aware.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Parsed {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub microsecond: u32,
    pub offset: Option<i32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// `datetime_type`
    Type,
    /// `datetime_parsing`
    Parsing(&'static str),
    /// `datetime_from_date_parsing`
    FromDateParsing(&'static str),
}

impl Error {
    pub fn kind(&self) -> &'static str {
        match self {
            Error::Type => "datetime_type",
            Error::Parsing(_) => "datetime_parsing",
            Error::FromDateParsing(_) => "datetime_from_date_parsing",
        }
    }

    pub fn message(&self) -> String {
        match self {
            Error::Type => "Input should be a valid datetime".into(),
            Error::Parsing(e) => format!("Input should be a valid datetime, {e}"),
            Error::FromDateParsing(e) => format!("Input should be a valid datetime or date, {e}"),
        }
    }

    /// The `ctx.error` text, when the error type has one.
    pub fn ctx_error(&self) -> Option<&'static str> {
        match self {
            Error::Type => None,
            Error::Parsing(e) | Error::FromDateParsing(e) => Some(e),
        }
    }
}

fn doc(e: ParseError) -> &'static str {
    e.get_documentation().unwrap_or_default()
}

fn config() -> DateTimeConfig {
    DateTimeConfig {
        timestamp_unit: TimestampUnit::Infer,
        time_config: TimeConfig {
            microseconds_precision_overflow_behavior: MicrosecondsPrecisionOverflowBehavior::Truncate,
            unix_timestamp_offset: Some(0),
        },
    }
}

fn parsed(dt: DateTime) -> Result<Parsed, Error> {
    if dt.date.year == 0 {
        return Err(Error::Parsing("year 0 is out of range"));
    }
    Ok(Parsed {
        year: dt.date.year,
        month: dt.date.month,
        day: dt.date.day,
        hour: dt.time.hour,
        minute: dt.time.minute,
        second: dt.time.second,
        microsecond: dt.time.microsecond,
        offset: dt.time.tz_offset,
    })
}

fn timestamp_config() -> DateTimeConfig {
    DateTimeConfig {
        timestamp_unit: TimestampUnit::Infer,
        time_config: TimeConfig { unix_timestamp_offset: Some(0), ..Default::default() },
    }
}

fn as_datetime(input: Input<'_>) -> Result<DateTime, Error> {
    match input {
        Input::Str(s) => DateTime::parse_bytes_with_config(s.as_bytes(), &config()).map_err(|e| Error::Parsing(doc(e))),
        Input::Int(i) => {
            DateTime::from_timestamp_with_config(i, 0, &timestamp_config()).map_err(|e| Error::Parsing(doc(e)))
        }
        Input::Float(f) if f.is_nan() => Err(Error::Parsing("NaN values not permitted")),
        Input::Float(f) => DateTime::from_float_with_config(f, &timestamp_config()).map_err(|e| Error::Parsing(doc(e))),
        Input::Bool | Input::Other => Err(Error::Type),
    }
}

/// `DateTimeValidator.validate` in lax mode: a value that is not a datetime
/// falls back to a date at midnight, whose parse error then wins for strings.
pub fn validate(input: Input<'_>) -> Result<Parsed, Error> {
    match as_datetime(input) {
        Ok(dt) => parsed(dt),
        Err(err) => {
            let Input::Str(s) = input else { return Err(err) };
            let cfg = DateConfig { timestamp_unit: TimestampUnit::Infer };
            match Date::parse_bytes_with_config(s.as_bytes(), &cfg) {
                Ok(date) => {
                    let time = speedate::Time { hour: 0, minute: 0, second: 0, microsecond: 0, tz_offset: None };
                    parsed(DateTime { date, time })
                }
                Err(e) => Err(Error::FromDateParsing(doc(e))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes() {
        let p = validate(Input::Str("2026-10-22T11:00:00.123456789+02:00")).unwrap();
        assert_eq!((p.hour, p.microsecond, p.offset), (11, 123_456, Some(7200)));
        let p = validate(Input::Str("2026-10-22")).unwrap();
        assert_eq!((p.day, p.hour, p.offset), (22, 0, None));
        assert_eq!(validate(Input::Int(0)).unwrap().offset, Some(0));
        assert_eq!(validate(Input::Bool), Err(Error::Type));
        assert_eq!(validate(Input::Str("nope")), Err(Error::FromDateParsing("input is too short")),);
    }
}
