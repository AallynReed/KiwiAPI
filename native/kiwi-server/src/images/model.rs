//! `app/images/models.py` + `schemas.py`: the `image_designs` document, the
//! request body and the DTO.
//!
//! A layer's float fields keep Python's int-or-float identity: Beanie
//! validates a created or loaded design (every one a float), but `PUT` assigns
//! the body's `model_dump()` dicts unvalidated, so a defaulted `x`/`y`/`w`/`h`
//! stays the int it was declared as, in Mongo and in the response.

use chrono::{DateTime, Utc};
use mongodb::bson::{Bson, Document, doc, oid::ObjectId};
use serde_json::{Value, json};

use crate::validation::{Field, FromBody};

pub const COLLECTION: &str = "image_designs";
pub const MIN_DIM: i64 = 32;
pub const MAX_DIM: i64 = 1600;
pub const MAX_LAYERS: usize = 40;
pub const MAX_DESIGNS_PER_USER: u64 = 60;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Num {
    Int(i64),
    Float(f64),
}

impl Num {
    pub fn f(self) -> f64 {
        match self {
            Num::Int(i) => i as f64,
            Num::Float(f) => f,
        }
    }

    fn as_float(self) -> Num {
        Num::Float(self.f())
    }

    fn json(self) -> Value {
        match self {
            Num::Int(i) => i.into(),
            Num::Float(f) => serde_json::Number::from_f64(f).map_or(Value::Null, Value::Number),
        }
    }

    fn bson(self) -> Bson {
        match self {
            Num::Int(i) => py_int(i),
            Num::Float(f) => Bson::Double(f),
        }
    }
}

/// pymongo's encoding of a Python int.
pub fn py_int(i: i64) -> Bson {
    match i32::try_from(i) {
        Ok(v) => Bson::Int32(v),
        Err(_) => Bson::Int64(i),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Background {
    pub kind: String,
    pub color1: String,
    pub color2: String,
    pub angle: i64,
    pub image_sha: Option<String>,
    pub fit: String,
}

impl Default for Background {
    fn default() -> Self {
        Background {
            kind: "solid".into(),
            color1: "#0d1117".into(),
            color2: "#16213e".into(),
            angle: 90,
            image_sha: None,
            fit: "cover".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub kind: String,
    pub x: Num,
    pub y: Num,
    pub opacity: Num,
    pub text: String,
    pub font_size: i64,
    pub color: String,
    pub bold: bool,
    pub align: String,
    pub max_width: Option<i64>,
    pub w: Num,
    pub h: Num,
    pub radius: i64,
    pub image_sha: Option<String>,
}

impl Default for Layer {
    fn default() -> Self {
        Layer {
            kind: "text".into(),
            x: Num::Int(0),
            y: Num::Int(0),
            opacity: Num::Float(1.0),
            text: String::new(),
            font_size: 32,
            color: "#ffffff".into(),
            bold: false,
            align: "left".into(),
            max_width: None,
            w: Num::Int(120),
            h: Num::Int(48),
            radius: 0,
            image_sha: None,
        }
    }
}

impl Layer {
    /// After pydantic validation every float field is a float.
    fn validated(mut self) -> Self {
        self.x = self.x.as_float();
        self.y = self.y.as_float();
        self.opacity = self.opacity.as_float();
        self.w = self.w.as_float();
        self.h = self.h.as_float();
        self
    }

    fn json(&self) -> Value {
        json!({
            "type": self.kind,
            "x": self.x.json(),
            "y": self.y.json(),
            "opacity": self.opacity.json(),
            "text": self.text,
            "font_size": self.font_size,
            "color": self.color,
            "bold": self.bold,
            "align": self.align,
            "max_width": self.max_width,
            "w": self.w.json(),
            "h": self.h.json(),
            "radius": self.radius,
            "image_sha": self.image_sha,
        })
    }

    fn bson(&self) -> Document {
        doc! {
            "type": &self.kind,
            "x": self.x.bson(),
            "y": self.y.bson(),
            "opacity": self.opacity.bson(),
            "text": &self.text,
            "font_size": py_int(self.font_size),
            "color": &self.color,
            "bold": self.bold,
            "align": &self.align,
            "max_width": self.max_width.map_or(Bson::Null, py_int),
            "w": self.w.bson(),
            "h": self.h.bson(),
            "radius": py_int(self.radius),
            "image_sha": opt(&self.image_sha),
        }
    }
}

impl Background {
    fn json(&self) -> Value {
        json!({
            "type": self.kind,
            "color1": self.color1,
            "color2": self.color2,
            "angle": self.angle,
            "image_sha": self.image_sha,
            "fit": self.fit,
        })
    }

    fn bson(&self) -> Document {
        doc! {
            "type": &self.kind,
            "color1": &self.color1,
            "color2": &self.color2,
            "angle": py_int(self.angle),
            "image_sha": opt(&self.image_sha),
            "fit": &self.fit,
        }
    }
}

fn opt(v: &Option<String>) -> Bson {
    v.as_ref().map_or(Bson::Null, |s| Bson::String(s.clone()))
}

/// `DesignBody`.
#[derive(Clone, Debug, Default)]
pub struct DesignBody {
    pub name: String,
    pub width: i64,
    pub height: i64,
    pub background: Background,
    pub layers: Vec<Layer>,
    pub bind_type: Option<String>,
}

const LAYER_TYPES: &[&str] = &["text", "rect", "image"];
const ALIGNS: &[&str] = &["left", "center", "right"];
const BG_TYPES: &[&str] = &["solid", "gradient", "image"];
const FITS: &[&str] = &["cover", "contain", "stretch"];

/// A given float validates to a float; an absent one keeps the declared
/// (maybe int) default.
fn float(f: Field<'_>, default: Num) -> Num {
    if f.is_missing() { default } else { Num::Float(f.float().required()) }
}

pub fn validate_layer(f: Field<'_>) -> Layer {
    f.model(|m| {
        let d = Layer::default();
        Layer {
            kind: m.field("type").literal(LAYER_TYPES).default(d.kind),
            x: float(m.field("x"), d.x),
            y: float(m.field("y"), d.y),
            opacity: float(m.field("opacity"), d.opacity),
            text: m.field("text").str().default(d.text),
            font_size: m.field("font_size").int().default(d.font_size),
            color: m.field("color").str().default(d.color),
            bold: m.field("bold").bool().default(d.bold),
            align: m.field("align").literal(ALIGNS).default(d.align),
            max_width: m.field("max_width").int().optional(),
            w: float(m.field("w"), d.w),
            h: float(m.field("h"), d.h),
            radius: m.field("radius").int().default(d.radius),
            image_sha: m.field("image_sha").str().optional(),
        }
    })
    .required()
}

fn validate_background(f: Field<'_>) -> Background {
    f.model(|m| {
        let d = Background::default();
        Background {
            kind: m.field("type").literal(BG_TYPES).default(d.kind),
            color1: m.field("color1").str().default(d.color1),
            color2: m.field("color2").str().default(d.color2),
            angle: m.field("angle").int().default(d.angle),
            image_sha: m.field("image_sha").str().optional(),
            fit: m.field("fit").literal(FITS).default(d.fit),
        }
    })
    .default(Background::default())
}

impl FromBody for DesignBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(DesignBody {
                name: m.field("name").str().max_len(120).default(String::new()),
                width: m.field("width").int().saturating().default(600),
                height: m.field("height").int().saturating().default(240),
                background: validate_background(m.field("background")),
                layers: m.field("layers").list(validate_layer).default(Vec::new()),
                bind_type: m.field("bind_type").str().optional(),
            })
        })
        .required()
    }
}

/// An `ImageDesign`.
#[derive(Clone, Debug, PartialEq)]
pub struct Design {
    pub id: ObjectId,
    pub owner_id: ObjectId,
    pub name: String,
    pub width: i64,
    pub height: i64,
    pub background: Background,
    pub layers: Vec<Layer>,
    pub bind_type: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn b_str(d: &Document, k: &str, default: &str) -> String {
    match d.get(k) {
        Some(Bson::String(s)) => s.clone(),
        _ => default.to_owned(),
    }
}

fn b_opt_str(d: &Document, k: &str) -> Option<String> {
    match d.get(k) {
        Some(Bson::String(s)) => Some(s.clone()),
        _ => None,
    }
}

fn b_int(d: &Document, k: &str, default: i64) -> i64 {
    match d.get(k) {
        Some(Bson::Int32(i)) => *i as i64,
        Some(Bson::Int64(i)) => *i,
        Some(Bson::Double(f)) if f.fract() == 0.0 && f.is_finite() => *f as i64,
        Some(Bson::Boolean(b)) => *b as i64,
        _ => default,
    }
}

fn b_opt_int(d: &Document, k: &str) -> Option<i64> {
    match d.get(k) {
        None | Some(Bson::Null) => None,
        Some(_) => Some(b_int(d, k, 0)),
    }
}

fn b_float(d: &Document, k: &str, default: Num) -> Num {
    match d.get(k) {
        Some(Bson::Double(f)) => Num::Float(*f),
        Some(Bson::Int32(i)) => Num::Float(*i as f64),
        Some(Bson::Int64(i)) => Num::Float(*i as f64),
        _ => default.as_float(),
    }
}

fn b_bool(d: &Document, k: &str, default: bool) -> bool {
    match d.get(k) {
        Some(Bson::Boolean(b)) => *b,
        Some(Bson::Int32(i)) => *i != 0,
        Some(Bson::Int64(i)) => *i != 0,
        _ => default,
    }
}

fn b_dt(d: &Document, k: &str) -> DateTime<Utc> {
    match d.get(k) {
        Some(Bson::DateTime(t)) => crate::dt::from_bson(*t),
        _ => DateTime::<Utc>::UNIX_EPOCH,
    }
}

impl Layer {
    pub fn from_bson(d: &Document) -> Layer {
        let df = Layer::default();
        Layer {
            kind: b_str(d, "type", "text"),
            x: b_float(d, "x", df.x),
            y: b_float(d, "y", df.y),
            opacity: b_float(d, "opacity", df.opacity),
            text: b_str(d, "text", ""),
            font_size: b_int(d, "font_size", 32),
            color: b_str(d, "color", "#ffffff"),
            bold: b_bool(d, "bold", false),
            align: b_str(d, "align", "left"),
            max_width: b_opt_int(d, "max_width"),
            w: b_float(d, "w", df.w),
            h: b_float(d, "h", df.h),
            radius: b_int(d, "radius", 0),
            image_sha: b_opt_str(d, "image_sha"),
        }
    }
}

impl Background {
    pub fn from_bson(d: &Document) -> Background {
        Background {
            kind: b_str(d, "type", "solid"),
            color1: b_str(d, "color1", "#0d1117"),
            color2: b_str(d, "color2", "#16213e"),
            angle: b_int(d, "angle", 90),
            image_sha: b_opt_str(d, "image_sha"),
            fit: b_str(d, "fit", "cover"),
        }
    }
}

impl Design {
    /// Beanie's load: pydantic validation of the stored document.
    pub fn from_doc(d: &Document) -> Option<Design> {
        let id = d.get_object_id("_id").ok()?;
        let owner_id = d.get_object_id("owner_id").ok()?;
        let background = match d.get("background") {
            Some(Bson::Document(b)) => Background::from_bson(b),
            _ => Background::default(),
        };
        let layers = match d.get("layers") {
            Some(Bson::Array(a)) => a.iter().filter_map(Bson::as_document).map(Layer::from_bson).collect(),
            _ => Vec::new(),
        };
        Some(Design {
            id,
            owner_id,
            name: b_str(d, "name", ""),
            width: b_int(d, "width", 600),
            height: b_int(d, "height", 240),
            background,
            layers,
            bind_type: b_opt_str(d, "bind_type"),
            created_at: b_dt(d, "created_at"),
            updated_at: b_dt(d, "updated_at"),
        })
    }

    /// `ImageDesign(owner_id=..., **body.model_dump())`, validated.
    pub fn create(id: ObjectId, owner_id: ObjectId, body: &DesignBody, now: DateTime<Utc>) -> Design {
        Design {
            id,
            owner_id,
            name: body.name.clone(),
            width: body.width,
            height: body.height,
            background: body.background.clone(),
            layers: body.layers.iter().cloned().map(Layer::validated).collect(),
            bind_type: body.bind_type.clone(),
            created_at: now,
            updated_at: now,
        }
    }

    /// `setattr` of every body field (unvalidated).
    pub fn assign(&mut self, body: &DesignBody) {
        self.name = body.name.clone();
        self.width = body.width;
        self.height = body.height;
        self.background = body.background.clone();
        self.layers = body.layers.clone();
        self.bind_type = body.bind_type.clone();
    }

    /// `_clamp`.
    pub fn clamp(&mut self, bindable: impl Fn(&str) -> bool) {
        self.width = self.width.clamp(MIN_DIM, MAX_DIM);
        self.height = self.height.clamp(MIN_DIM, MAX_DIM);
        self.layers.truncate(MAX_LAYERS);
        if self.bind_type.as_deref().is_some_and(|b| !b.is_empty() && !bindable(b)) {
            self.bind_type = None;
        }
    }

    /// The stored fields, in model order (no `_id`).
    pub fn fields(&self) -> Document {
        doc! {
            "owner_id": self.owner_id,
            "name": &self.name,
            "width": py_int(self.width),
            "height": py_int(self.height),
            "background": self.background.bson(),
            "layers": self.layers.iter().map(|l| Bson::Document(l.bson())).collect::<Vec<_>>(),
            "bind_type": opt(&self.bind_type),
            "created_at": crate::dt::to_bson(self.created_at),
            "updated_at": crate::dt::to_bson(self.updated_at),
        }
    }

    /// `_dto`.
    pub fn dto(&self, asset_url: &str) -> Value {
        json!({
            "id": self.id.to_hex(),
            "name": self.name,
            "width": self.width,
            "height": self.height,
            "background": self.background.json(),
            "layers": self.layers.iter().map(Layer::json).collect::<Vec<_>>(),
            "bind_type": self.bind_type,
            "created_at": crate::dt::pydantic(&self.created_at),
            "updated_at": crate::dt::pydantic(&self.updated_at),
            "render_url": format!("{asset_url}/site/images/{}.png", self.id.to_hex()),
        })
    }
}
