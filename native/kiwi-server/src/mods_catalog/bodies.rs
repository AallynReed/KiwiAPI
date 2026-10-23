//! The request models of `mods_hub/schemas.py` this component takes.

use crate::validation::{Field, FromBody};

pub const VISIBILITIES: &[&str] = &["draft", "unlisted", "public", "public_website_only", "hidden"];
pub const MODES: &[&str] = &["files", "releases"];
pub const SOURCE_VISIBILITIES: &[&str] = &["public", "private"];

pub type I18n = Vec<(String, String)>;

fn i18n(f: Field<'_>) -> Option<I18n> {
    f.dict(|v| v.str().required()).optional()
}

fn strs(f: Field<'_>, max: usize) -> Option<Vec<String>> {
    f.list(|v| v.str().required()).max_len(max).optional()
}

/// `CreateProjectRequest`.
#[derive(Default)]
pub struct CreateProject {
    pub title: String,
    pub summary: String,
    pub description: String,
    pub tags: Vec<String>,
    pub visibility: String,
    pub is_beta: bool,
    pub mode: String,
    pub source_visibility: String,
    pub inspired_by: Option<String>,
    pub on_behalf: bool,
    pub credited_author: Option<String>,
}

impl FromBody for CreateProject {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(CreateProject {
                title: m.field("title").str().min_len(1).max_len(120).required(),
                summary: m.field("summary").str().max_len(280).default(String::new()),
                description: m.field("description").str().max_len(40_000).default(String::new()),
                tags: m.field("tags").list(|v| v.str().required()).max_len(12).default(Vec::new()),
                visibility: m.field("visibility").literal(VISIBILITIES).default("draft".into()),
                is_beta: m.field("is_beta").bool().default(false),
                mode: m.field("mode").literal(MODES).default("files".into()),
                source_visibility: m.field("source_visibility").literal(SOURCE_VISIBILITIES).default("public".into()),
                inspired_by: m.field("inspired_by").str().max_len(80).optional(),
                on_behalf: m.field("on_behalf").bool().default(false),
                credited_author: m.field("credited_author").str().max_len(120).optional(),
            })
        })
        .required()
    }
}

/// `UpdateProjectRequest`, with `exclude_unset` folded in: an explicit null
/// is a no-op in the service, so absent and null are both `None`.
#[derive(Default)]
pub struct UpdateProject {
    pub title: Option<String>,
    pub title_i18n: Option<I18n>,
    pub summary: Option<String>,
    pub summary_i18n: Option<I18n>,
    pub description: Option<String>,
    pub description_i18n: Option<I18n>,
    pub readme_text: Option<String>,
    pub readme_i18n: Option<I18n>,
    pub warnings: Option<String>,
    pub warnings_i18n: Option<I18n>,
    pub tags: Option<Vec<String>>,
    pub visibility: Option<String>,
    pub is_beta: Option<bool>,
    pub issues_enabled: Option<bool>,
    pub mode: Option<String>,
    pub source_visibility: Option<String>,
    pub hidden_release_branches: Option<Vec<String>>,
    pub branch_order: Option<Vec<String>>,
    pub discord_url: Option<String>,
    pub website_url: Option<String>,
    pub donation_urls: Option<Vec<String>>,
    pub inspired_by: Option<String>,
}

impl FromBody for UpdateProject {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(UpdateProject {
                title: m.field("title").str().min_len(1).max_len(120).optional(),
                title_i18n: i18n(m.field("title_i18n")),
                summary: m.field("summary").str().max_len(280).optional(),
                summary_i18n: i18n(m.field("summary_i18n")),
                description: m.field("description").str().max_len(40_000).optional(),
                description_i18n: i18n(m.field("description_i18n")),
                readme_text: m.field("readme_text").str().max_len(60_000).optional(),
                readme_i18n: i18n(m.field("readme_i18n")),
                warnings: m.field("warnings").str().max_len(4_000).optional(),
                warnings_i18n: i18n(m.field("warnings_i18n")),
                tags: strs(m.field("tags"), 12),
                visibility: m.field("visibility").literal(VISIBILITIES).optional(),
                is_beta: m.field("is_beta").bool().optional(),
                issues_enabled: m.field("issues_enabled").bool().optional(),
                mode: m.field("mode").literal(MODES).optional(),
                source_visibility: m.field("source_visibility").literal(SOURCE_VISIBILITIES).optional(),
                hidden_release_branches: strs(m.field("hidden_release_branches"), 100),
                branch_order: strs(m.field("branch_order"), 100),
                discord_url: m.field("discord_url").str().max_len(300).optional(),
                website_url: m.field("website_url").str().max_len(300).optional(),
                donation_urls: strs(m.field("donation_urls"), 5),
                inspired_by: m.field("inspired_by").str().max_len(80).optional(),
            })
        })
        .required()
    }
}

/// `UpdateProfileRequest`.
#[derive(Default)]
pub struct UpdateProfile {
    pub display_name: Option<String>,
    pub tagline: Option<String>,
    pub tagline_i18n: Option<I18n>,
    pub readme: Option<String>,
    pub readme_i18n: Option<I18n>,
    pub discord_url: Option<String>,
    pub website_url: Option<String>,
    pub donation_urls: Option<Vec<String>>,
    pub mod_order: Option<Vec<String>>,
    pub featured_slug: Option<String>,
}

impl FromBody for UpdateProfile {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(UpdateProfile {
                display_name: m.field("display_name").str().max_len(80).optional(),
                tagline: m.field("tagline").str().max_len(160).optional(),
                tagline_i18n: i18n(m.field("tagline_i18n")),
                readme: m.field("readme").str().max_len(40_000).optional(),
                readme_i18n: i18n(m.field("readme_i18n")),
                discord_url: m.field("discord_url").str().max_len(300).optional(),
                website_url: m.field("website_url").str().max_len(300).optional(),
                donation_urls: strs(m.field("donation_urls"), 5),
                mod_order: strs(m.field("mod_order"), 500),
                featured_slug: m.field("featured_slug").str().max_len(80).optional(),
            })
        })
        .required()
    }
}

/// `ClaimRequest`.
#[derive(Default)]
pub struct ClaimBody {
    pub message: String,
}

impl FromBody for ClaimBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| Some(ClaimBody { message: m.field("message").str().max_len(2000).default(String::new()) }))
            .required()
    }
}

/// `RemixStatusRequest`.
#[derive(Default)]
pub struct RemixStatus {
    pub status: String,
}

impl FromBody for RemixStatus {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(RemixStatus { status: m.field("status").literal(&["approved", "denied", "pending"]).required() })
        })
        .required()
    }
}

/// `RemixClaimRequest`.
#[derive(Default)]
pub struct RemixClaimBody {
    pub target_handle: String,
    pub target_slug: String,
    pub message: String,
}

impl FromBody for RemixClaimBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(RemixClaimBody {
                target_handle: m.field("target_handle").str().min_len(1).max_len(80).required(),
                target_slug: m.field("target_slug").str().min_len(1).max_len(120).required(),
                message: m.field("message").str().max_len(2000).default(String::new()),
            })
        })
        .required()
    }
}

/// `CollaboratorRequest`.
#[derive(Default)]
pub struct CollaboratorBody {
    pub username: String,
}

impl FromBody for CollaboratorBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| Some(CollaboratorBody { username: m.field("username").str().min_len(1).max_len(80).required() }))
            .required()
    }
}

/// `HashLookupRequest`.
#[derive(Default)]
pub struct Lookup {
    pub hashes: Vec<String>,
    pub releases: Option<String>,
    pub include_releases: bool,
}

impl Lookup {
    /// `release_mode`.
    pub fn mode(&self) -> &str {
        match &self.releases {
            Some(r) => r,
            None if self.include_releases => "latest",
            None => "none",
        }
    }
}

impl FromBody for Lookup {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(Lookup {
                hashes: m.field("hashes").list(|v| v.str().required()).min_len(1).max_len(200).required(),
                releases: m.field("releases").literal(&["none", "latest", "all"]).optional(),
                include_releases: m.field("include_releases").bool().default(false),
            })
        })
        .required()
    }
}
