//! `app/trove/modpacks/schemas.py` request bodies and the query parameters
//! of the modpack routes.

use crate::validation::{Field, FromBody, FromParams, Params};

use super::model::VISIBILITY;

pub struct CreateBody {
    pub title: String,
    pub summary: String,
    pub description: String,
    pub tags: Vec<String>,
    pub visibility: String,
}

impl FromBody for CreateBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(CreateBody {
                title: m.field("title").str().min_len(1).max_len(120).required(),
                summary: m.field("summary").str().max_len(280).default(String::new()),
                description: m.field("description").str().max_len(40_000).default(String::new()),
                tags: m.field("tags").list(|f| f.str().required()).max_len(12).default(Vec::new()),
                visibility: m.field("visibility").literal(VISIBILITY).default("draft".into()),
            })
        })
        .required()
    }
}

#[derive(Default)]
pub struct UpdateBody {
    pub title: Option<String>,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub warnings: Option<String>,
    pub tags: Option<Vec<String>>,
    pub visibility: Option<String>,
    pub discord_url: Option<String>,
    pub website_url: Option<String>,
    pub donation_urls: Option<Vec<String>>,
    pub default_variant: Option<String>,
    pub variant_order: Option<Vec<String>>,
}

impl FromBody for UpdateBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(UpdateBody {
                title: m.field("title").str().min_len(1).max_len(120).optional(),
                summary: m.field("summary").str().max_len(280).optional(),
                description: m.field("description").str().max_len(40_000).optional(),
                warnings: m.field("warnings").str().max_len(4_000).optional(),
                tags: m.field("tags").list(|f| f.str().required()).max_len(12).optional(),
                visibility: m.field("visibility").literal(VISIBILITY).optional(),
                discord_url: m.field("discord_url").str().max_len(300).optional(),
                website_url: m.field("website_url").str().max_len(300).optional(),
                donation_urls: m.field("donation_urls").list(|f| f.str().required()).max_len(5).optional(),
                default_variant: m.field("default_variant").str().max_len(80).optional(),
                variant_order: m.field("variant_order").list(|f| f.str().required()).max_len(100).optional(),
            })
        })
        .required()
    }
}

pub struct CreateVariantBody {
    pub name: String,
    pub copy_from: Option<String>,
}

impl FromBody for CreateVariantBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(CreateVariantBody {
                name: m.field("name").str().min_len(1).max_len(80).required(),
                copy_from: m.field("copy_from").str().max_len(80).optional(),
            })
        })
        .required()
    }
}

pub struct UpdateVariantBody {
    pub label: Option<String>,
}

impl FromBody for UpdateVariantBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| Some(UpdateVariantBody { label: m.field("label").str().max_len(120).optional() })).required()
    }
}

#[derive(Clone, Debug, Default)]
pub struct EntryInput {
    pub handle: String,
    pub slug: String,
    pub branch: String,
    pub version_locked: bool,
    pub locked_tag: Option<String>,
    pub custom_sha: Option<String>,
    pub custom_filename: Option<String>,
    pub title: String,
    pub author: String,
}

pub struct SetEntriesBody {
    pub entries: Vec<EntryInput>,
}

impl FromBody for SetEntriesBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            let entries = m
                .field("entries")
                .list(|f| {
                    f.model(|e| EntryInput {
                        handle: e.field("handle").str().max_len(80).default(String::new()),
                        slug: e.field("slug").str().max_len(120).default(String::new()),
                        branch: e.field("branch").str().max_len(80).default("main".into()),
                        version_locked: e.field("version_locked").bool().default(false),
                        locked_tag: e.field("locked_tag").str().max_len(60).optional(),
                        custom_sha: e.field("custom_sha").str().max_len(128).optional(),
                        custom_filename: e.field("custom_filename").str().max_len(200).optional(),
                        title: e.field("title").str().max_len(200).default(String::new()),
                        author: e.field("author").str().max_len(200).default(String::new()),
                    })
                    .required()
                })
                .max_len(300)
                .default(Vec::new());
            Some(SetEntriesBody { entries })
        })
        .required()
    }
}

/// `mods_hub.schemas.CollaboratorRequest`.
pub struct CollaboratorBody {
    pub username: String,
}

impl FromBody for CollaboratorBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| Some(CollaboratorBody { username: m.field("username").str().min_len(1).max_len(80).required() }))
            .required()
    }
}

/// `mods_hub.schemas.TakedownRequest`.
pub struct TakedownBody {
    pub reason: String,
}

impl FromBody for TakedownBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| Some(TakedownBody { reason: m.field("reason").str().max_len(2000).default(String::new()) }))
            .required()
    }
}

/// The browse parameters shared by the hub, catalog and site lists.
pub struct ListQuery {
    pub q: Option<String>,
    pub tag: Option<String>,
    pub author: Option<String>,
    pub sort: String,
    pub limit: i64,
    pub offset: i64,
}

impl FromParams for ListQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        ListQuery {
            q: p.query("q").str().max_len(120).optional(),
            tag: p.query("tag").str().max_len(40).optional(),
            author: p.query("author").str().max_len(80).optional(),
            sort: p.query("sort").str().default("recent".into()),
            limit: p.query("limit").int().ge(1).le(100).default(30),
            offset: p.query("offset").int().ge(0).default(0),
        }
    }
}

/// `variant` and `format` of a download; `/site` defaults to a zip.
pub struct DownloadQuery {
    pub variant: Option<String>,
    pub format: String,
}

fn download(p: &mut Params<'_>, default: &str) -> DownloadQuery {
    DownloadQuery {
        variant: p.query("variant").str().max_len(80).optional(),
        format: p.query("format").str().one_of_pattern(&["tpack", "zip", "configs"]).default(default.into()),
    }
}

pub struct ApiDownload(pub DownloadQuery);

impl FromParams for ApiDownload {
    fn from_params(p: &mut Params<'_>) -> Self {
        ApiDownload(download(p, "tpack"))
    }
}

pub struct SiteDownload(pub DownloadQuery);

impl FromParams for SiteDownload {
    fn from_params(p: &mut Params<'_>) -> Self {
        SiteDownload(download(p, "zip"))
    }
}

pub struct AdminListQuery {
    pub q: Option<String>,
    pub owner: Option<String>,
    pub visibility: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

impl FromParams for AdminListQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        AdminListQuery {
            q: p.query("q").str().optional(),
            owner: p.query("owner").str().optional(),
            visibility: p.query("visibility").str().optional(),
            limit: p.query("limit").int().ge(1).le(200).default(50),
            offset: p.query("offset").int().ge(0).default(0),
        }
    }
}
