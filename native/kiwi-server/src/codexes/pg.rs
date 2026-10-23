//! asyncpg as the codex code uses it: arguments encoded for the type Postgres
//! inferred, `jsonb` through `json.dumps`/`json.loads`, `timestamptz` as an
//! aware UTC datetime, and reads through a per-connection prepared statement
//! cache, so Postgres plans them as it plans asyncpg's (custom plans for the
//! first five runs, then possibly the generic one), which decides the order
//! of rows an ORDER BY leaves tied.

use std::error::Error;

use bytes::{BufMut, BytesMut};
use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value;
use tokio_postgres::{
    GenericClient,
    types::{FromSql, IsNull, ToSql, Type, to_sql_checked},
};

use crate::pyjson;

type BoxError = Box<dyn Error + Sync + Send>;

/// Microseconds between the Unix and Postgres epochs.
const PG_EPOCH_US: i64 = 946_684_800_000_000;

#[derive(Clone, Debug, PartialEq)]
pub enum P {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    /// JSON text (`json.dumps` of the value).
    Json(String),
    TextList(Vec<String>),
    Ts(DateTime<Utc>),
}

impl P {
    pub fn text(s: impl Into<String>) -> P {
        P::Text(s.into())
    }

    pub fn opt_text(s: Option<impl Into<String>>) -> P {
        s.map_or(P::Null, |v| P::Text(v.into()))
    }

    pub fn opt_int(v: Option<i64>) -> P {
        v.map_or(P::Null, P::Int)
    }

    pub fn opt_float(v: Option<f64>) -> P {
        v.map_or(P::Null, P::Float)
    }

    pub fn opt_bool(v: Option<bool>) -> P {
        v.map_or(P::Null, P::Bool)
    }

    pub fn json(v: &Value) -> P {
        P::Json(pyjson::dumps_spaced(v))
    }
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

impl ToSql for P {
    fn to_sql(&self, ty: &Type, out: &mut BytesMut) -> Result<IsNull, BoxError> {
        match self {
            P::Null => Ok(IsNull::Yes),
            P::Bool(b) => b.to_sql(ty, out),
            P::Int(v) => int_arg(*v, ty, out),
            P::Float(f) => match *ty {
                Type::FLOAT4 => (*f as f32).to_sql(ty, out),
                _ => f.to_sql(ty, out),
            },
            P::Text(s) => s.as_str().to_sql(ty, out),
            P::Json(s) => {
                if *ty == Type::JSONB {
                    out.put_u8(1);
                }
                out.put_slice(s.as_bytes());
                Ok(IsNull::No)
            }
            P::TextList(v) => v.iter().map(String::as_str).collect::<Vec<_>>().to_sql(ty, out),
            P::Ts(dt) => {
                out.put_i64(dt.timestamp_micros() - PG_EPOCH_US);
                Ok(IsNull::No)
            }
        }
    }

    fn accepts(_: &Type) -> bool {
        true
    }

    to_sql_checked!();
}

/// A column value as asyncpg hands it to Python.
#[derive(Clone, Debug, PartialEq)]
pub enum Cell {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    Json(Value),
    Ts(DateTime<Utc>),
}

impl Cell {
    pub fn is_null(&self) -> bool {
        matches!(self, Cell::Null)
    }

    pub fn str(&self) -> Option<&str> {
        match self {
            Cell::Text(s) => Some(s),
            _ => None,
        }
    }

    pub fn int(&self) -> Option<i64> {
        match self {
            Cell::Int(i) => Some(*i),
            _ => None,
        }
    }

    /// The value as Python would put it in a JSON payload (datetimes are
    /// converted by the caller, so they never get here).
    pub fn json(&self) -> Value {
        match self {
            Cell::Null => Value::Null,
            Cell::Bool(b) => Value::Bool(*b),
            Cell::Int(i) => Value::from(*i),
            Cell::Float(f) => serde_json::Number::from_f64(*f).map_or(Value::Null, Value::Number),
            Cell::Text(s) => Value::String(s.clone()),
            Cell::Json(v) => v.clone(),
            Cell::Ts(t) => Value::String(crate::dt::isoformat(t)),
        }
    }
}

struct JsonCell(Value);

impl<'a> FromSql<'a> for JsonCell {
    fn from_sql(ty: &Type, raw: &'a [u8]) -> Result<Self, BoxError> {
        let body = if *ty == Type::JSONB {
            match raw.split_first() {
                Some((1, rest)) => rest,
                _ => return Err("unsupported jsonb version".into()),
            }
        } else {
            raw
        };
        pyjson::loads(body).map(JsonCell).map_err(|e| format!("jsonb decode: {e:?}").into())
    }

    fn accepts(ty: &Type) -> bool {
        matches!(*ty, Type::JSONB | Type::JSON)
    }
}

struct TsCell(DateTime<Utc>);

impl<'a> FromSql<'a> for TsCell {
    fn from_sql(_: &Type, raw: &'a [u8]) -> Result<Self, BoxError> {
        let us = i64::from_be_bytes(raw.try_into()?);
        let dt = Utc.timestamp_micros(us + PG_EPOCH_US).single().ok_or("timestamp out of range")?;
        Ok(TsCell(dt))
    }

    fn accepts(ty: &Type) -> bool {
        matches!(*ty, Type::TIMESTAMPTZ | Type::TIMESTAMP)
    }
}

fn cell(row: &tokio_postgres::Row, i: usize) -> Result<Cell, tokio_postgres::Error> {
    let ty = row.columns()[i].type_().clone();
    Ok(match ty {
        Type::BOOL => row.try_get::<_, Option<bool>>(i)?.map(Cell::Bool),
        Type::INT2 => row.try_get::<_, Option<i16>>(i)?.map(|v| Cell::Int(v.into())),
        Type::INT4 => row.try_get::<_, Option<i32>>(i)?.map(|v| Cell::Int(v.into())),
        Type::INT8 => row.try_get::<_, Option<i64>>(i)?.map(Cell::Int),
        Type::FLOAT4 => row.try_get::<_, Option<f32>>(i)?.map(|v| Cell::Float(v.into())),
        Type::FLOAT8 => row.try_get::<_, Option<f64>>(i)?.map(Cell::Float),
        Type::JSONB | Type::JSON => row.try_get::<_, Option<JsonCell>>(i)?.map(|v| Cell::Json(v.0)),
        Type::TIMESTAMPTZ | Type::TIMESTAMP => row.try_get::<_, Option<TsCell>>(i)?.map(|v| Cell::Ts(v.0)),
        _ => row.try_get::<_, Option<String>>(i)?.map(Cell::Text),
    }
    .unwrap_or(Cell::Null))
}

/// One record: column names and values in select order (`dict(record)`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row(pub Vec<(String, Cell)>);

const NULL: Cell = Cell::Null;

impl Row {
    pub fn get(&self, name: &str) -> &Cell {
        self.0.iter().find(|(k, _)| k == name).map_or(&NULL, |(_, v)| v)
    }

    pub fn first(&self) -> &Cell {
        self.0.first().map_or(&NULL, |(_, v)| v)
    }

    pub fn text(&self, name: &str) -> String {
        self.get(name).str().unwrap_or_default().to_owned()
    }

    /// `dict(record)` as JSON, datetimes in `isoformat()`.
    pub fn to_map(&self) -> serde_json::Map<String, Value> {
        self.0.iter().map(|(k, v)| (k.clone(), v.json())).collect()
    }
}

fn params(args: &[P]) -> Vec<&(dyn ToSql + Sync)> {
    args.iter().map(|a| a as &(dyn ToSql + Sync)).collect()
}

/// `con.fetch`.
pub async fn fetch(client: &impl deadpool_postgres::GenericClient, sql: &str, args: &[P]) -> anyhow::Result<Vec<Row>> {
    let stmt = client.prepare_cached(sql).await?;
    let rows = client.query(&stmt, &params(args)).await?;
    rows.iter()
        .map(|r| {
            let cols = r.columns().iter().enumerate().map(|(i, c)| Ok((c.name().to_owned(), cell(r, i)?)));
            cols.collect::<Result<Vec<_>, tokio_postgres::Error>>().map(Row)
        })
        .collect::<Result<_, _>>()
        .map_err(Into::into)
}

/// `con.fetchrow`.
pub async fn fetchrow(
    client: &impl deadpool_postgres::GenericClient,
    sql: &str,
    args: &[P],
) -> anyhow::Result<Option<Row>> {
    Ok(fetch(client, sql, args).await?.into_iter().next())
}

/// `con.fetchval`.
pub async fn fetchval(client: &impl deadpool_postgres::GenericClient, sql: &str, args: &[P]) -> anyhow::Result<Cell> {
    Ok(fetchrow(client, sql, args).await?.map_or(Cell::Null, |r| r.first().clone()))
}

/// `con.execute`.
pub async fn execute(client: &impl GenericClient, sql: &str, args: &[P]) -> anyhow::Result<()> {
    client.execute(sql, &params(args)).await?;
    Ok(())
}

/// `con.executemany`: one prepared statement, rows pipelined.
pub async fn executemany(client: &impl GenericClient, sql: &str, rows: &[Vec<P>]) -> anyhow::Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let stmt = client.prepare(sql).await?;
    for chunk in rows.chunks(256) {
        let ps: Vec<Vec<&(dyn ToSql + Sync)>> = chunk.iter().map(|r| params(r)).collect();
        futures::future::try_join_all(ps.iter().map(|p| client.execute(&stmt, p))).await?;
    }
    Ok(())
}
