//! The Beanie documents of `app/trove/mods_hub/models.py` this component reads
//! and writes. Reads are lenient the way pydantic's lax mode is; writes emit
//! the fields in model order with pymongo's types (int32 when it fits,
//! explicit nulls), so a document Rust writes is the one Python would.

use chrono::{DateTime, Utc};
use mongodb::bson::{Bson, Document, oid::ObjectId};
use serde_json::{Map, Value};

use crate::{auth::pyutil::bson_int, dt};

pub const PROJECTS: &str = "mod_projects";
pub const RELEASES: &str = "mod_releases";
pub const STARS: &str = "mod_stars";
pub const PROFILES: &str = "mod_profiles";
pub const IMAGES: &str = "mod_images";
pub const DOWNLOAD_EVENTS: &str = "mod_download_events";
pub const REMIX_CLAIMS: &str = "mod_remix_claims";
pub const CLAIMS: &str = "mod_claim_requests";
pub const TAKEN_DOWN: &str = "mod_taken_down_artifacts";
pub const REPORTS: &str = "content_reports";
pub const ISSUES: &str = "mod_issues";
pub const ISSUE_EVENTS: &str = "mod_issue_events";
pub const CREATOR_LINKS: &str = "mod_creator_links";

/// A `dict[str, str]` in insertion order.
pub type StrMap = Vec<(String, String)>;

pub fn map_json(m: &StrMap) -> Value {
    Value::Object(m.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect::<Map<_, _>>())
}

fn map_bson(m: &StrMap) -> Bson {
    Bson::Document(m.iter().map(|(k, v)| (k.clone(), Bson::String(v.clone()))).collect())
}

fn strs_bson(v: &[String]) -> Bson {
    Bson::Array(v.iter().cloned().map(Bson::String).collect())
}

pub fn opt_str(v: &Option<String>) -> Bson {
    v.as_ref().map_or(Bson::Null, |s| Bson::String(s.clone()))
}

pub fn opt_oid(v: &Option<ObjectId>) -> Bson {
    v.map_or(Bson::Null, Bson::ObjectId)
}

pub fn date(d: &DateTime<Utc>) -> Bson {
    Bson::DateTime(dt::to_bson(*d))
}

pub fn opt_date(v: &Option<DateTime<Utc>>) -> Bson {
    v.as_ref().map_or(Bson::Null, date)
}

/// Field access with pydantic's lax coercions and the model defaults.
pub struct Rd<'a>(pub &'a Document);

impl Rd<'_> {
    pub fn oid(&self, k: &str) -> Option<ObjectId> {
        match self.0.get(k) {
            Some(Bson::ObjectId(o)) => Some(*o),
            Some(Bson::String(s)) => ObjectId::parse_str(s).ok(),
            _ => None,
        }
    }

    pub fn id(&self) -> ObjectId {
        self.oid("_id").unwrap_or_else(|| ObjectId::from_bytes([0; 12]))
    }

    pub fn os(&self, k: &str) -> Option<String> {
        match self.0.get(k) {
            Some(Bson::String(s)) => Some(s.clone()),
            _ => None,
        }
    }

    pub fn s(&self, k: &str, default: &str) -> String {
        self.os(k).unwrap_or_else(|| default.to_owned())
    }

    pub fn b(&self, k: &str, default: bool) -> bool {
        match self.0.get(k) {
            Some(Bson::Boolean(b)) => *b,
            Some(Bson::Int32(n)) => *n != 0,
            Some(Bson::Int64(n)) => *n != 0,
            _ => default,
        }
    }

    pub fn oi(&self, k: &str) -> Option<i64> {
        match self.0.get(k) {
            Some(Bson::Int32(n)) => Some(i64::from(*n)),
            Some(Bson::Int64(n)) => Some(*n),
            Some(Bson::Double(f)) if f.is_finite() && f.fract() == 0.0 => Some(*f as i64),
            _ => None,
        }
    }

    pub fn i(&self, k: &str, default: i64) -> i64 {
        self.oi(k).unwrap_or(default)
    }

    pub fn f(&self, k: &str, default: f64) -> f64 {
        match self.0.get(k) {
            Some(Bson::Double(f)) => *f,
            Some(Bson::Int32(n)) => f64::from(*n),
            Some(Bson::Int64(n)) => *n as f64,
            _ => default,
        }
    }

    pub fn odt(&self, k: &str) -> Option<DateTime<Utc>> {
        match self.0.get(k) {
            Some(Bson::DateTime(d)) => Some(dt::from_bson(*d)),
            _ => None,
        }
    }

    /// A required datetime with a `default_factory=utcnow`.
    pub fn dt(&self, k: &str) -> DateTime<Utc> {
        self.odt(k).unwrap_or_else(dt::now)
    }

    pub fn strs(&self, k: &str) -> Vec<String> {
        match self.0.get(k) {
            Some(Bson::Array(a)) => a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect(),
            _ => Vec::new(),
        }
    }

    pub fn oids(&self, k: &str) -> Vec<ObjectId> {
        match self.0.get(k) {
            Some(Bson::Array(a)) => a.iter().filter_map(Bson::as_object_id).collect(),
            _ => Vec::new(),
        }
    }

    pub fn map(&self, k: &str) -> StrMap {
        match self.0.get(k) {
            Some(Bson::Document(d)) => {
                d.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned()))).collect()
            }
            _ => Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Collaborator {
    pub user_id: ObjectId,
    pub username: String,
}

/// `ModProject`.
#[derive(Clone, Debug)]
pub struct Project {
    pub id: ObjectId,
    pub slug: String,
    pub title: String,
    pub summary: String,
    pub description: String,
    pub readme_text: String,
    pub warnings: String,
    pub title_i18n: StrMap,
    pub summary_i18n: StrMap,
    pub description_i18n: StrMap,
    pub readme_i18n: StrMap,
    pub warnings_i18n: StrMap,
    pub tags: Vec<String>,
    pub owner_id: Option<ObjectId>,
    pub owner_username: String,
    pub owner_handle: String,
    pub collaborators: Vec<Collaborator>,
    pub is_stray: bool,
    pub stray_status: Option<String>,
    pub author: String,
    pub source: Option<String>,
    pub source_id: Option<String>,
    pub source_url: Option<String>,
    pub source_author_id: Option<String>,
    pub source_file_id: Option<String>,
    pub source_likes: i64,
    pub uploaded_on_behalf: bool,
    pub issues_enabled: bool,
    pub issue_seq: i64,
    pub open_issue_count: i64,
    pub visibility: String,
    pub is_beta: bool,
    pub mode: String,
    pub source_visibility: String,
    pub default_branch: String,
    pub banner_sha: Option<String>,
    pub preview_shas: Vec<String>,
    pub discord_url: Option<String>,
    pub website_url: Option<String>,
    pub donation_urls: Vec<String>,
    pub download_count: i64,
    pub star_count: i64,
    pub downloads_7d: i64,
    pub popularity_score: f64,
    pub hidden_release_branches: Vec<String>,
    pub branch_order: Vec<String>,
    pub forked_from_id: Option<ObjectId>,
    pub forked_from_slug: Option<String>,
    pub forked_from_handle: Option<String>,
    pub forked_from_title: Option<String>,
    pub forked_from_owner: Option<String>,
    pub inspired_by_slug: Option<String>,
    pub inspired_by_handle: Option<String>,
    pub inspired_by_title: Option<String>,
    pub inspired_by_owner: Option<String>,
    pub fork_count: i64,
    pub is_remix: bool,
    pub remix_status: String,
    pub remix_requested_at: Option<DateTime<Utc>>,
    pub remix_resolved_at: Option<DateTime<Utc>>,
    pub remix_resolved_by: Option<ObjectId>,
    pub taken_down: bool,
    pub takedown_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_release_at: Option<DateTime<Utc>>,
}

impl Project {
    /// A fresh `ModProject(...)`: model defaults, `created_at`/`updated_at` now.
    pub fn new(slug: String, title: String) -> Self {
        let now = dt::now();
        Project {
            id: ObjectId::new(),
            slug,
            title,
            summary: String::new(),
            description: String::new(),
            readme_text: String::new(),
            warnings: String::new(),
            title_i18n: Vec::new(),
            summary_i18n: Vec::new(),
            description_i18n: Vec::new(),
            readme_i18n: Vec::new(),
            warnings_i18n: Vec::new(),
            tags: Vec::new(),
            owner_id: None,
            owner_username: String::new(),
            owner_handle: String::new(),
            collaborators: Vec::new(),
            is_stray: false,
            stray_status: None,
            author: String::new(),
            source: None,
            source_id: None,
            source_url: None,
            source_author_id: None,
            source_file_id: None,
            source_likes: 0,
            uploaded_on_behalf: false,
            issues_enabled: true,
            issue_seq: 0,
            open_issue_count: 0,
            visibility: "draft".into(),
            is_beta: false,
            mode: "files".into(),
            source_visibility: "public".into(),
            default_branch: "main".into(),
            banner_sha: None,
            preview_shas: Vec::new(),
            discord_url: None,
            website_url: None,
            donation_urls: Vec::new(),
            download_count: 0,
            star_count: 0,
            downloads_7d: 0,
            popularity_score: 0.0,
            hidden_release_branches: Vec::new(),
            branch_order: Vec::new(),
            forked_from_id: None,
            forked_from_slug: None,
            forked_from_handle: None,
            forked_from_title: None,
            forked_from_owner: None,
            inspired_by_slug: None,
            inspired_by_handle: None,
            inspired_by_title: None,
            inspired_by_owner: None,
            fork_count: 0,
            is_remix: false,
            remix_status: "pending".into(),
            remix_requested_at: None,
            remix_resolved_at: None,
            remix_resolved_by: None,
            taken_down: false,
            takedown_reason: None,
            created_at: now,
            updated_at: now,
            last_release_at: None,
        }
    }

    pub fn from_doc(d: &Document) -> Self {
        let r = Rd(d);
        let collaborators = match d.get("collaborators") {
            Some(Bson::Array(a)) => a
                .iter()
                .filter_map(|c| {
                    let c = c.as_document()?;
                    let r = Rd(c);
                    Some(Collaborator { user_id: r.oid("user_id")?, username: r.s("username", "") })
                })
                .collect(),
            _ => Vec::new(),
        };
        Project {
            id: r.id(),
            slug: r.s("slug", ""),
            title: r.s("title", ""),
            summary: r.s("summary", ""),
            description: r.s("description", ""),
            readme_text: r.s("readme_text", ""),
            warnings: r.s("warnings", ""),
            title_i18n: r.map("title_i18n"),
            summary_i18n: r.map("summary_i18n"),
            description_i18n: r.map("description_i18n"),
            readme_i18n: r.map("readme_i18n"),
            warnings_i18n: r.map("warnings_i18n"),
            tags: r.strs("tags"),
            owner_id: r.oid("owner_id"),
            owner_username: r.s("owner_username", ""),
            owner_handle: r.s("owner_handle", ""),
            collaborators,
            is_stray: r.b("is_stray", false),
            stray_status: r.os("stray_status"),
            author: r.s("author", ""),
            source: r.os("source"),
            source_id: r.os("source_id"),
            source_url: r.os("source_url"),
            source_author_id: r.os("source_author_id"),
            source_file_id: r.os("source_file_id"),
            source_likes: r.i("source_likes", 0),
            uploaded_on_behalf: r.b("uploaded_on_behalf", false),
            issues_enabled: r.b("issues_enabled", true),
            issue_seq: r.i("issue_seq", 0),
            open_issue_count: r.i("open_issue_count", 0),
            visibility: r.s("visibility", "draft"),
            is_beta: r.b("is_beta", false),
            mode: r.s("mode", "files"),
            source_visibility: r.s("source_visibility", "public"),
            default_branch: r.s("default_branch", "main"),
            banner_sha: r.os("banner_sha"),
            preview_shas: r.strs("preview_shas"),
            discord_url: r.os("discord_url"),
            website_url: r.os("website_url"),
            donation_urls: r.strs("donation_urls"),
            download_count: r.i("download_count", 0),
            star_count: r.i("star_count", 0),
            downloads_7d: r.i("downloads_7d", 0),
            popularity_score: r.f("popularity_score", 0.0),
            hidden_release_branches: r.strs("hidden_release_branches"),
            branch_order: r.strs("branch_order"),
            forked_from_id: r.oid("forked_from_id"),
            forked_from_slug: r.os("forked_from_slug"),
            forked_from_handle: r.os("forked_from_handle"),
            forked_from_title: r.os("forked_from_title"),
            forked_from_owner: r.os("forked_from_owner"),
            inspired_by_slug: r.os("inspired_by_slug"),
            inspired_by_handle: r.os("inspired_by_handle"),
            inspired_by_title: r.os("inspired_by_title"),
            inspired_by_owner: r.os("inspired_by_owner"),
            fork_count: r.i("fork_count", 0),
            is_remix: r.b("is_remix", false),
            remix_status: r.s("remix_status", "pending"),
            remix_requested_at: r.odt("remix_requested_at"),
            remix_resolved_at: r.odt("remix_resolved_at"),
            remix_resolved_by: r.oid("remix_resolved_by"),
            taken_down: r.b("taken_down", false),
            takedown_reason: r.os("takedown_reason"),
            created_at: r.dt("created_at"),
            updated_at: r.dt("updated_at"),
            last_release_at: r.odt("last_release_at"),
        }
    }

    /// `get_dict(project, to_db=True)`.
    pub fn to_doc(&self) -> Document {
        let mut d = Document::new();
        d.insert("_id", self.id);
        d.insert("slug", self.slug.clone());
        d.insert("title", self.title.clone());
        d.insert("summary", self.summary.clone());
        d.insert("description", self.description.clone());
        d.insert("readme_text", self.readme_text.clone());
        d.insert("warnings", self.warnings.clone());
        d.insert("title_i18n", map_bson(&self.title_i18n));
        d.insert("summary_i18n", map_bson(&self.summary_i18n));
        d.insert("description_i18n", map_bson(&self.description_i18n));
        d.insert("readme_i18n", map_bson(&self.readme_i18n));
        d.insert("warnings_i18n", map_bson(&self.warnings_i18n));
        d.insert("tags", strs_bson(&self.tags));
        d.insert("owner_id", opt_oid(&self.owner_id));
        d.insert("owner_username", self.owner_username.clone());
        d.insert("owner_handle", self.owner_handle.clone());
        let collabs: Vec<Bson> = self
            .collaborators
            .iter()
            .map(|c| {
                let mut cd = Document::new();
                cd.insert("user_id", c.user_id);
                cd.insert("username", c.username.clone());
                Bson::Document(cd)
            })
            .collect();
        d.insert("collaborators", collabs);
        d.insert("is_stray", self.is_stray);
        d.insert("stray_status", opt_str(&self.stray_status));
        d.insert("author", self.author.clone());
        d.insert("source", opt_str(&self.source));
        d.insert("source_id", opt_str(&self.source_id));
        d.insert("source_url", opt_str(&self.source_url));
        d.insert("source_author_id", opt_str(&self.source_author_id));
        d.insert("source_file_id", opt_str(&self.source_file_id));
        d.insert("source_likes", bson_int(self.source_likes));
        d.insert("uploaded_on_behalf", self.uploaded_on_behalf);
        d.insert("issues_enabled", self.issues_enabled);
        d.insert("issue_seq", bson_int(self.issue_seq));
        d.insert("open_issue_count", bson_int(self.open_issue_count));
        d.insert("visibility", self.visibility.clone());
        d.insert("is_beta", self.is_beta);
        d.insert("mode", self.mode.clone());
        d.insert("source_visibility", self.source_visibility.clone());
        d.insert("default_branch", self.default_branch.clone());
        d.insert("banner_sha", opt_str(&self.banner_sha));
        d.insert("preview_shas", strs_bson(&self.preview_shas));
        d.insert("discord_url", opt_str(&self.discord_url));
        d.insert("website_url", opt_str(&self.website_url));
        d.insert("donation_urls", strs_bson(&self.donation_urls));
        d.insert("download_count", bson_int(self.download_count));
        d.insert("star_count", bson_int(self.star_count));
        d.insert("downloads_7d", bson_int(self.downloads_7d));
        d.insert("popularity_score", self.popularity_score);
        d.insert("hidden_release_branches", strs_bson(&self.hidden_release_branches));
        d.insert("branch_order", strs_bson(&self.branch_order));
        d.insert("forked_from_id", opt_oid(&self.forked_from_id));
        d.insert("forked_from_slug", opt_str(&self.forked_from_slug));
        d.insert("forked_from_handle", opt_str(&self.forked_from_handle));
        d.insert("forked_from_title", opt_str(&self.forked_from_title));
        d.insert("forked_from_owner", opt_str(&self.forked_from_owner));
        d.insert("inspired_by_slug", opt_str(&self.inspired_by_slug));
        d.insert("inspired_by_handle", opt_str(&self.inspired_by_handle));
        d.insert("inspired_by_title", opt_str(&self.inspired_by_title));
        d.insert("inspired_by_owner", opt_str(&self.inspired_by_owner));
        d.insert("fork_count", bson_int(self.fork_count));
        d.insert("is_remix", self.is_remix);
        d.insert("remix_status", self.remix_status.clone());
        d.insert("remix_requested_at", opt_date(&self.remix_requested_at));
        d.insert("remix_resolved_at", opt_date(&self.remix_resolved_at));
        d.insert("remix_resolved_by", opt_oid(&self.remix_resolved_by));
        d.insert("taken_down", self.taken_down);
        d.insert("takedown_reason", opt_str(&self.takedown_reason));
        d.insert("created_at", date(&self.created_at));
        d.insert("updated_at", date(&self.updated_at));
        d.insert("last_release_at", opt_date(&self.last_release_at));
        d
    }
}

/// `ModRelease`.
#[derive(Clone, Debug)]
pub struct Release {
    pub id: ObjectId,
    pub project_id: ObjectId,
    pub owner_id: Option<ObjectId>,
    pub tag: String,
    pub branch: String,
    pub title: String,
    pub changelog: String,
    pub title_i18n: StrMap,
    pub changelog_i18n: StrMap,
    pub source_commit_sha: Option<String>,
    pub release_format: String,
    pub tmod_sha: String,
    pub prior_tmod_shas: Vec<String>,
    pub tmod_size: i64,
    pub tmod_filename: String,
    pub tmod_properties: StrMap,
    pub banner_sha: Option<String>,
    pub download_count: i64,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub published_at: Option<DateTime<Utc>>,
    pub announced_at: Option<DateTime<Utc>>,
    pub silent: bool,
}

impl Release {
    pub fn from_doc(d: &Document) -> Self {
        let r = Rd(d);
        Release {
            id: r.id(),
            project_id: r.oid("project_id").unwrap_or_else(|| ObjectId::from_bytes([0; 12])),
            owner_id: r.oid("owner_id"),
            tag: r.s("tag", ""),
            branch: r.s("branch", "main"),
            title: r.s("title", ""),
            changelog: r.s("changelog", ""),
            title_i18n: r.map("title_i18n"),
            changelog_i18n: r.map("changelog_i18n"),
            source_commit_sha: r.os("source_commit_sha"),
            release_format: r.s("release_format", "tmod"),
            tmod_sha: r.s("tmod_sha", ""),
            prior_tmod_shas: r.strs("prior_tmod_shas"),
            tmod_size: r.i("tmod_size", 0),
            tmod_filename: r.s("tmod_filename", "mod.tmod"),
            tmod_properties: r.map("tmod_properties"),
            banner_sha: r.os("banner_sha"),
            download_count: r.i("download_count", 0),
            status: r.s("status", "published"),
            created_at: r.dt("created_at"),
            updated_at: r.dt("updated_at"),
            published_at: r.odt("published_at"),
            announced_at: r.odt("announced_at"),
            silent: r.b("silent", false),
        }
    }

    pub fn to_doc(&self) -> Document {
        let mut d = Document::new();
        d.insert("_id", self.id);
        d.insert("project_id", self.project_id);
        d.insert("owner_id", opt_oid(&self.owner_id));
        d.insert("tag", self.tag.clone());
        d.insert("branch", self.branch.clone());
        d.insert("title", self.title.clone());
        d.insert("changelog", self.changelog.clone());
        d.insert("title_i18n", map_bson(&self.title_i18n));
        d.insert("changelog_i18n", map_bson(&self.changelog_i18n));
        d.insert("source_commit_sha", opt_str(&self.source_commit_sha));
        d.insert("release_format", self.release_format.clone());
        d.insert("tmod_sha", self.tmod_sha.clone());
        d.insert("prior_tmod_shas", strs_bson(&self.prior_tmod_shas));
        d.insert("tmod_size", bson_int(self.tmod_size));
        d.insert("tmod_filename", self.tmod_filename.clone());
        d.insert("tmod_properties", map_bson(&self.tmod_properties));
        d.insert("banner_sha", opt_str(&self.banner_sha));
        d.insert("download_count", bson_int(self.download_count));
        d.insert("status", self.status.clone());
        d.insert("created_at", date(&self.created_at));
        d.insert("updated_at", date(&self.updated_at));
        d.insert("published_at", opt_date(&self.published_at));
        d.insert("announced_at", opt_date(&self.announced_at));
        d.insert("silent", self.silent);
        d
    }
}

/// `ModProfile`.
#[derive(Clone, Debug)]
pub struct Profile {
    pub id: ObjectId,
    pub site_user_id: ObjectId,
    pub handle: String,
    pub display_name: String,
    pub tagline: String,
    pub readme: String,
    pub tagline_i18n: StrMap,
    pub readme_i18n: StrMap,
    pub avatar_sha: Option<String>,
    pub banner_sha: Option<String>,
    pub discord_url: Option<String>,
    pub website_url: Option<String>,
    pub donation_urls: Vec<String>,
    pub mod_order: Vec<String>,
    pub featured_slug: Option<String>,
    pub taken_down: bool,
    pub takedown_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Profile {
    pub fn new(site_user_id: ObjectId, handle: String) -> Self {
        let now = dt::now();
        Profile {
            id: ObjectId::new(),
            site_user_id,
            handle,
            display_name: String::new(),
            tagline: String::new(),
            readme: String::new(),
            tagline_i18n: Vec::new(),
            readme_i18n: Vec::new(),
            avatar_sha: None,
            banner_sha: None,
            discord_url: None,
            website_url: None,
            donation_urls: Vec::new(),
            mod_order: Vec::new(),
            featured_slug: None,
            taken_down: false,
            takedown_reason: None,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn from_doc(d: &Document) -> Self {
        let r = Rd(d);
        Profile {
            id: r.id(),
            site_user_id: r.oid("site_user_id").unwrap_or_else(|| ObjectId::from_bytes([0; 12])),
            handle: r.s("handle", ""),
            display_name: r.s("display_name", ""),
            tagline: r.s("tagline", ""),
            readme: r.s("readme", ""),
            tagline_i18n: r.map("tagline_i18n"),
            readme_i18n: r.map("readme_i18n"),
            avatar_sha: r.os("avatar_sha"),
            banner_sha: r.os("banner_sha"),
            discord_url: r.os("discord_url"),
            website_url: r.os("website_url"),
            donation_urls: r.strs("donation_urls"),
            mod_order: r.strs("mod_order"),
            featured_slug: r.os("featured_slug"),
            taken_down: r.b("taken_down", false),
            takedown_reason: r.os("takedown_reason"),
            created_at: r.dt("created_at"),
            updated_at: r.dt("updated_at"),
        }
    }

    pub fn to_doc(&self) -> Document {
        let mut d = Document::new();
        d.insert("_id", self.id);
        d.insert("site_user_id", self.site_user_id);
        d.insert("handle", self.handle.clone());
        d.insert("display_name", self.display_name.clone());
        d.insert("tagline", self.tagline.clone());
        d.insert("readme", self.readme.clone());
        d.insert("tagline_i18n", map_bson(&self.tagline_i18n));
        d.insert("readme_i18n", map_bson(&self.readme_i18n));
        d.insert("avatar_sha", opt_str(&self.avatar_sha));
        d.insert("banner_sha", opt_str(&self.banner_sha));
        d.insert("discord_url", opt_str(&self.discord_url));
        d.insert("website_url", opt_str(&self.website_url));
        d.insert("donation_urls", strs_bson(&self.donation_urls));
        d.insert("mod_order", strs_bson(&self.mod_order));
        d.insert("featured_slug", opt_str(&self.featured_slug));
        d.insert("taken_down", self.taken_down);
        d.insert("takedown_reason", opt_str(&self.takedown_reason));
        d.insert("created_at", date(&self.created_at));
        d.insert("updated_at", date(&self.updated_at));
        d
    }
}

/// `ModImageAsset` as `store_image` inserts it.
#[derive(Clone, Debug)]
pub struct ImageAsset {
    pub id: ObjectId,
    pub sha: String,
    pub content_type: String,
    pub byte_size: i64,
    pub owner_id: ObjectId,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub created_at: DateTime<Utc>,
}

impl ImageAsset {
    pub fn to_doc(&self) -> Document {
        let mut d = Document::new();
        d.insert("_id", self.id);
        d.insert("sha", self.sha.clone());
        d.insert("content_type", self.content_type.clone());
        d.insert("byte_size", bson_int(self.byte_size));
        d.insert("owner_id", self.owner_id);
        d.insert("width", self.width.map_or(Bson::Null, bson_int));
        d.insert("height", self.height.map_or(Bson::Null, bson_int));
        d.insert("variants", Document::new());
        d.insert("created_at", date(&self.created_at));
        d
    }
}

/// `ModStar`.
pub fn star_doc(id: ObjectId, project_id: ObjectId, site_user_id: ObjectId, created_at: DateTime<Utc>) -> Document {
    let mut d = Document::new();
    d.insert("_id", id);
    d.insert("project_id", project_id);
    d.insert("site_user_id", site_user_id);
    d.insert("created_at", date(&created_at));
    d
}

/// `ModClaimRequest`.
#[derive(Clone, Debug)]
pub struct Claim {
    pub id: ObjectId,
    pub project_id: ObjectId,
    pub project_slug: String,
    pub project_title: String,
    pub claimant_id: ObjectId,
    pub claimant_username: String,
    pub message: String,
    pub status: String,
    pub resolved_by: Option<ObjectId>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl Claim {
    pub fn from_doc(d: &Document) -> Self {
        let r = Rd(d);
        Claim {
            id: r.id(),
            project_id: r.oid("project_id").unwrap_or_else(|| ObjectId::from_bytes([0; 12])),
            project_slug: r.s("project_slug", ""),
            project_title: r.s("project_title", ""),
            claimant_id: r.oid("claimant_id").unwrap_or_else(|| ObjectId::from_bytes([0; 12])),
            claimant_username: r.s("claimant_username", ""),
            message: r.s("message", ""),
            status: r.s("status", "pending"),
            resolved_by: r.oid("resolved_by"),
            resolved_at: r.odt("resolved_at"),
            created_at: r.dt("created_at"),
        }
    }

    pub fn to_doc(&self) -> Document {
        let mut d = Document::new();
        d.insert("_id", self.id);
        d.insert("project_id", self.project_id);
        d.insert("project_slug", self.project_slug.clone());
        d.insert("project_title", self.project_title.clone());
        d.insert("claimant_id", self.claimant_id);
        d.insert("claimant_username", self.claimant_username.clone());
        d.insert("message", self.message.clone());
        d.insert("status", self.status.clone());
        d.insert("resolved_by", opt_oid(&self.resolved_by));
        d.insert("resolved_at", opt_date(&self.resolved_at));
        d.insert("created_at", date(&self.created_at));
        d
    }
}

/// `RemixClaim`.
#[derive(Clone, Debug)]
pub struct RemixClaim {
    pub id: ObjectId,
    pub target_id: ObjectId,
    pub target_slug: String,
    pub target_title: String,
    pub original_id: ObjectId,
    pub original_slug: String,
    pub original_title: String,
    pub claimant_id: ObjectId,
    pub claimant_username: String,
    pub message: String,
    pub status: String,
    pub resolved_by: Option<ObjectId>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl RemixClaim {
    pub fn from_doc(d: &Document) -> Self {
        let r = Rd(d);
        let zero = || ObjectId::from_bytes([0; 12]);
        RemixClaim {
            id: r.id(),
            target_id: r.oid("target_id").unwrap_or_else(zero),
            target_slug: r.s("target_slug", ""),
            target_title: r.s("target_title", ""),
            original_id: r.oid("original_id").unwrap_or_else(zero),
            original_slug: r.s("original_slug", ""),
            original_title: r.s("original_title", ""),
            claimant_id: r.oid("claimant_id").unwrap_or_else(zero),
            claimant_username: r.s("claimant_username", ""),
            message: r.s("message", ""),
            status: r.s("status", "pending"),
            resolved_by: r.oid("resolved_by"),
            resolved_at: r.odt("resolved_at"),
            created_at: r.dt("created_at"),
        }
    }

    pub fn to_doc(&self) -> Document {
        let mut d = Document::new();
        d.insert("_id", self.id);
        d.insert("target_id", self.target_id);
        d.insert("target_slug", self.target_slug.clone());
        d.insert("target_title", self.target_title.clone());
        d.insert("original_id", self.original_id);
        d.insert("original_slug", self.original_slug.clone());
        d.insert("original_title", self.original_title.clone());
        d.insert("claimant_id", self.claimant_id);
        d.insert("claimant_username", self.claimant_username.clone());
        d.insert("message", self.message.clone());
        d.insert("status", self.status.clone());
        d.insert("resolved_by", opt_oid(&self.resolved_by));
        d.insert("resolved_at", opt_date(&self.resolved_at));
        d.insert("created_at", date(&self.created_at));
        d
    }
}

/// The parts of `ModCreatorLink` the write gate reads.
#[derive(Clone, Debug)]
pub struct CreatorLink {
    pub id: ObjectId,
    pub site_user_id: ObjectId,
    pub all_projects: bool,
    pub project_ids: Vec<ObjectId>,
}

impl CreatorLink {
    pub fn from_doc(d: &Document) -> Self {
        let r = Rd(d);
        CreatorLink {
            id: r.id(),
            site_user_id: r.oid("site_user_id").unwrap_or_else(|| ObjectId::from_bytes([0; 12])),
            all_projects: r.b("all_projects", true),
            project_ids: r.oids("project_ids"),
        }
    }
}
