//! `ModProject` and `ModRelease` as Beanie loads and saves them: every model
//! field in declaration order, pydantic's lax coercions on read, pymongo's
//! encodings on write. `save()` is a `$set` of the whole model, `insert()` the
//! whole model behind `_id`.

use chrono::{DateTime, Utc};
use mongodb::bson::{Bson, Document, oid::ObjectId};
use serde_json::{Map, Value};

use crate::{auth::pyutil::bson_int, dt};

#[derive(Clone, Debug, PartialEq)]
pub enum V {
    Null,
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Oid(ObjectId),
    Dt(DateTime<Utc>),
    Map(Vec<(String, String)>),
    List(Vec<String>),
    Collabs(Vec<(ObjectId, String)>),
}

#[derive(Clone, Copy)]
enum K {
    Str,
    OptStr,
    Int,
    Float,
    Bool,
    Oid,
    OptOid,
    Dt,
    OptDt,
    Map,
    List,
    Lit(&'static [&'static str]),
    OptLit(&'static [&'static str]),
    Collabs,
}

#[derive(Clone, Copy)]
enum D {
    Req,
    Null,
    Str(&'static str),
    Int(i64),
    Float(f64),
    Bool(bool),
    Empty,
    Now,
}

pub struct F {
    name: &'static str,
    kind: K,
    default: D,
}

const fn f(name: &'static str, kind: K, default: D) -> F {
    F { name, kind, default }
}

const VISIBILITY: &[&str] = &["draft", "unlisted", "public", "public_website_only", "hidden"];

pub const PROJECT: &[F] = &[
    f("slug", K::Str, D::Req),
    f("title", K::Str, D::Req),
    f("summary", K::Str, D::Str("")),
    f("description", K::Str, D::Str("")),
    f("readme_text", K::Str, D::Str("")),
    f("warnings", K::Str, D::Str("")),
    f("title_i18n", K::Map, D::Empty),
    f("summary_i18n", K::Map, D::Empty),
    f("description_i18n", K::Map, D::Empty),
    f("readme_i18n", K::Map, D::Empty),
    f("warnings_i18n", K::Map, D::Empty),
    f("tags", K::List, D::Empty),
    f("owner_id", K::OptOid, D::Null),
    f("owner_username", K::Str, D::Str("")),
    f("owner_handle", K::Str, D::Str("")),
    f("collaborators", K::Collabs, D::Empty),
    f("is_stray", K::Bool, D::Bool(false)),
    f("stray_status", K::OptLit(&["approved", "pending", "rejected"]), D::Null),
    f("author", K::Str, D::Str("")),
    f("source", K::OptStr, D::Null),
    f("source_id", K::OptStr, D::Null),
    f("source_url", K::OptStr, D::Null),
    f("source_author_id", K::OptStr, D::Null),
    f("source_file_id", K::OptStr, D::Null),
    f("source_likes", K::Int, D::Int(0)),
    f("uploaded_on_behalf", K::Bool, D::Bool(false)),
    f("issues_enabled", K::Bool, D::Bool(true)),
    f("issue_seq", K::Int, D::Int(0)),
    f("open_issue_count", K::Int, D::Int(0)),
    f("visibility", K::Lit(VISIBILITY), D::Str("draft")),
    f("is_beta", K::Bool, D::Bool(false)),
    f("mode", K::Lit(&["files", "releases"]), D::Str("files")),
    f("source_visibility", K::Lit(&["public", "private"]), D::Str("public")),
    f("default_branch", K::Str, D::Str("main")),
    f("banner_sha", K::OptStr, D::Null),
    f("preview_shas", K::List, D::Empty),
    f("discord_url", K::OptStr, D::Null),
    f("website_url", K::OptStr, D::Null),
    f("donation_urls", K::List, D::Empty),
    f("download_count", K::Int, D::Int(0)),
    f("star_count", K::Int, D::Int(0)),
    f("downloads_7d", K::Int, D::Int(0)),
    f("popularity_score", K::Float, D::Float(0.0)),
    f("hidden_release_branches", K::List, D::Empty),
    f("branch_order", K::List, D::Empty),
    f("forked_from_id", K::OptOid, D::Null),
    f("forked_from_slug", K::OptStr, D::Null),
    f("forked_from_handle", K::OptStr, D::Null),
    f("forked_from_title", K::OptStr, D::Null),
    f("forked_from_owner", K::OptStr, D::Null),
    f("inspired_by_slug", K::OptStr, D::Null),
    f("inspired_by_handle", K::OptStr, D::Null),
    f("inspired_by_title", K::OptStr, D::Null),
    f("inspired_by_owner", K::OptStr, D::Null),
    f("fork_count", K::Int, D::Int(0)),
    f("is_remix", K::Bool, D::Bool(false)),
    f("remix_status", K::Lit(&["pending", "approved", "denied"]), D::Str("pending")),
    f("remix_requested_at", K::OptDt, D::Null),
    f("remix_resolved_at", K::OptDt, D::Null),
    f("remix_resolved_by", K::OptOid, D::Null),
    f("taken_down", K::Bool, D::Bool(false)),
    f("takedown_reason", K::OptStr, D::Null),
    f("created_at", K::Dt, D::Now),
    f("updated_at", K::Dt, D::Now),
    f("last_release_at", K::OptDt, D::Null),
];

pub const RELEASE: &[F] = &[
    f("project_id", K::Oid, D::Req),
    f("owner_id", K::OptOid, D::Null),
    f("tag", K::Str, D::Req),
    f("branch", K::Str, D::Str("main")),
    f("title", K::Str, D::Str("")),
    f("changelog", K::Str, D::Str("")),
    f("title_i18n", K::Map, D::Empty),
    f("changelog_i18n", K::Map, D::Empty),
    f("source_commit_sha", K::OptStr, D::Null),
    f("release_format", K::Lit(&["tmod", "zip"]), D::Str("tmod")),
    f("tmod_sha", K::Str, D::Req),
    f("prior_tmod_shas", K::List, D::Empty),
    f("tmod_size", K::Int, D::Int(0)),
    f("tmod_filename", K::Str, D::Str("mod.tmod")),
    f("tmod_properties", K::Map, D::Empty),
    f("banner_sha", K::OptStr, D::Null),
    f("download_count", K::Int, D::Int(0)),
    f("status", K::Lit(&["draft", "published"]), D::Str("published")),
    f("created_at", K::Dt, D::Now),
    f("updated_at", K::Dt, D::Now),
    f("published_at", K::OptDt, D::Null),
    f("announced_at", K::OptDt, D::Null),
    f("silent", K::Bool, D::Bool(false)),
];

fn oid_of(b: &Bson) -> Option<ObjectId> {
    match b {
        Bson::ObjectId(o) => Some(*o),
        Bson::String(s) if s.len() == 24 => ObjectId::parse_str(s).ok(),
        _ => None,
    }
}

fn int_of(b: &Bson) -> Option<i64> {
    match b {
        Bson::Int32(v) => Some(i64::from(*v)),
        Bson::Int64(v) => Some(*v),
        Bson::Double(v) if v.is_finite() && v.fract() == 0.0 => Some(*v as i64),
        Bson::Boolean(v) => Some(i64::from(*v)),
        _ => None,
    }
}

fn coerce(kind: K, b: &Bson) -> Option<V> {
    let null = matches!(b, Bson::Null);
    Some(match kind {
        K::Str => match b {
            Bson::String(s) => V::Str(s.clone()),
            _ => return None,
        },
        K::OptStr if null => V::Null,
        K::OptStr => return coerce(K::Str, b),
        K::Int => V::Int(int_of(b)?),
        K::Float => match b {
            Bson::Double(v) => V::Float(*v),
            other => V::Float(int_of(other)? as f64),
        },
        K::Bool => match b {
            Bson::Boolean(v) => V::Bool(*v),
            other => match int_of(other)? {
                0 => V::Bool(false),
                1 => V::Bool(true),
                _ => return None,
            },
        },
        K::Oid => V::Oid(oid_of(b)?),
        K::OptOid if null => V::Null,
        K::OptOid => V::Oid(oid_of(b)?),
        K::Dt => match b {
            Bson::DateTime(d) => V::Dt(dt::from_bson(*d)),
            _ => return None,
        },
        K::OptDt if null => V::Null,
        K::OptDt => return coerce(K::Dt, b),
        K::Map => match b {
            Bson::Document(d) => {
                let mut out = Vec::with_capacity(d.len());
                for (k, v) in d {
                    match v {
                        Bson::String(s) => out.push((k.clone(), s.clone())),
                        _ => return None,
                    }
                }
                V::Map(out)
            }
            _ => return None,
        },
        K::List => match b {
            Bson::Array(a) => {
                let mut out = Vec::with_capacity(a.len());
                for v in a {
                    match v {
                        Bson::String(s) => out.push(s.clone()),
                        _ => return None,
                    }
                }
                V::List(out)
            }
            _ => return None,
        },
        K::Lit(choices) => match b {
            Bson::String(s) if choices.contains(&s.as_str()) => V::Str(s.clone()),
            _ => return None,
        },
        K::OptLit(_) if null => V::Null,
        K::OptLit(choices) => return coerce(K::Lit(choices), b),
        K::Collabs => match b {
            Bson::Array(a) => {
                let mut out = Vec::with_capacity(a.len());
                for v in a {
                    let Bson::Document(d) = v else { return None };
                    let id = oid_of(d.get("user_id")?)?;
                    let Some(Bson::String(name)) = d.get("username") else { return None };
                    out.push((id, name.clone()));
                }
                V::Collabs(out)
            }
            _ => return None,
        },
    })
}

fn default_of(kind: K, d: D) -> Option<V> {
    Some(match d {
        D::Req => return None,
        D::Null => V::Null,
        D::Str(s) => V::Str(s.to_owned()),
        D::Int(n) => V::Int(n),
        D::Float(x) => V::Float(x),
        D::Bool(b) => V::Bool(b),
        D::Now => V::Dt(dt::now()),
        D::Empty => match kind {
            K::Map => V::Map(Vec::new()),
            K::Collabs => V::Collabs(Vec::new()),
            _ => V::List(Vec::new()),
        },
    })
}

pub fn to_bson(v: &V) -> Bson {
    match v {
        V::Null => Bson::Null,
        V::Str(s) => Bson::String(s.clone()),
        V::Int(n) => bson_int(*n),
        V::Float(x) => Bson::Double(*x),
        V::Bool(b) => Bson::Boolean(*b),
        V::Oid(o) => Bson::ObjectId(*o),
        V::Dt(d) => Bson::DateTime(dt::to_bson(*d)),
        V::Map(m) => Bson::Document(m.iter().map(|(k, v)| (k.clone(), Bson::String(v.clone()))).collect()),
        V::List(l) => Bson::Array(l.iter().map(|s| Bson::String(s.clone())).collect()),
        V::Collabs(c) => Bson::Array(
            c.iter()
                .map(|(id, name)| {
                    let mut d = Document::new();
                    d.insert("user_id", *id);
                    d.insert("username", name.clone());
                    Bson::Document(d)
                })
                .collect(),
        ),
    }
}

/// A loaded (or freshly built) document of one of the models above.
#[derive(Clone, Debug)]
pub struct Doc {
    pub id: ObjectId,
    pub fields: Vec<(&'static str, V)>,
}

impl Doc {
    /// The model's validation of a stored document; `None` where pydantic raises.
    pub fn load(spec: &[F], d: &Document) -> Option<Doc> {
        let id = match d.get("_id") {
            Some(Bson::ObjectId(o)) => *o,
            _ => return None,
        };
        let mut fields = Vec::with_capacity(spec.len());
        for fs in spec {
            let v = match d.get(fs.name) {
                Some(b) => coerce(fs.kind, b)?,
                None => default_of(fs.kind, fs.default)?,
            };
            fields.push((fs.name, v));
        }
        Some(Doc { id, fields })
    }

    /// A new model instance: `given` fields, defaults for the rest.
    pub fn build(spec: &[F], given: Vec<(&'static str, V)>) -> Doc {
        let mut fields = Vec::with_capacity(spec.len());
        for fs in spec {
            let v = given
                .iter()
                .find(|(k, _)| *k == fs.name)
                .map(|(_, v)| v.clone())
                .or_else(|| default_of(fs.kind, fs.default))
                .unwrap_or(V::Null);
            fields.push((fs.name, v));
        }
        Doc { id: ObjectId::new(), fields }
    }

    pub fn get(&self, k: &str) -> &V {
        self.fields.iter().find(|(n, _)| *n == k).map_or(&V::Null, |(_, v)| v)
    }

    pub fn set(&mut self, k: &str, v: V) {
        if let Some(slot) = self.fields.iter_mut().find(|(n, _)| *n == k) {
            slot.1 = v;
        }
    }

    pub fn str(&self, k: &str) -> &str {
        match self.get(k) {
            V::Str(s) => s,
            _ => "",
        }
    }

    pub fn opt_str(&self, k: &str) -> Option<&str> {
        match self.get(k) {
            V::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn int(&self, k: &str) -> i64 {
        match self.get(k) {
            V::Int(n) => *n,
            _ => 0,
        }
    }

    pub fn float(&self, k: &str) -> f64 {
        match self.get(k) {
            V::Float(x) => *x,
            _ => 0.0,
        }
    }

    pub fn bool(&self, k: &str) -> bool {
        matches!(self.get(k), V::Bool(true))
    }

    pub fn oid(&self, k: &str) -> Option<ObjectId> {
        match self.get(k) {
            V::Oid(o) => Some(*o),
            _ => None,
        }
    }

    pub fn dt(&self, k: &str) -> Option<DateTime<Utc>> {
        match self.get(k) {
            V::Dt(d) => Some(*d),
            _ => None,
        }
    }

    pub fn map(&self, k: &str) -> &[(String, String)] {
        match self.get(k) {
            V::Map(m) => m,
            _ => &[],
        }
    }

    pub fn list(&self, k: &str) -> &[String] {
        match self.get(k) {
            V::List(l) => l,
            _ => &[],
        }
    }

    pub fn collab_ids(&self) -> Vec<ObjectId> {
        match self.get("collaborators") {
            V::Collabs(c) => c.iter().map(|(id, _)| *id).collect(),
            _ => Vec::new(),
        }
    }

    /// `_iso(field)`: `isoformat()`, or null.
    pub fn iso(&self, k: &str) -> Value {
        self.dt(k).map_or(Value::Null, |d| Value::String(dt::isoformat(&d)))
    }

    pub fn map_json(&self, k: &str) -> Value {
        Value::Object(self.map(k).iter().map(|(a, b)| (a.clone(), Value::String(b.clone()))).collect::<Map<_, _>>())
    }

    pub fn list_json(&self, k: &str) -> Value {
        Value::Array(self.list(k).iter().map(|s| Value::String(s.clone())).collect())
    }

    pub fn opt_json(&self, k: &str) -> Value {
        self.opt_str(k).map_or(Value::Null, |s| Value::String(s.to_owned()))
    }

    /// The `$set` body of `save()`.
    pub fn set_doc(&self) -> Document {
        self.fields.iter().map(|(k, v)| ((*k).to_owned(), to_bson(v))).collect()
    }

    /// What `insert()` writes.
    pub fn insert_doc(&self) -> Document {
        let mut d = Document::new();
        d.insert("_id", self.id);
        for (k, v) in &self.fields {
            d.insert(*k, to_bson(v));
        }
        d
    }
}
