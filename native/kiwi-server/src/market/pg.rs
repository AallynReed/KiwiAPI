//! The asyncpg calls the market code makes. Arguments are encoded for the type
//! Postgres inferred, as asyncpg's codecs do (a float bound to an `int4`
//! parameter is truncated), and rows come back as the Python values asyncpg
//! hands the app, so the fixtures can stand in for the database.

use std::error::Error;

use bytes::BytesMut;
use tokio_postgres::types::{FromSql, IsNull, ToSql, Type, to_sql_checked};

type BoxError = Box<dyn Error + Sync + Send>;

#[derive(Clone, Debug, PartialEq)]
pub enum Arg {
    Int(i64),
    Float(f64),
    Text(String),
    TextList(Vec<String>),
}

fn int_arg(v: i64, ty: &Type, out: &mut BytesMut) -> Result<IsNull, BoxError> {
    match *ty {
        Type::INT2 => i16::try_from(v).map_err(|_| "value out of int16 range")?.to_sql(ty, out),
        Type::INT4 => i32::try_from(v).map_err(|_| "value out of int32 range")?.to_sql(ty, out),
        Type::INT8 => v.to_sql(ty, out),
        Type::FLOAT4 => (v as f32).to_sql(ty, out),
        Type::FLOAT8 => (v as f64).to_sql(ty, out),
        _ => Err(format!("no int encoding for {ty}").into()),
    }
}

impl ToSql for Arg {
    fn to_sql(&self, ty: &Type, out: &mut BytesMut) -> Result<IsNull, BoxError> {
        match self {
            Arg::Int(v) => int_arg(*v, ty, out),
            Arg::Float(f) => match *ty {
                Type::FLOAT4 => (*f as f32).to_sql(ty, out),
                Type::FLOAT8 => f.to_sql(ty, out),
                Type::INT2 | Type::INT4 | Type::INT8 if f.is_finite() && f.abs() < 9.2e18 => {
                    int_arg(f.trunc() as i64, ty, out)
                }
                _ => Err(format!("no float encoding for {ty}").into()),
            },
            Arg::Text(s) => s.as_str().to_sql(ty, out),
            Arg::TextList(v) => v.iter().map(String::as_str).collect::<Vec<_>>().to_sql(ty, out),
        }
    }

    fn accepts(_: &Type) -> bool {
        true
    }

    to_sql_checked!();
}

/// A column value as asyncpg returns it.
#[derive(Clone, Debug, PartialEq)]
pub enum PgValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    Uuid(u128),
    /// A `Decimal`, as its text.
    Numeric(String),
}

impl PgValue {
    pub fn is_null(&self) -> bool {
        matches!(self, PgValue::Null)
    }

    /// `int(v)` of an int, float or numeric.
    pub fn int(&self) -> Option<i64> {
        match self {
            PgValue::Int(i) => Some(*i),
            PgValue::Float(f) if f.is_finite() => Some(f.trunc() as i64),
            PgValue::Numeric(s) => s.split('.').next().and_then(|i| i.parse().ok()),
            PgValue::Bool(b) => Some(*b as i64),
            _ => None,
        }
    }

    /// `int(v or 0)`.
    pub fn int_or_zero(&self) -> Option<i64> {
        if self.is_null() { Some(0) } else { self.int() }
    }

    /// `float(v)`.
    pub fn float(&self) -> Option<f64> {
        match self {
            PgValue::Float(f) => Some(*f),
            PgValue::Int(i) => Some(*i as f64),
            PgValue::Numeric(s) => s.parse().ok(),
            _ => None,
        }
    }

    /// `float(v or 0)`.
    pub fn float_or_zero(&self) -> Option<f64> {
        match self {
            PgValue::Null => Some(0.0),
            PgValue::Float(f) if *f == 0.0 => Some(0.0),
            other => other.float(),
        }
    }

    pub fn text(&self) -> Option<&str> {
        match self {
            PgValue::Text(s) => Some(s),
            _ => None,
        }
    }
}

/// One record: column names and values in select order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row(pub Vec<(String, PgValue)>);

const NULL: PgValue = PgValue::Null;

impl Row {
    pub fn get(&self, name: &str) -> &PgValue {
        self.0.iter().find(|(k, _)| k == name).map_or(&NULL, |(_, v)| v)
    }

    pub fn first(&self) -> &PgValue {
        self.0.first().map_or(&NULL, |(_, v)| v)
    }
}

struct RawUuid(u128);

impl<'a> FromSql<'a> for RawUuid {
    fn from_sql(_: &Type, raw: &'a [u8]) -> Result<Self, BoxError> {
        let bytes: [u8; 16] = raw.try_into()?;
        Ok(RawUuid(u128::from_be_bytes(bytes)))
    }

    fn accepts(ty: &Type) -> bool {
        *ty == Type::UUID
    }
}

struct NumericText(String);

impl<'a> FromSql<'a> for NumericText {
    fn from_sql(_: &Type, raw: &'a [u8]) -> Result<Self, BoxError> {
        numeric_text(raw).map(NumericText).ok_or_else(|| "bad numeric".into())
    }

    fn accepts(ty: &Type) -> bool {
        *ty == Type::NUMERIC
    }
}

/// The binary `numeric` wire format as decimal text (`str(Decimal)` for the
/// finite values this code reads).
pub fn numeric_text(raw: &[u8]) -> Option<String> {
    let word = |i: usize| raw.get(i..i + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
    let ndigits = word(0)? as usize;
    let weight = word(2)? as i16 as i32;
    let sign = word(4)?;
    let dscale = word(6)? as usize;
    let digits: Vec<u16> = (0..ndigits).map(|i| word(8 + 2 * i)).collect::<Option<_>>()?;
    match sign {
        0xC000 => return Some("NaN".into()),
        0xD000 => return Some("Infinity".into()),
        0xF000 => return Some("-Infinity".into()),
        _ => {}
    }
    let group = |i: i32| if i >= 0 { digits.get(i as usize).copied().unwrap_or(0) } else { 0 };
    let mut int = String::new();
    for i in 0..=weight {
        if int.is_empty() {
            let d = group(i);
            if d != 0 {
                int = d.to_string();
            }
        } else {
            int.push_str(&format!("{:04}", group(i)));
        }
    }
    if int.is_empty() {
        int.push('0');
    }
    let mut frac = String::new();
    for k in 1..=dscale.div_ceil(4) as i32 {
        frac.push_str(&format!("{:04}", group(weight + k)));
    }
    frac.truncate(dscale);
    let mut out = String::new();
    if sign == 0x4000 && (int != "0" || frac.bytes().any(|b| b != b'0')) {
        out.push('-');
    }
    out.push_str(&int);
    if !frac.is_empty() {
        out.push('.');
        out.push_str(&frac);
    }
    Some(out)
}

fn cell(row: &tokio_postgres::Row, i: usize) -> Result<PgValue, tokio_postgres::Error> {
    let ty = row.columns()[i].type_().clone();
    Ok(match ty {
        Type::BOOL => row.try_get::<_, Option<bool>>(i)?.map(PgValue::Bool),
        Type::INT2 => row.try_get::<_, Option<i16>>(i)?.map(|v| PgValue::Int(v.into())),
        Type::INT4 => row.try_get::<_, Option<i32>>(i)?.map(|v| PgValue::Int(v.into())),
        Type::INT8 => row.try_get::<_, Option<i64>>(i)?.map(PgValue::Int),
        Type::FLOAT4 => row.try_get::<_, Option<f32>>(i)?.map(|v| PgValue::Float(v.into())),
        Type::FLOAT8 => row.try_get::<_, Option<f64>>(i)?.map(PgValue::Float),
        Type::UUID => row.try_get::<_, Option<RawUuid>>(i)?.map(|v| PgValue::Uuid(v.0)),
        Type::NUMERIC => row.try_get::<_, Option<NumericText>>(i)?.map(|v| PgValue::Numeric(v.0)),
        _ => row.try_get::<_, Option<String>>(i)?.map(PgValue::Text),
    }
    .unwrap_or(PgValue::Null))
}

/// Runs one asyncpg-style call on a pooled connection.
pub async fn run(pool: &deadpool_postgres::Pool, method: &str, sql: &str, args: &[Arg]) -> anyhow::Result<Vec<Row>> {
    let client = pool.get().await?;
    let params: Vec<&(dyn ToSql + Sync)> = args.iter().map(|a| a as &(dyn ToSql + Sync)).collect();
    if method == "execute" {
        client.execute(sql, &params).await?;
        return Ok(Vec::new());
    }
    let rows = client.query(sql, &params).await?;
    let take = if method == "fetch" { rows.len() } else { rows.len().min(1) };
    rows.iter()
        .take(take)
        .map(|r| {
            let cols = r.columns().iter().enumerate().map(|(i, c)| Ok((c.name().to_owned(), cell(r, i)?)));
            cols.collect::<Result<Vec<_>, tokio_postgres::Error>>().map(Row)
        })
        .collect::<Result<_, _>>()
        .map_err(Into::into)
}
