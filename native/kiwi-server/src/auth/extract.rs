//! `app/core/dependencies.py` + `app/site_auth/dependencies.py` as extractors: same check order and messages.
//! Return `ctx.rate_limit` as a response part where Python returns a model/dict (only there does it add X-RateLimit-*).

use std::{marker::PhantomData, ops::Deref};

use axum::{
    extract::{FromRef, FromRequestParts, MatchedPath},
    http::{HeaderMap, request::Parts},
};
use mongodb::bson::{DateTime, doc, oid::ObjectId};
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::{
    Rejection,
    client::client_ip,
    jwt::{self, Claims},
    models::{API_TOKENS, ApiToken, SITE_USERS, SiteUser, USERS, User},
    pyutil::{eq_int, latin1, strip},
    scopes,
    sessions::SITE_TOKEN_KIND,
    site::site_credential,
    token_use::record_token_use,
    tokens::{hash_token, ip_allowed, verify_token_checksum},
};
use crate::{
    ratelimit::{RateLimitInfo, check_rate_limit, get_rate_limit},
    state::AppState,
    usage::{UsageSlot, fastapi_template},
};

/// `HTTPBearer(auto_error=False)`: the credentials of an `Authorization:
/// Bearer <x>` header, `None` for any other scheme or an empty value.
pub fn bearer(headers: &HeaderMap) -> Option<String> {
    let value = latin1(headers.get("authorization")?);
    if value.is_empty() {
        return None;
    }
    let (scheme, param) = value.split_once(' ').unwrap_or((&value, ""));
    let cred = strip(param);
    if scheme.is_empty() || cred.is_empty() || !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    Some(cred.to_owned())
}

fn mongo(state: &AppState) -> Result<&mongodb::Database, Rejection> {
    state.inner.db.mongo.as_ref().ok_or_else(|| Rejection::internal("MONGO_URI not configured"))
}

async fn find_by_id<T: DeserializeOwned + Send + Sync>(
    state: &AppState,
    coll: &str,
    id: ObjectId,
) -> Result<Option<T>, Rejection> {
    mongo(state)?.collection::<T>(coll).find_one(doc! { "_id": id }).await.map_err(Rejection::internal)
}

/// Decode a session JWT and pull its `sub`; any failure is the same 401.
fn decode_session(token: &str) -> Result<(Claims, String), Rejection> {
    let invalid = || Rejection::not_authenticated("Invalid or expired session token");
    let claims = jwt::decode(token, &super::auth_settings().secret_key).map_err(|_| invalid())?;
    let sub = claims.get("sub").and_then(Value::as_str).ok_or_else(invalid)?.to_owned();
    Ok((claims, sub))
}

/// `PydanticObjectId(sub)`: a malformed id is an unhandled error (500) in Python.
fn subject_oid(sub: &str) -> Result<ObjectId, Rejection> {
    ObjectId::parse_str(sub).map_err(|_| Rejection::internal(format!("invalid ObjectId in token sub: {sub:?}")))
}

/// Dev-portal `_authenticate`.
pub async fn authenticate_portal(state: &AppState, credential: Option<&str>) -> Result<(User, Claims), Rejection> {
    let token = credential.ok_or_else(|| Rejection::not_authenticated("Authentication required"))?;
    let (claims, sub) = decode_session(token)?;
    if claims.get("kind").and_then(Value::as_str) == Some(SITE_TOKEN_KIND) {
        return Err(Rejection::not_authenticated("Wrong-audience token"));
    }
    let user: Option<User> = find_by_id(state, USERS, subject_oid(&sub)?).await?;
    let user = user
        .filter(|u| u.is_active)
        .ok_or_else(|| Rejection::not_authenticated("Account is inactive or no longer exists"))?;
    if !eq_int(claims.get("ver"), user.token_version) {
        return Err(Rejection::not_authenticated("Session has been ended; please log in again"));
    }
    Ok((user, claims))
}

/// Site `_authenticate`: requires `kind == "site"`, loads from `site_users`.
pub async fn authenticate_site(state: &AppState, credential: Option<&str>) -> Result<(SiteUser, Claims), Rejection> {
    let token = credential.ok_or_else(|| Rejection::not_authenticated("Authentication required"))?;
    let (claims, sub) = decode_session(token)?;
    if claims.get("kind").and_then(Value::as_str) != Some(SITE_TOKEN_KIND) {
        return Err(Rejection::not_authenticated("Wrong-audience token"));
    }
    let user: Option<SiteUser> = find_by_id(state, SITE_USERS, subject_oid(&sub)?).await?;
    let user = user
        .filter(|u| u.is_active)
        .ok_or_else(|| Rejection::not_authenticated("Account is inactive or no longer exists"))?;
    if !eq_int(claims.get("ver"), user.token_version) {
        return Err(Rejection::not_authenticated("Session has been ended; please log in again"));
    }
    Ok((user, claims))
}

/// `_resolve_token`: checksum (no DB hit on a bad one), lookup by sha256,
/// revoked / expired / owner checks.
pub async fn resolve_token(state: &AppState, credential: &str) -> Result<(User, ApiToken), Rejection> {
    let invalid = || Rejection::not_authenticated("Invalid or revoked API token");
    if verify_token_checksum(credential, &super::auth_settings().api_token_prefix) == Some(false) {
        return Err(invalid());
    }
    let token = mongo(state)?
        .collection::<ApiToken>(API_TOKENS)
        .find_one(doc! { "hashed_token": hash_token(credential) })
        .await
        .map_err(Rejection::internal)?
        .filter(|t| !t.revoked)
        .ok_or_else(invalid)?;
    if token.expires_at.is_some_and(|e| e < DateTime::now()) {
        return Err(Rejection::not_authenticated("API token has expired"));
    }
    let user: Option<User> = find_by_id(state, USERS, token.user_id).await?;
    let user = user
        .filter(|u| u.is_active)
        .ok_or_else(|| Rejection::not_authenticated("Account is inactive or no longer exists"))?;
    Ok((user, token))
}

/// `_enforce_token_limits`: usage ids, IP allowlist, per-token limit, token-use.
pub async fn enforce_token_limits(
    state: &AppState,
    parts: &Parts,
    user: &User,
    token: &ApiToken,
    multiplier: i64,
    bucket: Option<&str>,
) -> Result<RateLimitInfo, Rejection> {
    if let Some(slot) = parts.extensions.get::<UsageSlot>() {
        slot.set(user.id, token.id);
    }
    if !token.allowed_ip_hashes.is_empty()
        && !ip_allowed(&client_ip(parts).unwrap_or_default(), token.ip_salt.as_deref(), &token.allowed_ip_hashes)
    {
        return Err(Rejection::forbidden("ip_not_allowed", "Requests from this IP are not allowed for this token"));
    }
    let key = match bucket {
        Some(b) => format!("token:{b}:{}", token.id),
        None => format!("token:{}", token.id),
    };
    let (max, window) = get_rate_limit(state, "api_rate_limit").await;
    let info = check_rate_limit(state, &key, max * multiplier, window).await?;
    record_token_use(state, token.id).await;
    Ok(info)
}

/// `_enforce_anonymous_limit`: the stricter per-IP budget.
pub async fn enforce_anonymous_limit(
    state: &AppState,
    parts: &Parts,
    multiplier: i64,
    bucket: Option<&str>,
) -> Result<RateLimitInfo, Rejection> {
    let ip = client_ip(parts).unwrap_or_else(|| "unknown".into());
    let key = match bucket {
        Some(b) => format!("public:{b}:{ip}"),
        None => format!("public:{ip}"),
    };
    let (max, window) = get_rate_limit(state, "public_anon_rate_limit").await;
    check_rate_limit(state, &key, max * multiplier, window).await
}

/// `get_token_context`.
#[derive(Debug, Clone)]
pub struct TokenContext {
    pub user: User,
    pub token: ApiToken,
    pub rate_limit: RateLimitInfo,
}

pub async fn token_context(state: &AppState, parts: &Parts) -> Result<TokenContext, Rejection> {
    let cred = bearer(&parts.headers).ok_or_else(|| Rejection::not_authenticated("API token required"))?;
    let (user, token) = resolve_token(state, &cred).await?;
    let rate_limit = enforce_token_limits(state, parts, &user, &token, 1, None).await?;
    Ok(TokenContext { user, token, rate_limit })
}

/// `require_scope(scope)` by bit: limits run first, then the scope check.
pub async fn require_scope(state: &AppState, parts: &Parts, bit: i64) -> Result<TokenContext, Rejection> {
    let ctx = token_context(state, parts).await?;
    if !scopes::mask_grants_bit(ctx.token.scopes, bit) {
        let key = scopes::key_for(bit).unwrap_or("?");
        return Err(Rejection::forbidden(
            "insufficient_scope",
            &format!("This token is missing the required scope: {key}"),
        ));
    }
    Ok(ctx)
}

/// Result of a public scope: an authenticated caller or an anonymous one.
#[derive(Debug, Clone)]
pub struct AccessContext {
    pub user: Option<User>,
    pub token: Option<ApiToken>,
    pub ip: Option<String>,
    pub rate_limit: RateLimitInfo,
}

impl AccessContext {
    pub fn authenticated(&self) -> bool {
        self.token.is_some()
    }
}

/// `public_scope(scope, rate_multiplier=, bucket=)`: no token is anonymous;
/// a valid token with the scope gets the per-token budget; a valid token
/// without it is downgraded to anonymous; a broken token is a 401.
pub async fn public_scope(
    state: &AppState,
    parts: &Parts,
    bit: i64,
    multiplier: i64,
    bucket: Option<&str>,
) -> Result<AccessContext, Rejection> {
    let scope_bucket = scopes::key_for(bit).filter(|_| multiplier != 1);
    let bucket = bucket.or(scope_bucket);
    if let Some(cred) = bearer(&parts.headers) {
        let (user, token) = resolve_token(state, &cred).await?;
        if scopes::mask_grants_bit(token.scopes, bit) {
            let rate_limit = enforce_token_limits(state, parts, &user, &token, multiplier, bucket).await?;
            return Ok(AccessContext { user: Some(user), token: Some(token), ip: client_ip(parts), rate_limit });
        }
    }
    let rate_limit = enforce_anonymous_limit(state, parts, multiplier, bucket).await?;
    Ok(AccessContext { user: None, token: None, ip: client_ip(parts), rate_limit })
}

/// `require_master_ingest`.
#[derive(Debug, Clone)]
pub struct IngestAuth {
    pub user: User,
    /// Set on the API-token (bot) path, `None` for a session JWT.
    pub token: Option<ApiToken>,
    pub rate_limit: Option<RateLimitInfo>,
}

pub async fn master_ingest(state: &AppState, parts: &Parts) -> Result<IngestAuth, Rejection> {
    let cred = bearer(&parts.headers).ok_or_else(|| Rejection::not_authenticated("Authentication required"))?;
    let (user, token, rate_limit) = if cred.matches('.').count() == 2 {
        let (user, _) = authenticate_portal(state, Some(&cred)).await?;
        (user, None, None)
    } else {
        let (user, token) = resolve_token(state, &cred).await?;
        let info = enforce_token_limits(state, parts, &user, &token, 1, None).await?;
        (user, Some(token), Some(info))
    };
    if !user.is_superuser {
        return Err(Rejection::forbidden("forbidden", "Master account required for ingest endpoints."));
    }
    if let (Some(token), Some(route)) = (&token, parts.extensions.get::<MatchedPath>()) {
        let route = fastapi_template(route.as_str());
        let (max, window) = get_rate_limit(state, "ingest_cooldown").await;
        check_rate_limit(state, &format!("ingest:{route}:{}", token.id), max, window).await?;
    }
    Ok(IngestAuth { user, token, rate_limit })
}

// ---------------------------------------------------------------------------
// Extractors

macro_rules! extractor {
    ($ty:ty, |$state:ident, $parts:ident| $body:expr) => {
        impl<S> FromRequestParts<S> for $ty
        where
            AppState: FromRef<S>,
            S: Send + Sync,
        {
            type Rejection = Rejection;

            async fn from_request_parts($parts: &mut Parts, s: &S) -> Result<Self, Self::Rejection> {
                let $state = AppState::from_ref(s);
                let $state = &$state;
                $body
            }
        }
    };
}

/// `get_current_user` (dev-portal session JWT).
#[derive(Debug, Clone)]
pub struct CurrentUser(pub User);

extractor!(CurrentUser, |state, parts| {
    let cred = bearer(&parts.headers);
    Ok(CurrentUser(authenticate_portal(state, cred.as_deref()).await?.0))
});

/// `get_auth_context`: the user plus the session id (`sid`) of the token.
#[derive(Debug, Clone)]
pub struct AuthContext {
    pub user: User,
    pub session_id: Option<String>,
}

extractor!(AuthContext, |state, parts| {
    let cred = bearer(&parts.headers);
    let (user, claims) = authenticate_portal(state, cred.as_deref()).await?;
    let session_id = claims.get("sid").and_then(Value::as_str).map(str::to_owned);
    Ok(AuthContext { user, session_id })
});

/// `get_current_superuser`.
#[derive(Debug, Clone)]
pub struct Superuser(pub User);

extractor!(Superuser, |state, parts| {
    let cred = bearer(&parts.headers);
    let (user, _) = authenticate_portal(state, cred.as_deref()).await?;
    if !user.is_superuser {
        return Err(Rejection::forbidden("forbidden", "Administrator privileges required"));
    }
    Ok(Superuser(user))
});

/// `get_token_context`: any valid API token, limits and accounting applied.
#[derive(Debug, Clone)]
pub struct TokenAuth(pub TokenContext);

extractor!(TokenAuth, |state, parts| Ok(TokenAuth(token_context(state, parts).await?)));

/// `require_scope(<scope>)`, e.g. `RequireScope<{ scopes::STATS_READ }>`.
#[derive(Debug, Clone)]
pub struct RequireScope<const BIT: i64>(pub TokenContext);

impl<S, const BIT: i64> FromRequestParts<S> for RequireScope<BIT>
where
    AppState: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = Rejection;

    async fn from_request_parts(parts: &mut Parts, s: &S) -> Result<Self, Self::Rejection> {
        let state = AppState::from_ref(s);
        Ok(RequireScope(require_scope(&state, parts, BIT).await?))
    }
}

/// `public_scope(<scope>)` with the default budget, e.g. `Public<{ scopes::BTT_READ }>`.
#[derive(Debug, Clone)]
pub struct Public<const BIT: i64>(pub AccessContext);

impl<S, const BIT: i64> FromRequestParts<S> for Public<BIT>
where
    AppState: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = Rejection;

    async fn from_request_parts(parts: &mut Parts, s: &S) -> Result<Self, Self::Rejection> {
        let state = AppState::from_ref(s);
        Ok(Public(public_scope(&state, parts, BIT, 1, None).await?))
    }
}

/// A widened public scope (`rate_multiplier` / `bucket`), declared once per
/// surface like Python's module-level `Depends(public_scope(...))`.
pub trait PublicScopeSpec: Send + Sync + 'static {
    const SCOPE: i64;
    const BUCKET: Option<&'static str> = None;
    fn multiplier() -> i64 {
        1
    }
}

#[derive(Debug, Clone)]
pub struct PublicScope<P: PublicScopeSpec> {
    pub ctx: AccessContext,
    _spec: PhantomData<P>,
}

impl<P: PublicScopeSpec> Deref for PublicScope<P> {
    type Target = AccessContext;

    fn deref(&self) -> &AccessContext {
        &self.ctx
    }
}

impl<S, P: PublicScopeSpec> FromRequestParts<S> for PublicScope<P>
where
    AppState: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = Rejection;

    async fn from_request_parts(parts: &mut Parts, s: &S) -> Result<Self, Self::Rejection> {
        let state = AppState::from_ref(s);
        let ctx = public_scope(&state, parts, P::SCOPE, P::multiplier(), P::BUCKET).await?;
        Ok(PublicScope { ctx, _spec: PhantomData })
    }
}

/// `require_master_ingest`.
#[derive(Debug, Clone)]
pub struct MasterIngest(pub IngestAuth);

extractor!(MasterIngest, |state, parts| Ok(MasterIngest(master_ingest(state, parts).await?)));

/// `get_current_site_user` (Bearer, else the origin-pinned session cookie).
#[derive(Debug, Clone)]
pub struct CurrentSiteUser(pub SiteUser);

extractor!(CurrentSiteUser, |state, parts| {
    let cred = site_credential(parts);
    Ok(CurrentSiteUser(authenticate_site(state, cred.as_deref()).await?.0))
});

/// `get_optional_site_user`: any failure is simply anonymous.
#[derive(Debug, Clone)]
pub struct OptionalSiteUser(pub Option<SiteUser>);

extractor!(OptionalSiteUser, |state, parts| {
    let Some(cred) = site_credential(parts) else {
        return Ok(OptionalSiteUser(None));
    };
    Ok(OptionalSiteUser(authenticate_site(state, Some(&cred)).await.ok().map(|(u, _)| u)))
});

/// `get_current_verified_site_user`.
#[derive(Debug, Clone)]
pub struct VerifiedSiteUser(pub SiteUser);

extractor!(VerifiedSiteUser, |state, parts| {
    let cred = site_credential(parts);
    let (user, _) = authenticate_site(state, cred.as_deref()).await?;
    if !user.is_verified {
        return Err(Rejection::forbidden("email_unverified", "Your account isn't verified yet."));
    }
    Ok(VerifiedSiteUser(user))
});
