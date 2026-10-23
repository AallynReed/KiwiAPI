//! The Beanie documents this area loads and saves. A load validates the stored
//! document the way pydantic does (a document that fails is a 500); `save()`
//! `$set`s every model field in order with pydantic's types; `insert()` writes
//! the same with `_id` first. A datetime set in memory keeps its microseconds
//! until the next save (Mongo holds the millisecond floor).

use chrono::{DateTime, Utc};
use mongodb::{
    Collection,
    bson::{Bson, Document, doc, oid::ObjectId},
};
use serde_json::{Map, Value};

use crate::{auth::pyutil::bson_int, dt, error::ApiError};

#[derive(Clone, Copy)]
pub enum K {
    Str,
    OptStr,
    Int,
    OptInt,
    Float,
    Bool,
    Oid,
    OptOid,
    Dt,
    OptDt,
    StrList,
    StrMap,
    Lit(&'static [&'static str]),
    OptLit(&'static [&'static str]),
    Models(&'static [F]),
}

#[derive(Clone, Copy)]
pub enum D {
    Req,
    S(&'static str),
    Zero,
    ZeroF,
    B(bool),
    Null,
    Map,
    List,
    Now,
}

pub struct F(pub &'static str, pub K, pub D);

use D::{B, List, Map as DMap, Now, Null, Req, S, Zero, ZeroF};
use K::*;

const VISIBILITY: &[&str] = &["draft", "unlisted", "public", "public_website_only", "hidden"];
const STRAY_STATUS: &[&str] = &["approved", "pending", "rejected"];
pub const CLAIM_STATUS: &[&str] = &["pending", "approved", "rejected"];
const TARGET_TYPES: &[&str] = &["mod", "modpack", "profile"];

const COLLABORATOR: &[F] = &[F("user_id", Oid, Req), F("username", Str, Req)];

pub const MOD_PROJECT: &[F] = &[
    F("slug", Str, Req),
    F("title", Str, Req),
    F("summary", Str, S("")),
    F("description", Str, S("")),
    F("readme_text", Str, S("")),
    F("warnings", Str, S("")),
    F("title_i18n", StrMap, DMap),
    F("summary_i18n", StrMap, DMap),
    F("description_i18n", StrMap, DMap),
    F("readme_i18n", StrMap, DMap),
    F("warnings_i18n", StrMap, DMap),
    F("tags", StrList, List),
    F("owner_id", OptOid, Null),
    F("owner_username", Str, S("")),
    F("owner_handle", Str, S("")),
    F("collaborators", Models(COLLABORATOR), List),
    F("is_stray", Bool, B(false)),
    F("stray_status", OptLit(STRAY_STATUS), Null),
    F("author", Str, S("")),
    F("source", OptStr, Null),
    F("source_id", OptStr, Null),
    F("source_url", OptStr, Null),
    F("source_author_id", OptStr, Null),
    F("source_file_id", OptStr, Null),
    F("source_likes", Int, Zero),
    F("uploaded_on_behalf", Bool, B(false)),
    F("issues_enabled", Bool, B(true)),
    F("issue_seq", Int, Zero),
    F("open_issue_count", Int, Zero),
    F("visibility", Lit(VISIBILITY), S("draft")),
    F("is_beta", Bool, B(false)),
    F("mode", Lit(&["files", "releases"]), S("files")),
    F("source_visibility", Lit(&["public", "private"]), S("public")),
    F("default_branch", Str, S("main")),
    F("banner_sha", OptStr, Null),
    F("preview_shas", StrList, List),
    F("discord_url", OptStr, Null),
    F("website_url", OptStr, Null),
    F("donation_urls", StrList, List),
    F("download_count", Int, Zero),
    F("star_count", Int, Zero),
    F("downloads_7d", Int, Zero),
    F("popularity_score", Float, ZeroF),
    F("hidden_release_branches", StrList, List),
    F("branch_order", StrList, List),
    F("forked_from_id", OptOid, Null),
    F("forked_from_slug", OptStr, Null),
    F("forked_from_handle", OptStr, Null),
    F("forked_from_title", OptStr, Null),
    F("forked_from_owner", OptStr, Null),
    F("inspired_by_slug", OptStr, Null),
    F("inspired_by_handle", OptStr, Null),
    F("inspired_by_title", OptStr, Null),
    F("inspired_by_owner", OptStr, Null),
    F("fork_count", Int, Zero),
    F("is_remix", Bool, B(false)),
    F("remix_status", Lit(&["pending", "approved", "denied"]), S("pending")),
    F("remix_requested_at", OptDt, Null),
    F("remix_resolved_at", OptDt, Null),
    F("remix_resolved_by", OptOid, Null),
    F("taken_down", Bool, B(false)),
    F("takedown_reason", OptStr, Null),
    F("created_at", Dt, Now),
    F("updated_at", Dt, Now),
    F("last_release_at", OptDt, Null),
];

pub const MOD_RELEASE: &[F] = &[
    F("project_id", Oid, Req),
    F("owner_id", OptOid, Null),
    F("tag", Str, Req),
    F("branch", Str, S("main")),
    F("title", Str, S("")),
    F("changelog", Str, S("")),
    F("title_i18n", StrMap, DMap),
    F("changelog_i18n", StrMap, DMap),
    F("source_commit_sha", OptStr, Null),
    F("release_format", Lit(&["tmod", "zip"]), S("tmod")),
    F("tmod_sha", Str, Req),
    F("prior_tmod_shas", StrList, List),
    F("tmod_size", Int, Zero),
    F("tmod_filename", Str, S("mod.tmod")),
    F("tmod_properties", StrMap, DMap),
    F("banner_sha", OptStr, Null),
    F("download_count", Int, Zero),
    F("status", Lit(&["draft", "published"]), S("published")),
    F("created_at", Dt, Now),
    F("updated_at", Dt, Now),
    F("published_at", OptDt, Null),
    F("announced_at", OptDt, Null),
    F("silent", Bool, B(false)),
];

pub const MOD_PROFILE: &[F] = &[
    F("site_user_id", Oid, Req),
    F("handle", Str, S("")),
    F("display_name", Str, S("")),
    F("tagline", Str, S("")),
    F("readme", Str, S("")),
    F("tagline_i18n", StrMap, DMap),
    F("readme_i18n", StrMap, DMap),
    F("avatar_sha", OptStr, Null),
    F("banner_sha", OptStr, Null),
    F("discord_url", OptStr, Null),
    F("website_url", OptStr, Null),
    F("donation_urls", StrList, List),
    F("mod_order", StrList, List),
    F("featured_slug", OptStr, Null),
    F("taken_down", Bool, B(false)),
    F("takedown_reason", OptStr, Null),
    F("created_at", Dt, Now),
    F("updated_at", Dt, Now),
];

pub const CLAIM: &[F] = &[
    F("project_id", Oid, Req),
    F("project_slug", Str, Req),
    F("project_title", Str, S("")),
    F("claimant_id", Oid, Req),
    F("claimant_username", Str, Req),
    F("message", Str, S("")),
    F("status", Lit(CLAIM_STATUS), S("pending")),
    F("resolved_by", OptOid, Null),
    F("resolved_at", OptDt, Null),
    F("created_at", Dt, Now),
];

pub const REMIX_CLAIM: &[F] = &[
    F("target_id", Oid, Req),
    F("target_slug", Str, S("")),
    F("target_title", Str, S("")),
    F("original_id", Oid, Req),
    F("original_slug", Str, S("")),
    F("original_title", Str, S("")),
    F("claimant_id", Oid, Req),
    F("claimant_username", Str, Req),
    F("message", Str, S("")),
    F("status", Lit(CLAIM_STATUS), S("pending")),
    F("resolved_by", OptOid, Null),
    F("resolved_at", OptDt, Null),
    F("created_at", Dt, Now),
];

pub const CONTENT_REPORT: &[F] = &[
    F("target_type", Lit(TARGET_TYPES), Req),
    F("target_id", Oid, Req),
    F("target_label", Str, S("")),
    F("target_url", Str, S("")),
    F("reason", Str, Req),
    F("resolved", Bool, B(false)),
    F("created_at", Dt, Now),
];

pub const TAKEN_DOWN: &[F] = &[
    F("sha256", Str, Req),
    F("md5", OptStr, Null),
    F("project_id", OptOid, Null),
    F("title", Str, S("")),
    F("owner_handle", Str, S("")),
    F("created_at", Dt, Now),
];

pub const STRAY_STATE: &[F] = &[
    F("key", Str, S(super::strayimport::SOURCE)),
    F("running", Bool, B(false)),
    F("phase", Str, S("idle")),
    F("total", Int, Zero),
    F("processed", Int, Zero),
    F("imported", Int, Zero),
    F("updated", Int, Zero),
    F("pending_added", Int, Zero),
    F("failed", Int, Zero),
    F("last_error", OptStr, Null),
    F("started_at", OptDt, Null),
    F("finished_at", OptDt, Null),
    F("updated_at", Dt, Now),
];

pub const IMAGE_ASSET: &[F] = &[
    F("sha", Str, Req),
    F("content_type", Str, Req),
    F("byte_size", Int, Req),
    F("owner_id", OptOid, Null),
    F("width", OptInt, Null),
    F("height", OptInt, Null),
    F("variants", StrMap, DMap),
    F("created_at", Dt, Now),
];

const MODPACK_ENTRY: &[F] = &[
    F("project_id", OptOid, Null),
    F("handle", Str, S("")),
    F("slug", Str, S("")),
    F("title", Str, S("")),
    F("branch", Str, S("main")),
    F("version_locked", Bool, B(false)),
    F("locked_tag", OptStr, Null),
    F("custom_sha", OptStr, Null),
    F("custom_filename", OptStr, Null),
    F("author", Str, S("")),
];

const MODPACK_VARIANT: &[F] = &[F("name", Str, Req), F("label", Str, S("")), F("entries", Models(MODPACK_ENTRY), List)];

pub const MODPACK: &[F] = &[
    F("slug", Str, Req),
    F("title", Str, Req),
    F("summary", Str, S("")),
    F("description", Str, S("")),
    F("warnings", Str, S("")),
    F("tags", StrList, List),
    F("owner_id", Oid, Req),
    F("owner_username", Str, Req),
    F("owner_handle", Str, S("")),
    F("collaborators", Models(COLLABORATOR), List),
    F("visibility", Lit(&["draft", "unlisted", "public"]), S("draft")),
    F("banner_sha", OptStr, Null),
    F("preview_shas", StrList, List),
    F("discord_url", OptStr, Null),
    F("website_url", OptStr, Null),
    F("donation_urls", StrList, List),
    F("download_count", Int, Zero),
    F("star_count", Int, Zero),
    F("variants", Models(MODPACK_VARIANT), List),
    F("default_variant", Str, S("default")),
    F("taken_down", Bool, B(false)),
    F("takedown_reason", OptStr, Null),
    F("created_at", Dt, Now),
    F("updated_at", Dt, Now),
];

fn bad(field: &str, v: &Bson) -> String {
    format!("{field}: {v:?}")
}

fn int_of(v: &Bson) -> Option<i64> {
    match v {
        Bson::Int32(n) => Some(i64::from(*n)),
        Bson::Int64(n) => Some(*n),
        Bson::Boolean(b) => Some(i64::from(*b)),
        Bson::Double(f) if f.is_finite() && f.fract() == 0.0 && f.abs() < 9.2e18 => Some(*f as i64),
        Bson::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn bool_of(v: &Bson) -> Option<bool> {
    match v {
        Bson::Boolean(b) => Some(*b),
        Bson::String(s) => match s.to_lowercase().as_str() {
            "0" | "off" | "f" | "false" | "n" | "no" => Some(false),
            "1" | "on" | "t" | "true" | "y" | "yes" => Some(true),
            _ => None,
        },
        other => match int_of(other) {
            Some(0) => Some(false),
            Some(1) => Some(true),
            _ => None,
        },
    }
}

fn str_of(v: &Bson) -> Option<String> {
    match v {
        Bson::String(s) => Some(s.clone()),
        _ => None,
    }
}

fn oid_of(v: &Bson) -> Option<ObjectId> {
    match v {
        Bson::ObjectId(o) => Some(*o),
        Bson::String(s) => ObjectId::parse_str(s).ok(),
        _ => None,
    }
}

fn coerce(name: &str, kind: K, v: &Bson) -> Result<Bson, String> {
    let fail = || bad(name, v);
    let opt = |v: &Bson| matches!(v, Bson::Null);
    Ok(match kind {
        Str => Bson::String(str_of(v).ok_or_else(fail)?),
        OptStr if opt(v) => Bson::Null,
        OptStr => Bson::String(str_of(v).ok_or_else(fail)?),
        Int => bson_int(int_of(v).ok_or_else(fail)?),
        OptInt if opt(v) => Bson::Null,
        OptInt => bson_int(int_of(v).ok_or_else(fail)?),
        Float => Bson::Double(match v {
            Bson::Double(f) => *f,
            Bson::Int32(n) => f64::from(*n),
            Bson::Int64(n) => *n as f64,
            Bson::Boolean(b) => f64::from(u8::from(*b)),
            Bson::String(s) => s.trim().parse().map_err(|_| fail())?,
            _ => return Err(fail()),
        }),
        Bool => Bson::Boolean(bool_of(v).ok_or_else(fail)?),
        Oid => Bson::ObjectId(oid_of(v).ok_or_else(fail)?),
        OptOid if opt(v) => Bson::Null,
        OptOid => Bson::ObjectId(oid_of(v).ok_or_else(fail)?),
        Dt => match v {
            Bson::DateTime(d) => Bson::DateTime(*d),
            _ => return Err(fail()),
        },
        OptDt if opt(v) => Bson::Null,
        OptDt => match v {
            Bson::DateTime(d) => Bson::DateTime(*d),
            _ => return Err(fail()),
        },
        StrList => match v {
            Bson::Array(a) => {
                Bson::Array(a.iter().map(|x| str_of(x).map(Bson::String).ok_or_else(fail)).collect::<Result<_, _>>()?)
            }
            _ => return Err(fail()),
        },
        StrMap => match v {
            Bson::Document(d) => {
                let mut out = Document::new();
                for (k, x) in d {
                    out.insert(k.clone(), str_of(x).ok_or_else(fail)?);
                }
                Bson::Document(out)
            }
            _ => return Err(fail()),
        },
        Lit(choices) => match v {
            Bson::String(s) if choices.contains(&s.as_str()) => v.clone(),
            _ => return Err(fail()),
        },
        OptLit(_) if opt(v) => Bson::Null,
        OptLit(choices) => match v {
            Bson::String(s) if choices.contains(&s.as_str()) => v.clone(),
            _ => return Err(fail()),
        },
        Models(spec) => match v {
            Bson::Array(a) => Bson::Array(
                a.iter()
                    .map(|x| match x {
                        Bson::Document(d) => normalize(spec, d, &mut Vec::new()).map(Bson::Document),
                        _ => Err(fail()),
                    })
                    .collect::<Result<_, _>>()?,
            ),
            _ => return Err(fail()),
        },
    })
}

fn default_of(
    name: &'static str,
    d: D,
    now: DateTime<Utc>,
    fresh: &mut Vec<(&'static str, DateTime<Utc>)>,
) -> Option<Bson> {
    Some(match d {
        Req => return None,
        S(s) => Bson::String(s.into()),
        Zero => Bson::Int32(0),
        ZeroF => Bson::Double(0.0),
        B(b) => Bson::Boolean(b),
        Null => Bson::Null,
        DMap => Bson::Document(Document::new()),
        List => Bson::Array(Vec::new()),
        Now => {
            fresh.push((name, now));
            Bson::DateTime(dt::to_bson(now))
        }
    })
}

fn normalize(
    spec: &'static [F],
    raw: &Document,
    fresh: &mut Vec<(&'static str, DateTime<Utc>)>,
) -> Result<Document, String> {
    normalize_at(spec, raw, fresh, dt::now())
}

fn normalize_at(
    spec: &'static [F],
    raw: &Document,
    fresh: &mut Vec<(&'static str, DateTime<Utc>)>,
    now: DateTime<Utc>,
) -> Result<Document, String> {
    let mut out = Document::new();
    for F(name, kind, default) in spec {
        let v = match raw.get(*name) {
            Some(v) => coerce(name, *kind, v)?,
            None => default_of(name, *default, now, fresh).ok_or_else(|| format!("{name}: missing"))?,
        };
        out.insert(*name, v);
    }
    Ok(out)
}

/// One loaded (or newly built) document.
#[derive(Clone)]
pub struct Model {
    pub id: ObjectId,
    pub doc: Document,
    fresh: Vec<(&'static str, DateTime<Utc>)>,
}

pub fn internal(e: impl std::fmt::Display) -> ApiError {
    tracing::error!("mods moderation: {e}");
    ApiError::internal()
}

impl Model {
    pub fn load(spec: &'static [F], raw: &Document) -> Result<Model, ApiError> {
        Model::load_at(spec, raw, dt::now())
    }

    pub fn load_at(spec: &'static [F], raw: &Document, now: DateTime<Utc>) -> Result<Model, ApiError> {
        let id = raw.get_object_id("_id").map_err(internal)?;
        let mut fresh = Vec::new();
        let doc = normalize_at(spec, raw, &mut fresh, now).map_err(|e| internal(format!("_id {id}: {e}")))?;
        Ok(Model { id, doc, fresh })
    }

    /// `Model(**fields)`: a new document with a fresh `_id`.
    pub fn build(spec: &'static [F], fields: Document) -> Result<Model, ApiError> {
        Model::build_at(spec, fields, dt::now(), ObjectId::new())
    }

    pub fn build_at(spec: &'static [F], fields: Document, now: DateTime<Utc>, id: ObjectId) -> Result<Model, ApiError> {
        let mut fresh = Vec::new();
        let doc = normalize_at(spec, &fields, &mut fresh, now).map_err(internal)?;
        Ok(Model { id, doc, fresh })
    }

    pub fn get(&self, k: &str) -> &Bson {
        self.doc.get(k).unwrap_or(&Bson::Null)
    }

    pub fn str(&self, k: &str) -> &str {
        self.doc.get_str(k).unwrap_or_default()
    }

    pub fn opt_str(&self, k: &str) -> Option<&str> {
        self.doc.get_str(k).ok()
    }

    pub fn bool(&self, k: &str) -> bool {
        self.doc.get_bool(k).unwrap_or_default()
    }

    pub fn int(&self, k: &str) -> i64 {
        match self.get(k) {
            Bson::Int32(n) => i64::from(*n),
            Bson::Int64(n) => *n,
            _ => 0,
        }
    }

    pub fn float(&self, k: &str) -> f64 {
        self.doc.get_f64(k).unwrap_or_default()
    }

    pub fn oid(&self, k: &str) -> Option<ObjectId> {
        self.doc.get_object_id(k).ok()
    }

    pub fn strs(&self, k: &str) -> Vec<String> {
        self.doc
            .get_array(k)
            .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect())
            .unwrap_or_default()
    }

    pub fn dt(&self, k: &str) -> Option<DateTime<Utc>> {
        if let Some((_, d)) = self.fresh.iter().rev().find(|(n, _)| *n == k) {
            return Some(*d);
        }
        self.doc.get_datetime(k).ok().map(|d| dt::from_bson(*d))
    }

    /// `iso(field)`: `isoformat()` or `None`.
    pub fn iso(&self, k: &str) -> Value {
        self.dt(k).map_or(Value::Null, |d| Value::String(dt::isoformat(&d)))
    }

    pub fn set(&mut self, k: &'static str, v: impl Into<Bson>) {
        self.fresh.retain(|(n, _)| *n != k);
        self.doc.insert(k, v.into());
    }

    pub fn set_dt(&mut self, k: &'static str, d: DateTime<Utc>) {
        self.fresh.retain(|(n, _)| *n != k);
        self.fresh.push((k, d));
        self.doc.insert(k, dt::to_bson(d));
    }

    /// A string-to-string map field as JSON (insertion order kept).
    pub fn map(&self, k: &str) -> Value {
        let mut out = Map::new();
        if let Ok(d) = self.doc.get_document(k) {
            for (key, v) in d {
                out.insert(key.clone(), Value::String(v.as_str().unwrap_or_default().to_owned()));
            }
        }
        Value::Object(out)
    }

    pub fn json_str_opt(&self, k: &str) -> Value {
        self.opt_str(k).map_or(Value::Null, |s| Value::String(s.to_owned()))
    }

    /// What `save()` `$set`s and `insert()` writes: `_id` first, then every field.
    pub fn to_doc(&self) -> Document {
        let mut out = doc! {"_id": self.id};
        for (k, v) in &self.doc {
            out.insert(k.clone(), v.clone());
        }
        out
    }

    /// `save()`: Beanie then merges the stored document back, so every
    /// datetime reads at millisecond precision afterwards.
    pub async fn save(&mut self, c: &Collection<Document>) -> Result<(), ApiError> {
        c.update_one(doc! {"_id": self.id}, doc! {"$set": self.to_doc()}).upsert(true).await.map_err(internal)?;
        self.saved();
        Ok(())
    }

    pub fn saved(&mut self) {
        self.fresh.clear();
    }

    pub async fn insert(&self, c: &Collection<Document>) -> Result<(), ApiError> {
        c.insert_one(self.to_doc()).await.map_err(internal)?;
        Ok(())
    }
}
