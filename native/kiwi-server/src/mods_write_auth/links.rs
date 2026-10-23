//! `ModCreatorLink` and the three `creators.py` helpers the write gate uses.

use chrono::{DateTime, Utc};
use mongodb::bson::{Bson, Document, doc, oid::ObjectId};

use super::{
    docs::{date, int, opt_date, read_bool, read_date, read_ids, read_int, read_opt_date, read_str},
    project::Project,
    store::{Res, Store},
};

pub const COLLECTION: &str = "mod_creator_links";

#[derive(Debug, Clone)]
pub struct CreatorLink {
    pub id: ObjectId,
    pub site_user_id: ObjectId,
    pub api_user_id: ObjectId,
    pub label: String,
    pub all_projects: bool,
    pub project_ids: Vec<ObjectId>,
    pub revoked: bool,
    pub revoked_at: Option<DateTime<Utc>>,
    pub revoked_by_rotation: bool,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub request_count: i64,
}

impl CreatorLink {
    /// A fresh connection as `ModCreatorLink(...)` builds it (id from the insert).
    pub fn new(site_user_id: ObjectId, api_user_id: ObjectId, label: String, now: DateTime<Utc>) -> Self {
        CreatorLink {
            id: ObjectId::from_bytes([0; 12]),
            site_user_id,
            api_user_id,
            label,
            all_projects: true,
            project_ids: Vec::new(),
            revoked: false,
            revoked_at: None,
            revoked_by_rotation: false,
            created_at: now,
            last_used_at: None,
            request_count: 0,
        }
    }

    pub fn from_doc(d: &Document) -> Self {
        CreatorLink {
            id: d.get_object_id("_id").unwrap_or_else(|_| ObjectId::from_bytes([0; 12])),
            site_user_id: d.get_object_id("site_user_id").unwrap_or_else(|_| ObjectId::from_bytes([0; 12])),
            api_user_id: d.get_object_id("api_user_id").unwrap_or_else(|_| ObjectId::from_bytes([0; 12])),
            label: read_str(d, "label"),
            all_projects: read_bool(d, "all_projects", true),
            project_ids: read_ids(d, "project_ids"),
            revoked: read_bool(d, "revoked", false),
            revoked_at: read_opt_date(d, "revoked_at"),
            revoked_by_rotation: read_bool(d, "revoked_by_rotation", false),
            created_at: read_date(d, "created_at"),
            last_used_at: read_opt_date(d, "last_used_at"),
            request_count: read_int(d, "request_count", 0),
        }
    }

    /// The document in model order, without `_id` (an insert adds it first).
    pub fn fields(&self) -> Document {
        doc! {
            "site_user_id": self.site_user_id,
            "api_user_id": self.api_user_id,
            "label": &self.label,
            "all_projects": self.all_projects,
            "project_ids": self.project_ids.iter().map(|i| Bson::ObjectId(*i)).collect::<Vec<_>>(),
            "revoked": self.revoked,
            "revoked_at": opt_date(self.revoked_at),
            "revoked_by_rotation": self.revoked_by_rotation,
            "created_at": date(self.created_at),
            "last_used_at": opt_date(self.last_used_at),
            "request_count": int(self.request_count),
        }
    }

    /// Beanie `save()`, after which the instance holds what Mongo stored.
    pub async fn save(&mut self, store: &Store) -> Res<()> {
        let set = crate::outbound::store::with_id(self.id, self.fields());
        store.save(COLLECTION, set).await?;
        self.revoked_at = self.revoked_at.map(crate::dt::mongo_round_trip);
        self.created_at = crate::dt::mongo_round_trip(self.created_at);
        self.last_used_at = self.last_used_at.map(crate::dt::mongo_round_trip);
        Ok(())
    }
}

/// `live_links(api_user)`.
pub async fn live_links(store: &Store, api_user_id: ObjectId) -> Res<Vec<CreatorLink>> {
    let docs =
        store.find(COLLECTION, doc! {"$and": [{"api_user_id": api_user_id}, {"revoked": false}]}, None, 0, 0).await?;
    Ok(docs.iter().map(CreatorLink::from_doc).collect())
}

/// `covers(link, project)`.
pub fn covers(link: &CreatorLink, project: &Project) -> bool {
    if project.owner_id != Some(link.site_user_id) {
        return false;
    }
    link.all_projects || link.project_ids.contains(&project.id)
}

/// `touch(link)`: best effort, never fails the request.
pub async fn touch(store: &Store, link: &CreatorLink) {
    let update = doc! {"$set": {"last_used_at": date(store.now())}, "$inc": {"request_count": 1_i32}};
    let _ = store.find_one_and_update(COLLECTION, doc! {"_id": link.id}, update).await;
}
