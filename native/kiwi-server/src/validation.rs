//! FastAPI request validation with pydantic v2's error items, rendered through
//! `app/core/errors.py`'s handler: 422, code `validation_error`, message
//! `Request validation failed`, details = `[{type, loc, msg, input, ctx?}]`.
//!
//! Inputs are read with explicit field builders that apply pydantic's lax
//! coercions and constraint messages, so every error a Python route would
//! report is reported the same way (all of them, in order):
//!
//! ```no_run
//! use kiwi_server::{pyjson::PyJson, validation::{Field, FromBody, FromParams, Json, Params, Query}};
//!
//! // limit: int = Query(50, ge=1, le=200), fmt: str = Query("json", pattern="^(json|bin)$")
//! struct ListQuery { limit: i64, fmt: String }
//! impl FromParams for ListQuery {
//!     fn from_params(p: &mut Params<'_>) -> Self {
//!         ListQuery {
//!             limit: p.query("limit").int().ge(1).le(200).default(50),
//!             fmt: p.query("fmt").str().one_of_pattern(&["json", "bin"]).default("json".into()),
//!         }
//!     }
//! }
//! async fn list(Query(q): Query<ListQuery>) -> PyJson {
//!     PyJson(serde_json::json!({"limit": q.limit, "fmt": q.fmt}))
//! }
//!
//! // class Login(BaseModel): email: str; password: str = Field(min_length=8, max_length=256)
//! struct Login { email: String, password: String }
//! impl FromBody for Login {
//!     fn from_body(body: Field<'_>) -> Option<Self> {
//!         body.model(|m| Some(Login {
//!             email: m.field("email").str().required(),
//!             password: m.field("password").str().min_len(8).max_len(256).required(),
//!         }))
//!         .required()
//!     }
//! }
//! async fn login(Json(body): Json<Login>) -> PyJson {
//!     PyJson(serde_json::json!({"email": body.email}))
//! }
//! ```
//!
//! Call the accessors in FastAPI's order: a dependency's parameters first,
//! then path, query, header, and the body last ([`Checked`] does path/query
//! then body). Terminals return a placeholder after recording an error; the
//! extractor rejects before the handler ever sees it.

use std::{borrow::Cow, marker::PhantomData};

use axum::{
    body::Body,
    extract::{FromRequest, FromRequestParts, RawPathParams, Request},
    http::{HeaderMap, StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
};
use mongodb::bson::oid::ObjectId;
use serde_json::{Map, Value};

use crate::{
    error::{ApiError, ErrorCode},
    pyfloat,
    pyjson::{self, LoadsError},
    request_id,
};

// ---------------------------------------------------------------------------
// Error items

#[derive(Clone, Debug, PartialEq)]
pub enum LocItem {
    Key(String),
    Index(usize),
}

impl From<&str> for LocItem {
    fn from(s: &str) -> Self {
        LocItem::Key(s.to_owned())
    }
}

impl From<String> for LocItem {
    fn from(s: String) -> Self {
        LocItem::Key(s)
    }
}

impl From<usize> for LocItem {
    fn from(i: usize) -> Self {
        LocItem::Index(i)
    }
}

pub type Loc = Vec<LocItem>;

/// One pydantic error dict, as `jsonable_encoder(exc.errors())` emits it.
#[derive(Clone, Debug)]
pub struct ErrorItem {
    pub kind: Cow<'static, str>,
    pub loc: Loc,
    pub msg: String,
    pub input: Value,
    pub ctx: Option<Map<String, Value>>,
}

impl ErrorItem {
    pub fn to_value(&self) -> Value {
        let mut m = Map::new();
        m.insert("type".into(), Value::String(self.kind.to_string()));
        let loc = self
            .loc
            .iter()
            .map(|l| match l {
                LocItem::Key(k) => Value::String(k.clone()),
                LocItem::Index(i) => Value::from(*i),
            })
            .collect();
        m.insert("loc".into(), Value::Array(loc));
        m.insert("msg".into(), Value::String(self.msg.clone()));
        m.insert("input".into(), self.input.clone());
        if let Some(ctx) = &self.ctx {
            m.insert("ctx".into(), Value::Object(ctx.clone()));
        }
        Value::Object(m)
    }

    /// FastAPI's malformed-JSON-body error.
    pub fn json_invalid(pos: usize, msg: &str) -> Self {
        let mut ctx = Map::new();
        ctx.insert("error".into(), Value::String(msg.to_owned()));
        ErrorItem {
            kind: "json_invalid".into(),
            loc: vec!["body".into(), pos.into()],
            msg: "JSON decode error".into(),
            input: Value::Object(Map::new()),
            ctx: Some(ctx),
        }
    }
}

/// A constraint or bound in a message and in `ctx` (`ge=1` vs `le=1.0` print
/// the same in the message but differ in `ctx`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Num {
    Int(i64),
    Float(f64),
}

impl From<i64> for Num {
    fn from(v: i64) -> Self {
        Num::Int(v)
    }
}

impl From<i32> for Num {
    fn from(v: i32) -> Self {
        Num::Int(v.into())
    }
}

impl From<u32> for Num {
    fn from(v: u32) -> Self {
        Num::Int(v.into())
    }
}

impl From<f64> for Num {
    fn from(v: f64) -> Self {
        Num::Float(v)
    }
}

impl Num {
    fn text(self) -> String {
        match self {
            Num::Int(i) => i.to_string(),
            Num::Float(f) => pyfloat::display(f),
        }
    }

    fn json(self) -> Value {
        match self {
            Num::Int(i) => Value::from(i),
            Num::Float(f) => serde_json::Number::from_f64(f).map(Value::Number).unwrap_or(Value::Null),
        }
    }

    fn as_f64(self) -> f64 {
        match self {
            Num::Int(i) => i as f64,
            Num::Float(f) => f,
        }
    }
}

/// An error without its location and input, which the field supplies.
#[derive(Clone, Debug)]
pub struct Fail {
    pub kind: Cow<'static, str>,
    pub msg: String,
    pub ctx: Option<Map<String, Value>>,
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Python `repr()` of a str, as pydantic prints `Literal`/enum choices.
pub fn py_repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// pydantic's `expected` text: `'a'`, `'a' or 'b'`, `'a', 'b' or 'c'`.
fn expected(choices: &[&str]) -> String {
    let reprs: Vec<String> = choices.iter().map(|c| py_repr_str(c)).collect();
    match reprs.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
    }
}

impl Fail {
    pub fn new(kind: impl Into<Cow<'static, str>>, msg: impl Into<String>) -> Self {
        Fail { kind: kind.into(), msg: msg.into(), ctx: None }
    }

    pub fn with_ctx(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.ctx.get_or_insert_with(Map::new).insert(key.to_owned(), value.into());
        self
    }

    /// A `ValueError` raised by a validator: `Value error, {msg}`, ctx
    /// `{"error": {}}` (jsonable_encoder of the exception object).
    pub fn value_error(message: &str) -> Self {
        Fail::new("value_error", format!("Value error, {message}")).with_ctx("error", Map::new())
    }

    pub fn missing() -> Self {
        Fail::new("missing", "Field required")
    }
    pub fn int_type() -> Self {
        Fail::new("int_type", "Input should be a valid integer")
    }
    pub fn int_parsing() -> Self {
        Fail::new("int_parsing", "Input should be a valid integer, unable to parse string as an integer")
    }
    pub fn int_from_float() -> Self {
        Fail::new("int_from_float", "Input should be a valid integer, got a number with a fractional part")
    }
    pub fn int_parsing_size() -> Self {
        Fail::new("int_parsing_size", "Unable to parse input string as an integer, exceeded maximum size")
    }
    pub fn finite_number() -> Self {
        Fail::new("finite_number", "Input should be a finite number")
    }
    pub fn float_type() -> Self {
        Fail::new("float_type", "Input should be a valid number")
    }
    pub fn float_parsing() -> Self {
        Fail::new("float_parsing", "Input should be a valid number, unable to parse string as a number")
    }
    pub fn string_type() -> Self {
        Fail::new("string_type", "Input should be a valid string")
    }
    pub fn bool_type() -> Self {
        Fail::new("bool_type", "Input should be a valid boolean")
    }
    pub fn bool_parsing() -> Self {
        Fail::new("bool_parsing", "Input should be a valid boolean, unable to interpret input")
    }
    pub fn list_type() -> Self {
        Fail::new("list_type", "Input should be a valid list")
    }
    pub fn dict_type() -> Self {
        Fail::new("dict_type", "Input should be a valid dictionary")
    }
    /// A non-object where a model is expected (FastAPI validates with
    /// `from_attributes=True`, hence not `model_type`).
    pub fn model_attributes_type() -> Self {
        Fail::new("model_attributes_type", "Input should be a valid dictionary or object to extract fields from")
    }
    pub fn object_id() -> Self {
        Fail::value_error("Id must be of type PydanticObjectId")
    }

    fn bound(kind: &'static str, words: &str, key: &str, n: Num) -> Self {
        Fail::new(kind, format!("Input should be {words} {}", n.text())).with_ctx(key, n.json())
    }
    pub fn greater_than(n: Num) -> Self {
        Fail::bound("greater_than", "greater than", "gt", n)
    }
    pub fn greater_than_equal(n: Num) -> Self {
        Fail::bound("greater_than_equal", "greater than or equal to", "ge", n)
    }
    pub fn less_than(n: Num) -> Self {
        Fail::bound("less_than", "less than", "lt", n)
    }
    pub fn less_than_equal(n: Num) -> Self {
        Fail::bound("less_than_equal", "less than or equal to", "le", n)
    }
    pub fn multiple_of(n: Num) -> Self {
        Fail::bound("multiple_of", "a multiple of", "multiple_of", n)
    }
    pub fn string_too_short(min: usize) -> Self {
        Fail::new("string_too_short", format!("String should have at least {min} character{}", plural(min)))
            .with_ctx("min_length", min)
    }
    pub fn string_too_long(max: usize) -> Self {
        Fail::new("string_too_long", format!("String should have at most {max} character{}", plural(max)))
            .with_ctx("max_length", max)
    }
    pub fn string_pattern_mismatch(pattern: &str) -> Self {
        Fail::new("string_pattern_mismatch", format!("String should match pattern '{pattern}'"))
            .with_ctx("pattern", pattern)
    }
    pub fn too_short(min: usize, actual: usize) -> Self {
        Fail::new(
            "too_short",
            format!("List should have at least {min} item{} after validation, not {actual}", plural(min)),
        )
        .with_ctx("field_type", "List")
        .with_ctx("min_length", min)
        .with_ctx("actual_length", actual)
    }
    pub fn too_long(max: usize, actual: usize) -> Self {
        Fail::new(
            "too_long",
            format!("List should have at most {max} item{} after validation, not {actual}", plural(max)),
        )
        .with_ctx("field_type", "List")
        .with_ctx("max_length", max)
        .with_ctx("actual_length", actual)
    }
    pub fn literal_error(choices: &[&str]) -> Self {
        let e = expected(choices);
        Fail::new("literal_error", format!("Input should be {e}")).with_ctx("expected", e)
    }
    pub fn enum_error(choices: &[&str]) -> Self {
        let e = expected(choices);
        Fail::new("enum", format!("Input should be {e}")).with_ctx("expected", e)
    }
}

// ---------------------------------------------------------------------------
// Collector + the 422 response

#[derive(Debug, Default)]
pub struct Validator {
    pub errors: Vec<ErrorItem>,
}

impl Validator {
    pub fn push(&mut self, loc: Loc, input: Value, fail: Fail) {
        self.errors.push(ErrorItem { kind: fail.kind, loc, msg: fail.msg, input, ctx: fail.ctx });
    }

    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }

    pub fn finish<T>(self, value: T) -> Result<T, ValidationError> {
        if self.errors.is_empty() { Ok(value) } else { Err(ValidationError(self.errors)) }
    }
}

/// A `RequestValidationError`; converts into the 422 envelope.
#[derive(Debug)]
pub struct ValidationError(pub Vec<ErrorItem>);

impl From<ValidationError> for ApiError {
    fn from(e: ValidationError) -> Self {
        let details = Value::Array(e.0.iter().map(ErrorItem::to_value).collect());
        ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, ErrorCode::ValidationError, "Request validation failed")
            .with_details(details)
    }
}

impl axum::response::IntoResponse for ValidationError {
    fn into_response(self) -> axum::response::Response {
        ApiError::from(self).into_response()
    }
}

// ---------------------------------------------------------------------------
// Fields

/// What a field was given.
#[derive(Clone, Debug)]
pub enum Input<'a> {
    /// Absent. pydantic reports the enclosing object as the `input` of a
    /// missing model field; parameters and the body report `null`.
    Missing(Option<&'a Value>),
    /// A path, query or header value.
    Str(Cow<'a, str>),
    /// Every value of a repeated query parameter (`list[...]` params).
    Strs(Vec<Cow<'a, str>>),
    /// Part of a JSON body.
    Json(&'a Value),
}

impl Input<'_> {
    /// The `input` pydantic would report.
    pub fn to_value(&self) -> Value {
        match self {
            Input::Missing(parent) => parent.cloned().unwrap_or(Value::Null),
            Input::Str(s) => Value::String(s.to_string()),
            Input::Strs(v) => Value::Array(v.iter().map(|s| Value::String(s.to_string())).collect()),
            Input::Json(v) => (*v).clone(),
        }
    }

    /// A text value (parameter, or JSON string).
    fn text(&self) -> Option<&str> {
        match self {
            Input::Str(s) => Some(s),
            Input::Json(Value::String(s)) => Some(s),
            _ => None,
        }
    }
}

/// One value being validated, at `loc`.
pub struct Field<'a> {
    v: &'a mut Validator,
    loc: Loc,
    input: Input<'a>,
}

enum Got<T> {
    Missing,
    Null,
    Value(T),
    Failed,
}

impl<'a> Field<'a> {
    pub fn new(v: &'a mut Validator, loc: Loc, input: Input<'a>) -> Self {
        Field { v, loc, input }
    }

    pub fn loc(&self) -> &Loc {
        &self.loc
    }

    pub fn input(&self) -> &Input<'a> {
        &self.input
    }

    pub fn is_missing(&self) -> bool {
        matches!(self.input, Input::Missing(_))
    }

    /// Records `fail` at this field.
    pub fn fail(self, fail: Fail) {
        let input = self.input.to_value();
        self.v.push(self.loc, input, fail);
    }

    fn typed<C: Coerce>(self, c: C) -> Typed<'a, C> {
        Typed { field: self, c, checks: Vec::new() }
    }

    pub fn int(self) -> Typed<'a, IntC> {
        self.typed(IntC::default())
    }

    pub fn float(self) -> Typed<'a, FloatC> {
        self.typed(FloatC::default())
    }

    pub fn str(self) -> Typed<'a, StrC> {
        self.typed(StrC::default())
    }

    pub fn bool(self) -> Typed<'a, BoolC> {
        self.typed(BoolC)
    }

    /// `Literal["a", "b"]`.
    pub fn literal(self, choices: &'static [&'static str]) -> Typed<'a, ChoiceC> {
        self.typed(ChoiceC { choices, is_enum: false })
    }

    /// A `str` Enum with these values.
    pub fn enumeration(self, choices: &'static [&'static str]) -> Typed<'a, ChoiceC> {
        self.typed(ChoiceC { choices, is_enum: true })
    }

    /// Beanie's `PydanticObjectId`.
    pub fn object_id(self) -> Typed<'a, ObjectIdC> {
        self.typed(ObjectIdC)
    }

    /// `Any`: the JSON value (or parameter text) unchanged.
    pub fn any(self) -> Typed<'a, AnyC> {
        self.typed(AnyC)
    }

    /// A hand-written coercion (`Err` is recorded at this field).
    pub fn custom<T: Default, F: FnOnce(&Input<'_>) -> Result<T, Fail>>(self, f: F) -> Typed<'a, CustomC<T, F>> {
        self.typed(CustomC(std::cell::Cell::new(Some(f)), PhantomData))
    }

    /// A nested model: `f` reads the fields of the object.
    pub fn model<T: Default, F: FnOnce(&mut Model<'_>) -> T>(self, f: F) -> Nested<'a, T, F, ModelK> {
        Nested { field: self, f, min: None, max: None, _k: ModelK, _t: PhantomData }
    }

    /// `list[T]`: `f` validates each item.
    pub fn list<T, F: FnMut(Field<'_>) -> T>(self, f: F) -> Nested<'a, T, F, ListK> {
        Nested { field: self, f, min: None, max: None, _k: ListK, _t: PhantomData }
    }

    /// `dict[str, T]`: `f` validates each value (keys in input order).
    pub fn dict<T, F: FnMut(Field<'_>) -> T>(self, f: F) -> Nested<'a, T, F, DictK> {
        Nested { field: self, f, min: None, max: None, _k: DictK, _t: PhantomData }
    }
}

/// A scalar coercion: pydantic's lax-mode conversion plus constraints.
pub trait Coerce {
    type Out;
    /// Converts a present, non-null-for-optional input.
    fn coerce(&self, input: &Input<'_>) -> Result<Self::Out, Fail>;
    /// Stands in for the value after an error (never reaches a handler).
    fn placeholder(&self) -> Self::Out;
}

type Check<'a, T> = Box<dyn FnOnce(&T) -> Result<(), Fail> + 'a>;

fn push_missing(field: &mut Field<'_>) {
    let input = field.input.to_value();
    field.v.push(field.loc.clone(), input, Fail::missing());
}

/// A field with a coercion chosen; finish with a terminal
/// (`required`/`default`/`optional`/`nullable`/`nullable_default`).
pub struct Typed<'a, C: Coerce> {
    field: Field<'a>,
    c: C,
    checks: Vec<Check<'a, C::Out>>,
}

impl<'a, C: Coerce> Typed<'a, C> {
    /// An after-validator (`@field_validator`), run only when the value
    /// passed its type and constraints.
    pub fn check(mut self, f: impl FnOnce(&C::Out) -> Result<(), Fail> + 'a) -> Self {
        self.checks.push(Box::new(f));
        self
    }

    fn get(self, null_is_none: bool) -> (Got<C::Out>, C) {
        let Typed { field: Field { v, loc, input }, c, checks } = self;
        let got = match &input {
            Input::Missing(_) => Got::Missing,
            Input::Json(Value::Null) if null_is_none => Got::Null,
            _ => match c.coerce(&input).and_then(|out| checks.into_iter().try_for_each(|f| f(&out)).map(|_| out)) {
                Ok(out) => Got::Value(out),
                Err(fail) => {
                    v.push(loc, input.to_value(), fail);
                    Got::Failed
                }
            },
        };
        (got, c)
    }

    /// No default: absence is a `missing` error.
    pub fn required(mut self) -> C::Out {
        if self.field.is_missing() {
            push_missing(&mut self.field);
            return self.c.placeholder();
        }
        match self.get(false) {
            (Got::Value(v), _) => v,
            (_, c) => c.placeholder(),
        }
    }

    /// `x: T = default`.
    pub fn default(self, default: C::Out) -> C::Out {
        match self.get(false) {
            (Got::Value(v), _) => v,
            (Got::Missing, _) => default,
            (_, c) => c.placeholder(),
        }
    }

    /// `x: T | None = None`.
    pub fn optional(self) -> Option<C::Out> {
        match self.get(true) {
            (Got::Value(v), _) => Some(v),
            _ => None,
        }
    }

    /// `x: T | None = default`.
    pub fn nullable_default(self, default: C::Out) -> Option<C::Out> {
        match self.get(true) {
            (Got::Value(v), _) => Some(v),
            (Got::Missing, _) => Some(default),
            _ => None,
        }
    }

    /// `x: T | None` with no default: absence is an error, null is `None`.
    pub fn nullable(mut self) -> Option<C::Out> {
        if self.field.is_missing() {
            push_missing(&mut self.field);
            return None;
        }
        match self.get(true) {
            (Got::Value(v), _) => Some(v),
            _ => None,
        }
    }
}

// --- integers --------------------------------------------------------------

#[derive(Default)]
pub struct IntC {
    multiple_of: Option<Num>,
    le: Option<Num>,
    lt: Option<Num>,
    ge: Option<Num>,
    gt: Option<Num>,
    saturate: bool,
}

impl<'a> Typed<'a, IntC> {
    /// A Python `int` past `i64` saturates instead of failing (pydantic takes
    /// big ints; only for a value the caller clamps anyway).
    pub fn saturating(mut self) -> Self {
        self.c.saturate = true;
        self
    }
    pub fn ge(mut self, n: impl Into<Num>) -> Self {
        self.c.ge = Some(n.into());
        self
    }
    pub fn gt(mut self, n: impl Into<Num>) -> Self {
        self.c.gt = Some(n.into());
        self
    }
    pub fn le(mut self, n: impl Into<Num>) -> Self {
        self.c.le = Some(n.into());
        self
    }
    pub fn lt(mut self, n: impl Into<Num>) -> Self {
        self.c.lt = Some(n.into());
        self
    }
    pub fn multiple_of(mut self, n: impl Into<Num>) -> Self {
        self.c.multiple_of = Some(n.into());
        self
    }
}

/// Ordering of an integer against a bound (big values are saturated).
fn cmp_int(v: i128, n: Num) -> std::cmp::Ordering {
    match n {
        Num::Int(i) => v.cmp(&(i as i128)),
        Num::Float(f) => (v as f64).partial_cmp(&f).unwrap_or(std::cmp::Ordering::Equal),
    }
}

/// Python `int()` digits: optional sign, ASCII digits, single underscores
/// between digits. Values beyond `i128` saturate.
fn parse_py_int(s: &str) -> Option<i128> {
    let (neg, digits) = match s.as_bytes().first()? {
        b'-' => (true, &s[1..]),
        b'+' => (false, &s[1..]),
        _ => (false, s),
    };
    let b = digits.as_bytes();
    if b.is_empty() || !b[0].is_ascii_digit() || !b[b.len() - 1].is_ascii_digit() {
        return None;
    }
    let mut v: i128 = 0;
    for (i, &c) in b.iter().enumerate() {
        if c == b'_' {
            if !b[i + 1].is_ascii_digit() {
                return None;
            }
            continue;
        }
        if !c.is_ascii_digit() {
            return None;
        }
        v = v.saturating_mul(10).saturating_add((c - b'0') as i128);
    }
    Some(if neg { -v } else { v })
}

/// pydantic-core `str_as_int`: trimmed, `int()` syntax, or a decimal whose
/// fraction is all zeros (`"1.0"`, not `"1."`).
pub fn str_as_int(s: &str) -> Result<i128, Fail> {
    let t = s.trim();
    if t.len() > 4300 {
        return Err(Fail::int_parsing_size());
    }
    if let Some(v) = parse_py_int(t) {
        return Ok(v);
    }
    if let Some((whole, frac)) = t.split_once('.')
        && !frac.is_empty()
        && frac.bytes().all(|b| b == b'0')
        && let Some(v) = parse_py_int(whole)
    {
        return Ok(v);
    }
    Err(Fail::int_parsing())
}

fn float_as_int(f: f64) -> Result<i128, Fail> {
    if !f.is_finite() {
        Err(Fail::finite_number())
    } else if f % 1.0 != 0.0 {
        Err(Fail::int_from_float())
    } else if (i64::MIN as f64) < f && f < (i64::MAX as f64) {
        Ok(f as i128)
    } else {
        Err(Fail::int_parsing_size())
    }
}

impl Coerce for IntC {
    type Out = i64;

    fn coerce(&self, input: &Input<'_>) -> Result<i64, Fail> {
        let v = match input {
            Input::Json(Value::Number(n)) => match (n.as_i64(), n.as_u64(), n.as_f64()) {
                (Some(i), _, _) => i as i128,
                (_, Some(u), _) => u as i128,
                (_, _, f) => float_as_int(f.unwrap_or(f64::NAN))?,
            },
            Input::Json(Value::Bool(b)) => *b as i128,
            other => str_as_int(other.text().ok_or_else(Fail::int_type)?)?,
        };
        use std::cmp::Ordering::*;
        if let Some(m) = self.multiple_of {
            let ok = match m {
                Num::Int(0) => true,
                Num::Int(m) => v % m as i128 == 0,
                Num::Float(f) => (v as f64) % f == 0.0,
            };
            if !ok {
                return Err(Fail::multiple_of(m));
            }
        }
        if let Some(n) = self.le.filter(|&n| cmp_int(v, n) == Greater) {
            return Err(Fail::less_than_equal(n));
        }
        if let Some(n) = self.lt.filter(|&n| cmp_int(v, n) != Less) {
            return Err(Fail::less_than(n));
        }
        if let Some(n) = self.ge.filter(|&n| cmp_int(v, n) == Less) {
            return Err(Fail::greater_than_equal(n));
        }
        if let Some(n) = self.gt.filter(|&n| cmp_int(v, n) != Greater) {
            return Err(Fail::greater_than(n));
        }
        if self.saturate {
            return Ok(v.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64);
        }
        i64::try_from(v).map_err(|_| Fail::int_parsing_size())
    }

    fn placeholder(&self) -> i64 {
        0
    }
}

// --- floats ----------------------------------------------------------------

/// Float constraints; pydantic stores them as floats (`gt=0` reports `0.0`).
#[derive(Default)]
pub struct FloatC {
    multiple_of: Option<f64>,
    le: Option<f64>,
    lt: Option<f64>,
    ge: Option<f64>,
    gt: Option<f64>,
}

impl<'a> Typed<'a, FloatC> {
    pub fn ge(mut self, n: impl Into<Num>) -> Self {
        self.c.ge = Some(n.into().as_f64());
        self
    }
    pub fn gt(mut self, n: impl Into<Num>) -> Self {
        self.c.gt = Some(n.into().as_f64());
        self
    }
    pub fn le(mut self, n: impl Into<Num>) -> Self {
        self.c.le = Some(n.into().as_f64());
        self
    }
    pub fn lt(mut self, n: impl Into<Num>) -> Self {
        self.c.lt = Some(n.into().as_f64());
        self
    }
    pub fn multiple_of(mut self, n: impl Into<Num>) -> Self {
        self.c.multiple_of = Some(n.into().as_f64());
        self
    }
}

/// pydantic-core `str_as_float`: Rust float syntax after trimming, then once
/// more with digit-separating underscores removed.
fn str_as_float(s: &str) -> Result<f64, Fail> {
    let t = s.trim();
    if let Ok(f) = t.parse::<f64>() {
        return Ok(f);
    }
    let b = t.as_bytes();
    let underscores_ok = t.contains('_')
        && b.iter().enumerate().all(|(i, &c)| {
            c != b'_' || (i > 0 && i + 1 < b.len() && b[i - 1].is_ascii_digit() && b[i + 1].is_ascii_digit())
        });
    if underscores_ok && let Ok(f) = t.replace('_', "").parse::<f64>() {
        return Ok(f);
    }
    Err(Fail::float_parsing())
}

impl Coerce for FloatC {
    type Out = f64;

    fn coerce(&self, input: &Input<'_>) -> Result<f64, Fail> {
        let f = match input {
            Input::Json(Value::Number(n)) => n.as_f64().ok_or_else(Fail::float_type)?,
            Input::Json(Value::Bool(b)) => *b as u8 as f64,
            other => str_as_float(other.text().ok_or_else(Fail::float_type)?)?,
        };
        if let Some(m) = self.multiple_of {
            let rem = f % m;
            let threshold = f.abs() / 1e9;
            if rem.abs() > threshold && (rem - m).abs() > threshold {
                return Err(Fail::multiple_of(Num::Float(m)));
            }
        }
        if let Some(n) = self.le.filter(|&n| f > n) {
            return Err(Fail::less_than_equal(Num::Float(n)));
        }
        if let Some(n) = self.lt.filter(|&n| f >= n) {
            return Err(Fail::less_than(Num::Float(n)));
        }
        if let Some(n) = self.ge.filter(|&n| f < n) {
            return Err(Fail::greater_than_equal(Num::Float(n)));
        }
        if let Some(n) = self.gt.filter(|&n| f <= n) {
            return Err(Fail::greater_than(Num::Float(n)));
        }
        Ok(f)
    }

    fn placeholder(&self) -> f64 {
        0.0
    }
}

// --- strings ---------------------------------------------------------------

type Matcher = Box<dyn Fn(&str) -> bool + Send + Sync>;

#[derive(Default)]
pub struct StrC {
    min: Option<usize>,
    max: Option<usize>,
    pattern: Option<(String, Matcher)>,
}

impl<'a> Typed<'a, StrC> {
    /// `min_length` (characters, i.e. code points).
    pub fn min_len(mut self, n: usize) -> Self {
        self.c.min = Some(n);
        self
    }
    pub fn max_len(mut self, n: usize) -> Self {
        self.c.max = Some(n);
        self
    }
    /// `pattern=...`: `text` is the regex as written in Python (for the
    /// message), `matches` decides (pydantic searches, so honour the anchors).
    pub fn pattern(mut self, text: &str, matches: impl Fn(&str) -> bool + Send + Sync + 'static) -> Self {
        self.c.pattern = Some((text.to_owned(), Box::new(matches)));
        self
    }
    /// The common `pattern="^(a|b|c)$"`.
    pub fn one_of_pattern(self, choices: &'static [&'static str]) -> Self {
        let text = format!("^({})$", choices.join("|"));
        self.pattern(&text, move |s| choices.contains(&s))
    }
}

impl Coerce for StrC {
    type Out = String;

    fn coerce(&self, input: &Input<'_>) -> Result<String, Fail> {
        let s = input.text().ok_or_else(Fail::string_type)?;
        let len = s.chars().count();
        if let Some(min) = self.min.filter(|&m| len < m) {
            return Err(Fail::string_too_short(min));
        }
        if let Some(max) = self.max.filter(|&m| len > m) {
            return Err(Fail::string_too_long(max));
        }
        if let Some((text, matches)) = &self.pattern
            && !matches(s)
        {
            return Err(Fail::string_pattern_mismatch(text));
        }
        Ok(s.to_owned())
    }

    fn placeholder(&self) -> String {
        String::new()
    }
}

// --- booleans --------------------------------------------------------------

pub struct BoolC;

impl Coerce for BoolC {
    type Out = bool;

    fn coerce(&self, input: &Input<'_>) -> Result<bool, Fail> {
        match input {
            Input::Json(Value::Bool(b)) => Ok(*b),
            Input::Json(Value::Number(n)) => match (n.as_i64(), n.as_u64(), n.as_f64()) {
                (Some(0), _, _) => Ok(false),
                (Some(1), _, _) => Ok(true),
                (Some(_), _, _) | (_, Some(_), _) => Err(Fail::bool_parsing()),
                (_, _, Some(0.0)) => Ok(false),
                (_, _, Some(1.0)) => Ok(true),
                _ => Err(Fail::bool_type()),
            },
            other => {
                let s = other.text().ok_or_else(Fail::bool_type)?;
                match s.to_ascii_lowercase().as_str() {
                    "0" | "off" | "f" | "false" | "n" | "no" => Ok(false),
                    "1" | "on" | "t" | "true" | "y" | "yes" => Ok(true),
                    _ => Err(Fail::bool_parsing()),
                }
            }
        }
    }

    fn placeholder(&self) -> bool {
        false
    }
}

// --- literals / enums ------------------------------------------------------

pub struct ChoiceC {
    choices: &'static [&'static str],
    is_enum: bool,
}

impl Coerce for ChoiceC {
    type Out = String;

    fn coerce(&self, input: &Input<'_>) -> Result<String, Fail> {
        match input.text() {
            Some(s) if self.choices.contains(&s) => Ok(s.to_owned()),
            _ if self.is_enum => Err(Fail::enum_error(self.choices)),
            _ => Err(Fail::literal_error(self.choices)),
        }
    }

    fn placeholder(&self) -> String {
        String::new()
    }
}

// --- ObjectId / Any / custom -----------------------------------------------

pub struct ObjectIdC;

impl Coerce for ObjectIdC {
    type Out = ObjectId;

    fn coerce(&self, input: &Input<'_>) -> Result<ObjectId, Fail> {
        input.text().and_then(|s| ObjectId::parse_str(s).ok()).ok_or_else(Fail::object_id)
    }

    fn placeholder(&self) -> ObjectId {
        ObjectId::from_bytes([0; 12])
    }
}

pub struct AnyC;

impl Coerce for AnyC {
    type Out = Value;

    fn coerce(&self, input: &Input<'_>) -> Result<Value, Fail> {
        Ok(input.to_value())
    }

    fn placeholder(&self) -> Value {
        Value::Null
    }
}

pub struct CustomC<T, F>(std::cell::Cell<Option<F>>, PhantomData<T>);

impl<T: Default, F: FnOnce(&Input<'_>) -> Result<T, Fail>> Coerce for CustomC<T, F> {
    type Out = T;

    fn coerce(&self, input: &Input<'_>) -> Result<T, Fail> {
        match self.0.take() {
            Some(f) => f(input),
            None => Ok(T::default()),
        }
    }

    fn placeholder(&self) -> T {
        T::default()
    }
}

// --- models, lists, dicts --------------------------------------------------

/// The fields of one JSON object (a pydantic model).
pub struct Model<'a> {
    v: &'a mut Validator,
    loc: Loc,
    obj: &'a Map<String, Value>,
    value: &'a Value,
    errors_before: usize,
}

impl Model<'_> {
    /// The field `name` (an alias, if the Python field has one).
    pub fn field(&mut self, name: &str) -> Field<'_> {
        let mut loc = self.loc.clone();
        loc.push(name.into());
        let input = match self.obj.get(name) {
            Some(v) => Input::Json(v),
            None => Input::Missing(Some(self.value)),
        };
        Field { v: self.v, loc, input }
    }

    /// The raw value of a key.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.obj.get(name)
    }

    /// True while no field of this model has failed (gate for
    /// `model_validator(mode="after")` logic).
    pub fn ok(&self) -> bool {
        self.v.errors.len() == self.errors_before
    }

    /// A model-level error (located at the model, input = the object).
    pub fn fail(&mut self, fail: Fail) {
        self.v.push(self.loc.clone(), self.value.clone(), fail);
    }
}

pub struct ModelK;
pub struct ListK;
pub struct DictK;

/// A model, list or dict field; finish with a terminal like [`Typed`].
pub struct Nested<'a, T, F, K> {
    field: Field<'a>,
    f: F,
    min: Option<usize>,
    max: Option<usize>,
    _k: K,
    _t: PhantomData<fn() -> T>,
}

macro_rules! nested_terminals {
    ($out:ty) => {
        /// No default: absence is a `missing` error.
        pub fn required(self) -> $out {
            match self.run(false) {
                Got::Value(v) => v,
                _ => Default::default(),
            }
        }

        /// `x: T = default`.
        pub fn default(self, default: $out) -> $out {
            match self.run_missing_ok(false) {
                Got::Value(v) => v,
                Got::Missing => default,
                _ => Default::default(),
            }
        }

        /// `x: T | None = None`.
        pub fn optional(self) -> Option<$out> {
            match self.run_missing_ok(true) {
                Got::Value(v) => Some(v),
                _ => None,
            }
        }

        /// `x: T | None` with no default.
        pub fn nullable(self) -> Option<$out> {
            match self.run(true) {
                Got::Value(v) => Some(v),
                _ => None,
            }
        }
    };
}

impl<'a, T: Default, F: FnOnce(&mut Model<'_>) -> T> Nested<'a, T, F, ModelK> {
    fn run_missing_ok(self, null_is_none: bool) -> Got<T> {
        let Nested { field, f, .. } = self;
        let Field { v, loc, input } = field;
        match input {
            Input::Missing(_) => Got::Missing,
            Input::Json(Value::Null) if null_is_none => Got::Null,
            Input::Json(value @ Value::Object(obj)) => {
                let errors_before = v.errors.len();
                let mut m = Model { v, loc, obj, value, errors_before };
                let out = f(&mut m);
                if m.ok() { Got::Value(out) } else { Got::Failed }
            }
            other => {
                v.push(loc, other.to_value(), Fail::model_attributes_type());
                Got::Failed
            }
        }
    }

    fn run(mut self, null_is_none: bool) -> Got<T> {
        if self.field.is_missing() {
            push_missing(&mut self.field);
            return Got::Failed;
        }
        self.run_missing_ok(null_is_none)
    }

    nested_terminals!(T);
}

impl<'a, T, F: FnMut(Field<'_>) -> T> Nested<'a, T, F, ListK> {
    pub fn min_len(mut self, n: usize) -> Self {
        self.min = Some(n);
        self
    }

    pub fn max_len(mut self, n: usize) -> Self {
        self.max = Some(n);
        self
    }

    fn run_missing_ok(self, null_is_none: bool) -> Got<Vec<T>> {
        let Nested { field, mut f, min, max, .. } = self;
        let Field { v, loc, input } = field;
        let items: Vec<Input<'_>> = match &input {
            Input::Missing(_) => return Got::Missing,
            Input::Json(Value::Null) if null_is_none => return Got::Null,
            Input::Json(Value::Array(a)) => a.iter().map(Input::Json).collect(),
            Input::Strs(s) => s.iter().map(|s| Input::Str(s.clone())).collect(),
            other => {
                v.push(loc, other.to_value(), Fail::list_type());
                return Got::Failed;
            }
        };
        if let Some(max) = max.filter(|&m| items.len() > m) {
            v.push(loc, input.to_value(), Fail::too_long(max, items.len()));
            return Got::Failed;
        }
        let errors_before = v.errors.len();
        let mut out = Vec::with_capacity(items.len());
        for (i, item) in items.into_iter().enumerate() {
            let mut l = loc.clone();
            l.push(i.into());
            out.push(f(Field { v: &mut *v, loc: l, input: item }));
        }
        if v.errors.len() != errors_before {
            return Got::Failed;
        }
        if let Some(min) = min.filter(|&m| out.len() < m) {
            v.push(loc, input.to_value(), Fail::too_short(min, out.len()));
            return Got::Failed;
        }
        Got::Value(out)
    }

    fn run(mut self, null_is_none: bool) -> Got<Vec<T>> {
        if self.field.is_missing() {
            push_missing(&mut self.field);
            return Got::Failed;
        }
        self.run_missing_ok(null_is_none)
    }

    nested_terminals!(Vec<T>);
}

impl<'a, T, F: FnMut(Field<'_>) -> T> Nested<'a, T, F, DictK> {
    fn run_missing_ok(self, null_is_none: bool) -> Got<Vec<(String, T)>> {
        let Nested { field, mut f, .. } = self;
        let Field { v, loc, input } = field;
        let obj = match input {
            Input::Missing(_) => return Got::Missing,
            Input::Json(Value::Null) if null_is_none => return Got::Null,
            Input::Json(Value::Object(o)) => o,
            other => {
                v.push(loc, other.to_value(), Fail::dict_type());
                return Got::Failed;
            }
        };
        let errors_before = v.errors.len();
        let mut out = Vec::with_capacity(obj.len());
        for (k, item) in obj {
            let mut l = loc.clone();
            l.push(k.as_str().into());
            out.push((k.clone(), f(Field { v: &mut *v, loc: l, input: Input::Json(item) })));
        }
        if v.errors.len() != errors_before { Got::Failed } else { Got::Value(out) }
    }

    fn run(mut self, null_is_none: bool) -> Got<Vec<(String, T)>> {
        if self.field.is_missing() {
            push_missing(&mut self.field);
            return Got::Failed;
        }
        self.run_missing_ok(null_is_none)
    }

    nested_terminals!(Vec<(String, T)>);
}

// ---------------------------------------------------------------------------
// Request inputs

/// `urllib.parse.parse_qsl(qs, keep_blank_values=True)` as Starlette's
/// `QueryParams` runs it: `&`-separated, `+` is a space, `%XX` decoded as
/// UTF-8 with replacement, invalid escapes kept literally.
pub fn parse_qsl(qs: &str) -> Vec<(String, String)> {
    qs.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (n, v) = pair.split_once('=').unwrap_or((pair, ""));
            (unquote_plus(n), unquote_plus(v))
        })
        .collect()
}

fn unquote_plus(s: &str) -> String {
    let s = s.replace('+', " ");
    if !s.contains('%') {
        return s;
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |j: usize| b.get(j).and_then(|c| (*c as char).to_digit(16));
        if b[i] == b'%'
            && let (Some(h), Some(l)) = (hex(i + 1), hex(i + 2))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Path, query and header values of a request, plus the error collector.
pub struct Params<'r> {
    path: Vec<(String, String)>,
    query: Vec<(String, String)>,
    headers: &'r HeaderMap,
    pub v: Validator,
}

impl<'r> Params<'r> {
    pub fn new(path: Vec<(String, String)>, query: Option<&str>, headers: &'r HeaderMap) -> Self {
        Params { path, query: query.map(parse_qsl).unwrap_or_default(), headers, v: Validator::default() }
    }

    async fn from_parts<S: Send + Sync>(parts: &'r mut Parts, state: &S) -> Params<'r> {
        let path = match RawPathParams::from_request_parts(parts, state).await {
            Ok(raw) => raw.iter().map(|(k, v)| (k.to_owned(), v.to_owned())).collect(),
            Err(_) => Vec::new(),
        };
        let query = parts.uri.query().map(str::to_owned);
        Params::new(path, query.as_deref(), &parts.headers)
    }

    fn field(&mut self, loc0: &str, name: &str, input: Option<Cow<'_, str>>) -> Field<'_> {
        let input = match input {
            Some(s) => Input::Str(Cow::Owned(s.into_owned())),
            None => Input::Missing(None),
        };
        Field { v: &mut self.v, loc: vec![loc0.into(), name.into()], input }
    }

    /// A path parameter (`loc = ["path", name]`).
    pub fn path(&mut self, name: &str) -> Field<'_> {
        let input = self.path.iter().find(|(k, _)| k == name).map(|(_, v)| Cow::Owned(v.clone()));
        self.field("path", name, input)
    }

    /// A query parameter; repeated keys give the last value, as Starlette's
    /// `QueryParams.get` does.
    pub fn query(&mut self, name: &str) -> Field<'_> {
        let input = self.query.iter().rev().find(|(k, _)| k == name).map(|(_, v)| Cow::Owned(v.clone()));
        self.field("query", name, input)
    }

    /// A `list[...]` query parameter: every value; none at all is missing.
    pub fn query_list(&mut self, name: &str) -> Field<'_> {
        let values: Vec<Cow<'_, str>> =
            self.query.iter().filter(|(k, _)| k == name).map(|(_, v)| Cow::Owned(v.clone())).collect();
        let input = if values.is_empty() { Input::Missing(None) } else { Input::Strs(values) };
        Field { v: &mut self.v, loc: vec!["query".into(), name.into()], input }
    }

    /// A header parameter (first value, latin-1). `name` is the header name
    /// FastAPI derives (`x_internal_key` -> `x-internal-key`).
    pub fn header(&mut self, name: &str) -> Field<'_> {
        let input = self.headers.get(name).map(|v| Cow::Owned(request_id::latin1(v.as_bytes())));
        self.field("header", name, input)
    }

    /// The raw query pairs.
    pub fn query_pairs(&self) -> &[(String, String)] {
        &self.query
    }
}

/// Path/query/header parameters read with [`Params`].
pub trait FromParams: Sized {
    fn from_params(p: &mut Params<'_>) -> Self;
}

/// A JSON body (`loc = ["body"]`); usually `body.model(|m| Some(..)).required()`.
/// `None` without a recorded error is a server bug and becomes a 500.
pub trait FromBody: Sized {
    fn from_body(body: Field<'_>) -> Option<Self>;
}

async fn params<S: Send + Sync, T: FromParams>(parts: &mut Parts, state: &S) -> Result<T, ApiError> {
    let mut p = Params::from_parts(parts, state).await;
    let value = T::from_params(&mut p);
    Ok(p.v.finish(value)?)
}

/// Query parameters (and any path/header values the type reads).
pub struct Query<T>(pub T);

impl<S: Send + Sync, T: FromParams> FromRequestParts<S> for Query<T> {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, ApiError> {
        params(parts, state).await.map(Query)
    }
}

/// Path parameters (and any query/header values the type reads).
pub struct Path<T>(pub T);

impl<S: Send + Sync, T: FromParams> FromRequestParts<S> for Path<T> {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, ApiError> {
        params(parts, state).await.map(Path)
    }
}

/// The request body as FastAPI reads it.
#[derive(Debug)]
pub enum BodyInput {
    /// Empty, or JSON `null`.
    Missing,
    Json(Value),
    /// A non-JSON content type: pydantic sees the bytes (as text).
    Raw(Value),
}

impl BodyInput {
    pub fn input(&self) -> Input<'_> {
        match self {
            BodyInput::Missing => Input::Missing(None),
            BodyInput::Json(v) | BodyInput::Raw(v) => Input::Json(v),
        }
    }
}

/// `application/json` or `application/*+json`, judged the way FastAPI does
/// (`email.message.Message` content-type parsing).
pub fn is_json_content_type(value: &str) -> bool {
    let ctype = value.split(';').next().unwrap_or("").trim().to_lowercase();
    if ctype.matches('/').count() != 1 {
        return false;
    }
    let (main, sub) = ctype.split_once('/').unwrap_or_default();
    main == "application" && (sub == "json" || sub.ends_with("+json"))
}

fn body_parse_error() -> ApiError {
    ApiError::http(StatusCode::BAD_REQUEST, Some("There was an error parsing the body"))
}

/// Reads and decodes a body like FastAPI's request handler: malformed JSON
/// is a 422 `json_invalid` raised before any dependency runs.
pub async fn read_body(headers: &HeaderMap, body: Body) -> Result<BodyInput, ApiError> {
    let bytes = axum::body::to_bytes(body, usize::MAX).await.map_err(|_| body_parse_error())?;
    if bytes.is_empty() {
        return Ok(BodyInput::Missing);
    }
    let is_json = match headers.get(header::CONTENT_TYPE) {
        Some(ct) if !ct.is_empty() => is_json_content_type(&request_id::latin1(ct.as_bytes())),
        _ => false,
    };
    if !is_json {
        return Ok(BodyInput::Raw(Value::String(String::from_utf8_lossy(&bytes).into_owned())));
    }
    match pyjson::loads(&bytes) {
        Ok(Value::Null) => Ok(BodyInput::Missing),
        Ok(v) => Ok(BodyInput::Json(v)),
        Err(LoadsError::Syntax { msg, pos }) => Err(ValidationError(vec![ErrorItem::json_invalid(pos, msg)]).into()),
        Err(LoadsError::Decode) => Err(body_parse_error()),
        Err(LoadsError::NonFinite) => Err(ApiError::internal()),
    }
}

fn body_value<T: FromBody>(v: &mut Validator, body: &BodyInput) -> Option<T> {
    T::from_body(Field::new(v, vec!["body".into()], body.input()))
}

fn finish_body<T>(v: Validator, value: Option<T>) -> Result<T, ApiError> {
    match v.finish(value)? {
        Some(t) => Ok(t),
        None => Err(ApiError::internal()),
    }
}

/// Validates a body [`read_body`] already read, for routes that run their
/// dependencies in between.
pub fn validate<T: FromBody>(body: &BodyInput) -> Result<T, ApiError> {
    let mut v = Validator::default();
    let value = body_value::<T>(&mut v, body);
    finish_body(v, value)
}

/// A JSON body validated by `T`.
pub struct Json<T>(pub T);

impl<S: Send + Sync, T: FromBody> FromRequest<S> for Json<T> {
    type Rejection = ApiError;

    async fn from_request(req: Request, _state: &S) -> Result<Self, ApiError> {
        let (parts, body) = req.into_parts();
        let body = read_body(&parts.headers, body).await?;
        validate(&body).map(Json)
    }
}

/// Parameters and body together, with FastAPI's error order: a JSON syntax
/// error alone first, else parameter errors followed by body errors.
pub struct Checked<P, B>(pub P, pub B);

impl<S: Send + Sync, P: FromParams, B: FromBody> FromRequest<S> for Checked<P, B> {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, ApiError> {
        let (mut parts, body) = req.into_parts();
        let body = read_body(&parts.headers, body).await?;
        let mut p = Params::from_parts(&mut parts, state).await;
        let params = P::from_params(&mut p);
        let mut v = p.v;
        let value = body_value::<B>(&mut v, &body);
        finish_body(v, value.map(|b| (params, b))).map(|(p, b)| Checked(p, b))
    }
}

/// A body behind dependencies, in FastAPI's order: read and decoded first (a
/// syntax error is a 422 before any dependency runs), then the dependency
/// extractor, then validation.
pub struct DepsThenBody<D, B>(pub D, pub B);

impl<S, D, B> FromRequest<S> for DepsThenBody<D, B>
where
    S: Send + Sync,
    D: FromRequestParts<S> + Send,
    D::Rejection: IntoResponse,
    B: FromBody + Send,
{
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Response> {
        let (mut parts, body) = req.into_parts();
        let body = read_body(&parts.headers, body).await.map_err(IntoResponse::into_response)?;
        let deps = D::from_request_parts(&mut parts, state).await.map_err(IntoResponse::into_response)?;
        let value = validate::<B>(&body).map_err(IntoResponse::into_response)?;
        Ok(DepsThenBody(deps, value))
    }
}
