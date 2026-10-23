//! The `ModProject` document as Beanie loads and saves it: every model field
//! coerced the way pydantic validates it, `save()` a `$set` of all of them in
//! model order (upsert), plus the visibility rules of `mods_hub/service.py`.

use chrono::{DateTime, TimeDelta, Utc};
use mongodb::bson::{Bson, Document, doc, oid::ObjectId};

use crate::{auth::models::SiteUser, auth::pyutil::bson_int, dt};

pub const COLLECTION: &str = "mod_projects";
pub const STRAY_HANDLE: &str = "stray";
const REACHABLE: [&str; 3] = ["public", "public_website_only", "unlisted"];

#[derive(Clone, Copy)]
enum K {
    Str(&'static str),
    StrReq,
    Int,
    Float,
    Bool(bool),
    OptStr,
    OptDt,
    DtNow,
    OptOid,
    StrList,
    StrMap,
    Collabs,
    Lit(&'static [&'static str], &'static str),
    OptLit(&'static [&'static str]),
}

const VISIBILITY: &[&str] = &["draft", "unlisted", "public", "public_website_only", "hidden"];

const FIELDS: &[(&str, K)] = &[
    ("slug", K::StrReq),
    ("title", K::StrReq),
    ("summary", K::Str("")),
    ("description", K::Str("")),
    ("readme_text", K::Str("")),
    ("warnings", K::Str("")),
    ("title_i18n", K::StrMap),
    ("summary_i18n", K::StrMap),
    ("description_i18n", K::StrMap),
    ("readme_i18n", K::StrMap),
    ("warnings_i18n", K::StrMap),
    ("tags", K::StrList),
    ("owner_id", K::OptOid),
    ("owner_username", K::Str("")),
    ("owner_handle", K::Str("")),
    ("collaborators", K::Collabs),
    ("is_stray", K::Bool(false)),
    ("stray_status", K::OptLit(&["approved", "pending", "rejected"])),
    ("author", K::Str("")),
    ("source", K::OptStr),
    ("source_id", K::OptStr),
    ("source_url", K::OptStr),
    ("source_author_id", K::OptStr),
    ("source_file_id", K::OptStr),
    ("source_likes", K::Int),
    ("uploaded_on_behalf", K::Bool(false)),
    ("issues_enabled", K::Bool(true)),
    ("issue_seq", K::Int),
    ("open_issue_count", K::Int),
    ("visibility", K::Lit(VISIBILITY, "draft")),
    ("is_beta", K::Bool(false)),
    ("mode", K::Lit(&["files", "releases"], "files")),
    ("source_visibility", K::Lit(&["public", "private"], "public")),
    ("default_branch", K::Str("main")),
    ("banner_sha", K::OptStr),
    ("preview_shas", K::StrList),
    ("discord_url", K::OptStr),
    ("website_url", K::OptStr),
    ("donation_urls", K::StrList),
    ("download_count", K::Int),
    ("star_count", K::Int),
    ("downloads_7d", K::Int),
    ("popularity_score", K::Float),
    ("hidden_release_branches", K::StrList),
    ("branch_order", K::StrList),
    ("forked_from_id", K::OptOid),
    ("forked_from_slug", K::OptStr),
    ("forked_from_handle", K::OptStr),
    ("forked_from_title", K::OptStr),
    ("forked_from_owner", K::OptStr),
    ("inspired_by_slug", K::OptStr),
    ("inspired_by_handle", K::OptStr),
    ("inspired_by_title", K::OptStr),
    ("inspired_by_owner", K::OptStr),
    ("fork_count", K::Int),
    ("is_remix", K::Bool(false)),
    ("remix_status", K::Lit(&["pending", "approved", "denied"], "pending")),
    ("remix_requested_at", K::OptDt),
    ("remix_resolved_at", K::OptDt),
    ("remix_resolved_by", K::OptOid),
    ("taken_down", K::Bool(false)),
    ("takedown_reason", K::OptStr),
    ("created_at", K::DtNow),
    ("updated_at", K::DtNow),
    ("last_release_at", K::OptDt),
];

/// A stored value pydantic rejects: the load raises, a 500.
#[derive(Debug)]
pub struct Invalid(pub String);

fn as_int(v: &Bson) -> Option<i64> {
    match v {
        Bson::Int32(n) => Some(*n as i64),
        Bson::Int64(n) => Some(*n),
        Bson::Double(f) if f.fract() == 0.0 && f.is_finite() => Some(*f as i64),
        Bson::Boolean(b) => Some(*b as i64),
        _ => None,
    }
}

fn as_oid(v: &Bson) -> Option<ObjectId> {
    match v {
        Bson::ObjectId(o) => Some(*o),
        Bson::String(s) => ObjectId::parse_str(s).ok(),
        _ => None,
    }
}

fn coerce(name: &str, kind: K, v: Option<&Bson>) -> Result<Bson, Invalid> {
    let bad = || Invalid(format!("mod_projects.{name}: {v:?}"));
    Ok(match (kind, v) {
        (K::StrReq, None) => return Err(bad()),
        (K::Str(_) | K::StrReq, Some(Bson::String(s))) => Bson::String(s.clone()),
        (K::Str(d), None) => Bson::String(d.into()),
        (K::Int, None) => Bson::Int32(0),
        (K::Int, Some(v)) => bson_int(as_int(v).ok_or_else(bad)?),
        (K::Float, None) => Bson::Double(0.0),
        (K::Float, Some(Bson::Double(f))) => Bson::Double(*f),
        (K::Float, Some(v)) => Bson::Double(as_int(v).ok_or_else(bad)? as f64),
        (K::Bool(d), None) => Bson::Boolean(d),
        (K::Bool(_), Some(Bson::Boolean(b))) => Bson::Boolean(*b),
        (K::Bool(_), Some(v)) => match as_int(v) {
            Some(0) => Bson::Boolean(false),
            Some(1) => Bson::Boolean(true),
            _ => return Err(bad()),
        },
        (K::OptStr | K::OptDt | K::OptOid | K::OptLit(_), None | Some(Bson::Null)) => Bson::Null,
        (K::OptStr, Some(Bson::String(s))) => Bson::String(s.clone()),
        (K::OptDt | K::DtNow, Some(Bson::DateTime(d))) => Bson::DateTime(*d),
        (K::DtNow, None) => Bson::DateTime(dt::to_bson(dt::now())),
        (K::OptOid, Some(v)) => Bson::ObjectId(as_oid(v).ok_or_else(bad)?),
        (K::StrList, None) | (K::Collabs, None) => Bson::Array(Vec::new()),
        (K::StrList, Some(Bson::Array(a))) => Bson::Array(
            a.iter()
                .map(|x| matches!(x, Bson::String(_)).then(|| x.clone()).ok_or_else(bad))
                .collect::<Result<_, _>>()?,
        ),
        (K::StrMap, None) => Bson::Document(Document::new()),
        (K::StrMap, Some(Bson::Document(d))) => {
            let mut out = Document::new();
            for (k, x) in d {
                if !matches!(x, Bson::String(_)) {
                    return Err(bad());
                }
                out.insert(k.clone(), x.clone());
            }
            Bson::Document(out)
        }
        (K::Collabs, Some(Bson::Array(a))) => {
            let mut out = Vec::new();
            for c in a {
                let Bson::Document(c) = c else { return Err(bad()) };
                let user_id = c.get("user_id").and_then(as_oid).ok_or_else(bad)?;
                let Some(Bson::String(username)) = c.get("username") else { return Err(bad()) };
                out.push(Bson::Document(doc! {"user_id": user_id, "username": username.clone()}));
            }
            Bson::Array(out)
        }
        (K::Lit(_, d), None) => Bson::String(d.into()),
        (K::Lit(allowed, _) | K::OptLit(allowed), Some(Bson::String(s))) if allowed.contains(&s.as_str()) => {
            Bson::String(s.clone())
        }
        _ => return Err(bad()),
    })
}

#[derive(Clone, Debug)]
pub struct Project {
    pub id: ObjectId,
    /// Every model field, validated, in model order.
    pub fields: Document,
}

impl Project {
    pub fn from_doc(raw: &Document) -> Result<Project, Invalid> {
        let id = raw.get("_id").and_then(as_oid).ok_or_else(|| Invalid("mod_projects._id".into()))?;
        let mut fields = Document::new();
        for (name, kind) in FIELDS {
            fields.insert(*name, coerce(name, *kind, raw.get(*name))?);
        }
        Ok(Project { id, fields })
    }

    pub fn str(&self, k: &str) -> &str {
        self.fields.get_str(k).unwrap_or("")
    }

    pub fn opt_str(&self, k: &str) -> Option<&str> {
        self.fields.get_str(k).ok()
    }

    pub fn bool(&self, k: &str) -> bool {
        self.fields.get_bool(k).unwrap_or(false)
    }

    pub fn oid(&self, k: &str) -> Option<ObjectId> {
        self.fields.get_object_id(k).ok()
    }

    pub fn datetime(&self, k: &str) -> Option<DateTime<Utc>> {
        self.fields.get_datetime(k).ok().map(|d| dt::from_bson(*d))
    }

    pub fn owner_id(&self) -> Option<ObjectId> {
        self.oid("owner_id")
    }

    pub fn collaborator_ids(&self) -> Vec<ObjectId> {
        self.fields
            .get_array("collaborators")
            .map(|a| a.iter().filter_map(|c| c.as_document().and_then(|d| d.get_object_id("user_id").ok())).collect())
            .unwrap_or_default()
    }

    pub fn default_branch(&self) -> &str {
        self.str("default_branch")
    }

    pub fn set_updated_now(&mut self) {
        self.fields.insert("updated_at", Bson::DateTime(dt::to_bson(dt::now())));
    }

    /// The `$set` of `ModProject.save()`: `_id` then every field.
    pub fn save_set(&self) -> Document {
        let mut d = doc! {"_id": self.id};
        for (k, v) in &self.fields {
            d.insert(k.clone(), v.clone());
        }
        d
    }
}

pub fn can_edit(p: &Project, actor: Option<&SiteUser>) -> bool {
    let Some(a) = actor else { return false };
    p.owner_id() == Some(a.id) || p.collaborator_ids().contains(&a.id)
}

pub fn is_primary_owner(p: &Project, actor: Option<&SiteUser>) -> bool {
    actor.is_some_and(|a| p.owner_id() == Some(a.id))
}

pub fn is_site_master(actor: Option<&SiteUser>, master_handle: &str) -> bool {
    actor.is_some_and(|a| a.username == master_handle)
}

/// `effective_visibility`.
pub fn effective_visibility(p: &Project, now: DateTime<Utc>) -> String {
    let vis = p.str("visibility");
    if !p.bool("is_remix") || !matches!(vis, "public" | "public_website_only") {
        return vis.to_owned();
    }
    let status = p.str("remix_status");
    if status == "denied" {
        return "hidden".into();
    }
    let aged = p.datetime("remix_requested_at").is_some_and(|t| now - t >= TimeDelta::days(7));
    if status == "approved" || aged { "public_website_only".into() } else { "hidden".into() }
}

pub fn can_view(p: &Project, viewer: Option<&SiteUser>, master: &str, now: DateTime<Utc>) -> bool {
    if can_edit(p, viewer) {
        return true;
    }
    if p.bool("taken_down") && !is_site_master(viewer, master) {
        return false;
    }
    REACHABLE.contains(&effective_visibility(p, now).as_str())
}

/// `source_visible`: files mode, and the editor or a viewable public source.
pub fn source_visible(p: &Project, viewer: Option<&SiteUser>, master: &str, now: DateTime<Utc>) -> bool {
    if p.str("mode") != "files" {
        return false;
    }
    if can_edit(p, viewer) {
        return true;
    }
    if p.bool("taken_down") && !is_site_master(viewer, master) {
        return false;
    }
    if p.str("source_visibility") != "public" {
        return false;
    }
    REACHABLE.contains(&effective_visibility(p, now).as_str())
}
