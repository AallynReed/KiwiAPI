//! Store documents read the way Beanie validates them (lax pydantic types,
//! model defaults for missing fields, anything else a 500) and rendered the
//! way `service._doc_out` / the route schemas emit them.

use kiwi_core::store::{Category, Price, PricePoint, Product, Records, State, Texture, is_active, records};
use mongodb::bson::{Bson, Document};
use serde_json::{Map, Number, Value, json};

/// A document the model rejects (a validation error, so a 500 in Python).
#[derive(Debug)]
pub struct Invalid(pub String);

type R<T> = Result<T, Invalid>;

fn bad<T>(key: &str, v: &Bson) -> R<T> {
    Err(Invalid(format!("{key}: unexpected {v:?}")))
}

fn int_of(key: &str, v: &Bson) -> R<i64> {
    match v {
        Bson::Int32(i) => Ok(i64::from(*i)),
        Bson::Int64(i) => Ok(*i),
        Bson::Boolean(b) => Ok(i64::from(*b)),
        Bson::Double(f) if f.is_finite() && f.fract() == 0.0 && f.abs() < 9.2e18 => Ok(*f as i64),
        Bson::String(s) => kiwi_core::store::py::int(s).map_or_else(|| bad(key, v), Ok),
        _ => bad(key, v),
    }
}

fn float_of(key: &str, v: &Bson) -> R<f64> {
    match v {
        Bson::Double(f) => Ok(*f),
        Bson::Int32(i) => Ok(f64::from(*i)),
        Bson::Int64(i) => Ok(*i as f64),
        Bson::Boolean(b) => Ok(f64::from(u8::from(*b))),
        _ => bad(key, v),
    }
}

fn bool_of(key: &str, v: &Bson) -> R<bool> {
    match v {
        Bson::Boolean(b) => Ok(*b),
        Bson::Int32(0) | Bson::Int64(0) => Ok(false),
        Bson::Int32(1) | Bson::Int64(1) => Ok(true),
        Bson::Double(f) if *f == 0.0 || *f == 1.0 => Ok(*f == 1.0),
        _ => bad(key, v),
    }
}

/// `x: str = default` (or required when `default` is `None`).
fn s(d: &Document, key: &str, default: Option<&str>) -> R<String> {
    match (d.get(key), default) {
        (Some(Bson::String(v)), _) => Ok(v.clone()),
        (None, Some(def)) => Ok(def.to_owned()),
        (None, None) => Err(Invalid(format!("{key}: missing"))),
        (Some(v), _) => bad(key, v),
    }
}

fn i(d: &Document, key: &str, default: Option<i64>) -> R<i64> {
    match (d.get(key), default) {
        (Some(v), _) => int_of(key, v),
        (None, Some(def)) => Ok(def),
        (None, None) => Err(Invalid(format!("{key}: missing"))),
    }
}

fn opt_i(d: &Document, key: &str) -> R<Option<i64>> {
    match d.get(key) {
        None | Some(Bson::Null) => Ok(None),
        Some(v) => int_of(key, v).map(Some),
    }
}

fn opt_s(d: &Document, key: &str) -> R<Option<String>> {
    match d.get(key) {
        None | Some(Bson::Null) => Ok(None),
        Some(Bson::String(v)) => Ok(Some(v.clone())),
        Some(v) => bad(key, v),
    }
}

fn b(d: &Document, key: &str, default: bool) -> R<bool> {
    d.get(key).map_or(Ok(default), |v| bool_of(key, v))
}

fn list<T>(d: &Document, key: &str, f: impl Fn(&Bson) -> R<T>) -> R<Vec<T>> {
    match d.get(key) {
        None => Ok(Vec::new()),
        Some(Bson::Array(a)) => a.iter().map(f).collect(),
        Some(v) => bad(key, v),
    }
}

fn sub<'a>(key: &str, v: &'a Bson) -> R<&'a Document> {
    match v {
        Bson::Document(d) => Ok(d),
        _ => bad(key, v),
    }
}

fn price(v: &Bson) -> R<Price> {
    let d = sub("prices", v)?;
    Ok(Price {
        currency: s(d, "currency", None)?,
        cost: match d.get("cost") {
            Some(v) => float_of("cost", v)?,
            None => return Err(Invalid("cost: missing".into())),
        },
        can_purchase: match d.get("can_purchase") {
            Some(v) => bool_of("can_purchase", v)?,
            None => return Err(Invalid("can_purchase: missing".into())),
        },
        monthly: i(d, "monthly", Some(0))?,
        sale: s(d, "sale", Some(""))?,
    })
}

fn texture(v: &Bson) -> R<Texture> {
    let d = sub("textures", v)?;
    Ok(Texture {
        texture: s(d, "texture", None)?,
        x: i(d, "x", Some(0))?,
        y: i(d, "y", Some(0))?,
        text: s(d, "text", Some(""))?,
        overlay: b(d, "overlay", false)?,
    })
}

fn point(v: &Bson) -> R<PricePoint> {
    let d = sub("price_history", v)?;
    Ok(PricePoint {
        ts: i(d, "ts", None)?,
        prices: list(d, "prices", price)?,
        price_string: s(d, "price_string", Some(""))?,
    })
}

fn interval(v: &Bson) -> R<Vec<i64>> {
    match v {
        Bson::Array(a) => a.iter().map(|x| int_of("availability", x)).collect(),
        _ => bad("availability", v),
    }
}

pub fn product(d: &Document) -> R<Product> {
    Ok(Product {
        code: s(d, "code", None)?,
        kind: s(d, "kind", None)?,
        name: s(d, "name", Some(""))?,
        image: s(d, "image", Some(""))?,
        info: s(d, "info", Some(""))?,
        informational: b(d, "informational", false)?,
        tradable: b(d, "tradable", false)?,
        prices: list(d, "prices", price)?,
        price_string: s(d, "price_string", Some(""))?,
        price_string_currency: s(d, "price_string_currency", Some(""))?,
        price_string_sale: s(d, "price_string_sale", Some(""))?,
        promo: s(d, "promo", Some(""))?,
        deal_expires_at: opt_i(d, "deal_expires_at")?,
        interact_label: s(d, "interact_label", Some(""))?,
        interact_enabled: b(d, "interact_enabled", false)?,
        trial_limits: s(d, "trial_limits", Some(""))?,
        class_level: opt_i(d, "class_level")?,
        class_power_rank: opt_i(d, "class_power_rank")?,
        class_shield_frame: opt_i(d, "class_shield_frame")?,
        class_sub_name: s(d, "class_sub_name", Some(""))?,
        class_icon: s(d, "class_icon", Some(""))?,
        textures: list(d, "textures", texture)?,
        loot_title: s(d, "loot_title", Some(""))?,
        loot_body: s(d, "loot_body", Some(""))?,
        categories: list(d, "categories", |v| int_of("categories", v))?,
        first_seen: i(d, "first_seen", Some(0))?,
        last_seen: i(d, "last_seen", Some(0))?,
        price_history: list(d, "price_history", point)?,
        availability: list(d, "availability", interval)?,
    })
}

pub fn category(d: &Document) -> R<Category> {
    Ok(Category {
        index: i(d, "index", None)?,
        label: s(d, "label", Some(""))?,
        icon: s(d, "icon", Some(""))?,
        codes: list(d, "codes", |v| match v {
            Bson::String(c) => Ok(c.clone()),
            other => bad("codes", other),
        })?,
        last_seen: i(d, "last_seen", Some(0))?,
    })
}

pub fn state(d: &Document) -> R<State> {
    Ok(State {
        key: s(d, "key", Some("state"))?,
        last_anchor: opt_i(d, "last_anchor")?,
        title: s(d, "title", Some(""))?,
        product_count: i(d, "product_count", Some(0))?,
        category_count: i(d, "category_count", Some(0))?,
    })
}

/// The `content_sha256` of an `UpdateState` row (the model's other fields validated).
pub fn update_state_sha(d: &Document) -> R<String> {
    s(d, "branch", None)?;
    s(d, "path", None)?;
    opt_i(d, "fnv_hash")?;
    i(d, "size", Some(0))?;
    opt_s(d, "archive")?;
    opt_i(d, "archive_index")?;
    i(d, "last_ordinal", Some(0))?;
    s(d, "content_sha256", None)
}

/// An `UpdateChange` row: (ordinal, type, content_sha256).
pub fn update_change(d: &Document) -> R<(i64, String, Option<String>)> {
    s(d, "branch", None)?;
    s(d, "path", None)?;
    opt_i(d, "fnv_hash")?;
    i(d, "size", Some(0))?;
    Ok((i(d, "ordinal", None)?, s(d, "type", None)?, opt_s(d, "content_sha256")?))
}

/// Which renderer the body goes through: Starlette's `JSONResponse`
/// (`json.dumps(allow_nan=False)`, so a non-finite float is a 500) or a
/// FastAPI response model (pydantic json mode, non-finite floats are null).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flavour {
    Site,
    V1,
}

/// A float the flavour cannot render.
#[derive(Debug)]
pub struct NonFinite;

fn float(x: f64, f: Flavour) -> Result<Value, NonFinite> {
    match Number::from_f64(x) {
        Some(n) => Ok(Value::Number(n)),
        None if f == Flavour::V1 => Ok(Value::Null),
        None => Err(NonFinite),
    }
}

fn or_none(v: &str) -> Value {
    if v.is_empty() { Value::Null } else { Value::String(v.to_owned()) }
}

fn opt(v: Option<i64>) -> Value {
    v.map_or(Value::Null, Value::from)
}

fn price_json(p: &Price, f: Flavour) -> Result<Value, NonFinite> {
    Ok(json!({
        "currency": p.currency,
        "cost": float(p.cost, f)?,
        "can_purchase": p.can_purchase,
        "monthly": p.monthly,
        "sale": p.sale,
    }))
}

fn prices_json(ps: &[Price], f: Flavour) -> Result<Value, NonFinite> {
    Ok(Value::Array(ps.iter().map(|p| price_json(p, f)).collect::<Result<_, _>>()?))
}

fn costs_json(m: &[(String, f64)], f: Flavour) -> Result<Value, NonFinite> {
    if m.is_empty() {
        return Ok(Value::Null);
    }
    let mut out = Map::new();
    for (c, v) in m {
        out.insert(c.clone(), float(*v, f)?);
    }
    Ok(Value::Object(out))
}

fn records_json(r: &Records, f: Flavour) -> Result<Value, NonFinite> {
    Ok(json!({
        "times_available": r.times_available,
        "returns": r.returns,
        "total_days_seen": r.total_days_seen,
        "longest_run_days": r.longest_run_days,
        "first_seen": r.first_seen,
        "last_seen": r.last_seen,
        "currently_active": r.currently_active,
        "gap_days": opt(r.gap_days),
        "price_low": costs_json(&r.price_low, f)?,
        "price_high": costs_json(&r.price_high, f)?,
        "price_changes": r.price_changes,
    }))
}

/// `_doc_out(doc, anchor, history=...)`.
pub fn product_json(p: &Product, anchor: Option<i64>, history: Option<i64>, f: Flavour) -> Result<Value, NonFinite> {
    let mut out = json!({
        "code": p.code,
        "kind": p.kind,
        "name": p.name,
        "image": p.image,
        "info": p.info,
        "informational": p.informational,
        "tradable": p.tradable,
        "prices": prices_json(&p.prices, f)?,
        "price_string": or_none(&p.price_string),
        "price_string_currency": or_none(&p.price_string_currency),
        "price_string_sale": or_none(&p.price_string_sale),
        "promo": or_none(&p.promo),
        "deal_expires_at": opt(p.deal_expires_at),
        "interact_label": or_none(&p.interact_label),
        "interact_enabled": p.interact_enabled,
        "trial_limits": or_none(&p.trial_limits),
        "class_level": opt(p.class_level),
        "class_power_rank": opt(p.class_power_rank),
        "class_sub_name": or_none(&p.class_sub_name),
        "class_icon": or_none(&p.class_icon),
        "textures": p.textures.iter().map(|t| json!({
            "texture": t.texture, "x": t.x, "y": t.y, "text": t.text, "overlay": t.overlay,
        })).collect::<Vec<_>>(),
        "loot_title": or_none(&p.loot_title),
        "loot_body": or_none(&p.loot_body),
        "categories": p.categories,
        "first_seen": p.first_seen,
        "last_seen": p.last_seen,
        "active": is_active(p.last_seen, anchor),
    });
    if let (Some(now), Value::Object(m)) = (history, &mut out) {
        let history = p
            .price_history
            .iter()
            .map(|pt| Ok(json!({"ts": pt.ts, "prices": prices_json(&pt.prices, f)?, "price_string": pt.price_string})))
            .collect::<Result<Vec<_>, NonFinite>>()?;
        m.insert("price_history".into(), Value::Array(history));
        m.insert("availability".into(), json!(p.availability));
        m.insert("records".into(), records_json(&records(p, anchor, now), f)?);
    }
    Ok(out)
}

pub fn category_json(c: &Category, anchor: Option<i64>) -> Value {
    json!({
        "index": c.index,
        "label": c.label,
        "icon": or_none(&c.icon),
        "codes": c.codes,
        "count": c.codes.len(),
        "active": is_active(c.last_seen, anchor),
    })
}

pub fn timeline_item(p: &Product, anchor: Option<i64>) -> Value {
    json!({
        "code": p.code,
        "name": p.name,
        "kind": p.kind,
        "image": or_none(&p.image),
        "availability": p.availability,
        "first_seen": p.first_seen,
        "last_seen": p.last_seen,
        "active": is_active(p.last_seen, anchor),
    })
}
