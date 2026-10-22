//! The Beanie documents auth reads and writes, field for field and in model
//! order (Beanie writes optional fields as explicit nulls; so do these).

use mongodb::bson::{DateTime, Document, oid::ObjectId};
use serde::{Deserialize, Serialize};

pub const USERS: &str = "users";
pub const SESSIONS: &str = "sessions";
pub const SITE_USERS: &str = "site_users";
pub const SITE_SESSIONS: &str = "site_sessions";
pub const API_TOKENS: &str = "api_tokens";

fn yes() -> bool {
    true
}

fn epoch() -> DateTime {
    DateTime::from_millis(0)
}

/// `app.auth.models.User` (dev portal account).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub hashed_password: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default = "yes")]
    pub is_active: bool,
    #[serde(default)]
    pub is_verified: bool,
    #[serde(default)]
    pub is_superuser: bool,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub token_version: i64,
    #[serde(default)]
    pub github_id: Option<i64>,
    #[serde(default)]
    pub email_bounced: bool,
    #[serde(default = "epoch")]
    pub created_at: DateTime,
    #[serde(default = "epoch")]
    pub updated_at: DateTime,
    #[serde(default)]
    pub last_login_at: Option<DateTime>,
}

/// `app.tokens.models.ApiToken`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiToken {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub user_id: ObjectId,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub hashed_token: String,
    #[serde(default)]
    pub scopes: i64,
    #[serde(default)]
    pub ip_salt: Option<String>,
    #[serde(default)]
    pub allowed_ip_hashes: Vec<String>,
    #[serde(default)]
    pub revoked: bool,
    #[serde(default)]
    pub revoked_at: Option<DateTime>,
    #[serde(default)]
    pub revoke_reason: Option<String>,
    #[serde(default)]
    pub expires_at: Option<DateTime>,
    #[serde(default)]
    pub expiry_warned: bool,
    #[serde(default = "epoch")]
    pub created_at: DateTime,
    #[serde(default)]
    pub last_used_at: Option<DateTime>,
    #[serde(default)]
    pub rotated_at: Option<DateTime>,
    #[serde(default)]
    pub request_count: i64,
}

/// `app.site_auth.models.SiteUser` (Discord-only website account).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteUser {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub discord_handle: String,
    #[serde(default)]
    pub notify_email: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default = "yes")]
    pub is_active: bool,
    #[serde(default)]
    pub is_verified: bool,
    #[serde(default)]
    pub is_deleted: bool,
    #[serde(default)]
    pub discord_id: Option<i64>,
    #[serde(default)]
    pub discord_avatar: Option<String>,
    #[serde(default)]
    pub token_version: i64,
    #[serde(default)]
    pub claimed_trove_name: Option<String>,
    #[serde(default)]
    pub claimed_trove_display: Option<String>,
    #[serde(default)]
    pub claimed_at: Option<DateTime>,
    #[serde(default)]
    pub claim_verified: bool,
    #[serde(default)]
    pub claim_baseline: Document,
    #[serde(default)]
    pub claim_verified_at: Option<DateTime>,
    #[serde(default)]
    pub creator_token_hash: Option<String>,
    #[serde(default)]
    pub creator_token_prefix: Option<String>,
    #[serde(default)]
    pub creator_token_at: Option<DateTime>,
    #[serde(default)]
    pub mod_issues_seen_at: Option<DateTime>,
    #[serde(default = "epoch")]
    pub created_at: DateTime,
    #[serde(default = "epoch")]
    pub updated_at: DateTime,
    #[serde(default)]
    pub last_login_at: Option<DateTime>,
}

/// `Session` (`user_id`) and `SiteSession` (`site_user_id`) rows; read-only, `sessions.rs` writes them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRow {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    #[serde(alias = "user_id", alias = "site_user_id")]
    pub owner_id: ObjectId,
    #[serde(default)]
    pub refresh_token_hash: String,
    #[serde(default)]
    pub device: Option<String>,
    #[serde(default = "epoch")]
    pub created_at: DateTime,
    #[serde(default = "epoch")]
    pub last_used_at: DateTime,
    #[serde(default = "epoch")]
    pub expires_at: DateTime,
    #[serde(default)]
    pub revoked: bool,
}
