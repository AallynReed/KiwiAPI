//! The request models of `app/giveaways/schemas.py`. The datetime fields are
//! pydantic `datetime`s (lax, see `kiwi_core::pydantic_dt`) run through the
//! `_as_utc` field validator.

use axum::{
    extract::{FromRequest, FromRequestParts, Request},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, NaiveDate, Utc};
use kiwi_core::pydantic_dt::{self, Parsed};
use serde_json::Value;

use crate::{
    auth::Superuser,
    error::ApiError,
    state::AppState,
    validation::{BodyInput, Fail, Field, FromBody, Input, read_body, validate},
};

pub struct VaultItemCreate {
    pub name: String,
    pub description: Option<String>,
}

pub struct VaultItemUpdate {
    pub name: Option<String>,
    pub description: Option<String>,
}

pub struct VaultCodesAdd {
    pub codes: Vec<String>,
}

pub struct VaultCodeUpdate {
    pub code: String,
}

pub struct GiveawayCreate {
    pub title: String,
    pub description: Option<String>,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub vault_item_id: String,
}

pub struct GiveawayUpdate {
    pub title: Option<String>,
    pub description: Option<String>,
    pub starts_at: Option<DateTime<Utc>>,
    pub ends_at: Option<DateTime<Utc>>,
    pub vault_item_id: Option<String>,
}

/// The Python value FastAPI hands pydantic for a JSON body field.
fn dt_input(v: &Value) -> pydantic_dt::Input<'_> {
    match v {
        Value::String(s) => pydantic_dt::Input::Str(s),
        Value::Bool(_) => pydantic_dt::Input::Bool,
        Value::Number(n) => match n.as_i64() {
            Some(i) => pydantic_dt::Input::Int(i),
            None => pydantic_dt::Input::Float(n.as_f64().unwrap_or(f64::NAN)),
        },
        _ => pydantic_dt::Input::Other,
    }
}

/// `_as_utc`: naive is taken as UTC, aware is converted; `None` where
/// `astimezone` overflows (an OverflowError, which pydantic does not catch).
pub fn as_utc(p: &Parsed) -> Option<DateTime<Utc>> {
    let date = NaiveDate::from_ymd_opt(i32::from(p.year), u32::from(p.month), u32::from(p.day))?;
    let wall = date.and_hms_micro_opt(u32::from(p.hour), u32::from(p.minute), u32::from(p.second), p.microsecond)?;
    let utc = wall - chrono::Duration::seconds(i64::from(p.offset.unwrap_or(0)));
    (1..=9999).contains(&chrono::Datelike::year(&utc)).then(|| utc.and_utc())
}

fn coerce_dt(input: &Input<'_>) -> Result<DateTime<Utc>, Fail> {
    let value = input.to_value();
    match pydantic_dt::validate(dt_input(&value)) {
        Ok(p) => Ok(as_utc(&p).unwrap_or_default()),
        Err(e) => {
            let fail = Fail::new(e.kind(), e.message());
            Err(match e.ctx_error() {
                Some(err) => fail.with_ctx("error", err),
                None => fail,
            })
        }
    }
}

/// A body whose datetime field would make the validator raise: Python
/// answers that with the unhandled 500 whatever else is wrong with it.
pub fn overflows(body: &BodyInput, fields: &[&str]) -> bool {
    let BodyInput::Json(Value::Object(o)) = body else { return false };
    fields
        .iter()
        .filter_map(|f| o.get(*f))
        .any(|v| matches!(pydantic_dt::validate(dt_input(v)), Ok(p) if as_utc(&p).is_none()))
}

/// A request model, with the datetime fields whose validator can raise.
pub trait Body: FromBody + Send {
    const DATETIMES: &'static [&'static str] = &[];
}

impl FromBody for VaultItemCreate {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(VaultItemCreate {
                name: m.field("name").str().min_len(1).max_len(120).required(),
                description: m.field("description").str().max_len(2000).optional(),
            })
        })
        .required()
    }
}

impl Body for VaultItemCreate {}

impl FromBody for VaultItemUpdate {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(VaultItemUpdate {
                name: m.field("name").str().min_len(1).max_len(120).optional(),
                description: m.field("description").str().max_len(2000).optional(),
            })
        })
        .required()
    }
}

impl Body for VaultItemUpdate {}

impl FromBody for VaultCodesAdd {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(VaultCodesAdd { codes: m.field("codes").list(|f| f.str().required()).min_len(1).required() })
        })
        .required()
    }
}

impl Body for VaultCodesAdd {}

impl FromBody for VaultCodeUpdate {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| Some(VaultCodeUpdate { code: m.field("code").str().min_len(1).max_len(400).required() }))
            .required()
    }
}

impl Body for VaultCodeUpdate {}

impl FromBody for GiveawayCreate {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(GiveawayCreate {
                title: m.field("title").str().min_len(1).max_len(160).required(),
                description: m.field("description").str().max_len(4000).optional(),
                starts_at: m.field("starts_at").custom(coerce_dt).required(),
                ends_at: m.field("ends_at").custom(coerce_dt).required(),
                vault_item_id: m.field("vault_item_id").str().required(),
            })
        })
        .required()
    }
}

impl Body for GiveawayCreate {
    const DATETIMES: &'static [&'static str] = &["starts_at", "ends_at"];
}

impl FromBody for GiveawayUpdate {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(GiveawayUpdate {
                title: m.field("title").str().min_len(1).max_len(160).optional(),
                description: m.field("description").str().max_len(4000).optional(),
                starts_at: m.field("starts_at").custom(coerce_dt).optional(),
                ends_at: m.field("ends_at").custom(coerce_dt).optional(),
                vault_item_id: m.field("vault_item_id").str().optional(),
            })
        })
        .required()
    }
}

impl Body for GiveawayUpdate {
    const DATETIMES: &'static [&'static str] = &["starts_at", "ends_at"];
}

/// The body as FastAPI reads it for an admin route: decoded before the
/// router's superuser dependency, validated after it.
pub fn checked<B: Body>(body: &BodyInput) -> Result<B, ApiError> {
    if overflows(body, B::DATETIMES) {
        return Err(ApiError::internal());
    }
    validate::<B>(body)
}

/// `get_current_superuser` then the request model.
pub struct AdminBody<B>(pub B);

impl<B: Body> FromRequest<AppState> for AdminBody<B> {
    type Rejection = Response;

    async fn from_request(req: Request, state: &AppState) -> Result<Self, Response> {
        let (mut parts, body) = req.into_parts();
        let body = read_body(&parts.headers, body).await.map_err(IntoResponse::into_response)?;
        Superuser::from_request_parts(&mut parts, state).await.map_err(IntoResponse::into_response)?;
        checked::<B>(&body).map(AdminBody).map_err(IntoResponse::into_response)
    }
}
