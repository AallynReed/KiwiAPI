//! `app/auth/router.py`: signup, login, profile, email verification, email
//! change and password reset.

use std::sync::OnceLock;

use axum::{
    extract::{Request, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};
use kiwi_core::email_validator::{self, Email};
use mongodb::bson::{doc, oid::ObjectId};
use serde_json::{Value, json};

use super::{
    checks::{captcha_failed, ensure_not_breached, is_disposable, verify_captcha},
    emails::{self, EMAIL_CHANGE_PURPOSE, RESET_PURPOSE, VERIFY_PURPOSE},
    req::{NoParams, with_body, without_body},
    store::{self, insert_user, revoke_all_sessions, save_user, user_by_email, user_by_id, user_public},
};
use crate::{
    auth::{
        CurrentUser, Rejection,
        client::{client_ip, device_label, user_agent},
        jwt, lockout,
        models::{SESSIONS, User},
        passwords::{hash_password_blocking, verify_password_blocking},
        sessions::{Audience, issue_tokens},
        tokens::password_fingerprint,
    },
    dt,
    pyjson::PydanticJson,
    ratelimit::{check_rate_limit, get_rate_limit},
    state::AppState,
    validation::{Fail, Field, FromBody, FromParams, Input, Model, Params},
};

pub const EMAIL_SPAM_NOTICE: &str =
    "Don't see it? Check your spam folder and mark it 'Not spam' so future emails reach your inbox.";

pub fn password_min_length() -> usize {
    static MIN: OnceLock<usize> = OnceLock::new();
    *MIN.get_or_init(|| {
        crate::config::Settings::from_env().map(|s| usize::try_from(s.password_min_length).unwrap_or(0)).unwrap_or(8)
    })
}

/// An `EmailStr` field: the normalized address.
pub fn email_field(f: Field<'_>) -> String {
    f.custom(|input| {
        let s = match input {
            Input::Json(Value::String(s)) => s.as_str(),
            Input::Str(s) => s.as_ref(),
            _ => return Err(Fail::string_type()),
        };
        match email_validator::validate(s) {
            Email::Valid(e) => Ok(e),
            Email::Invalid(reason) => {
                Err(Fail::new("value_error", format!("value is not a valid email address: {reason}"))
                    .with_ctx("reason", reason))
            }
        }
    })
    .required()
}

fn password_field(m: &mut Model<'_>, name: &str) -> String {
    m.field(name).str().min_len(password_min_length()).max_len(256).required()
}

fn message(text: String) -> Response {
    PydanticJson(json!({ "message": text })).into_response()
}

fn reply(r: Result<Response, Rejection>) -> Response {
    r.unwrap_or_else(IntoResponse::into_response)
}

fn bad_request(msg: &str) -> Rejection {
    Rejection::new(StatusCode::BAD_REQUEST, "bad_request", msg)
}

fn ip_of(parts: &axum::http::request::Parts) -> String {
    client_ip(parts).unwrap_or_else(|| "unknown".into())
}

/// argon2-cffi `PasswordHasher.verify`: a hash it cannot even route to Argon2
/// (non-ASCII, unknown header) raises past `verify_password`, a 500.
pub async fn verify_password(password: String, hashed: &str) -> Result<bool, Rejection> {
    let routable =
        hashed.is_ascii() && matches!(hashed.as_bytes().get(..9), Some(b"$argon2i$" | b"$argon2d$" | b"$argon2id"));
    if !routable {
        return Err(Rejection::internal("password hash is not an argon2 hash"));
    }
    Ok(verify_password_blocking(password, hashed.to_owned()).await)
}

/// `PydanticObjectId(sub)`: an invalid id is an unhandled error.
pub fn oid(sub: &str) -> Result<ObjectId, Rejection> {
    ObjectId::parse_str(sub).map_err(|_| Rejection::internal(format!("invalid ObjectId {sub:?}")))
}

/// `_html_page(title, body, status_code)`.
pub fn html_page(title: &str, body: &str, status: StatusCode) -> Response {
    let title = html_escape(title);
    let page = format!(
        "<!doctype html><html><head><meta charset='utf-8'><title>{title}</title>\
<style>body{{font-family:system-ui,sans-serif;background:#0d1117;color:#e6edf3;\
display:flex;min-height:100vh;align-items:center;justify-content:center;margin:0}}\
.card{{max-width:420px;padding:32px;border:1px solid #30363d;border-radius:12px;\
background:#161b22}}h1{{font-size:1.3rem;margin:0 0 12px}}a,button{{color:#58a6ff}}\
input,button{{font-size:1rem;padding:10px;width:100%;box-sizing:border-box;margin-top:8px;\
border-radius:8px;border:1px solid #30363d;background:#0d1117;color:#e6edf3}}\
button{{cursor:pointer;background:#238636;color:#fff;border:0;margin-top:16px}}\
</style></head><body><div class='card'><h1>{title}</h1>{body}</div></body></html>"
    );
    (status, Html(page)).into_response()
}

/// `html.escape(s, quote=True)`.
pub fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            c => out.push(c),
        }
    }
    out
}

pub fn reset_form_body(token: &str) -> String {
    let safe = html_escape(token);
    format!(
        r#"
        <input id="pw" type="password" placeholder="New password (min 8 chars)" autocomplete="new-password">
        <input id="rtoken" type="hidden" value="{safe}">
        <button onclick="submitReset()">Reset password</button>
        <p id="msg" style="margin-top:12px"></p>
        <script>
        const token = document.getElementById('rtoken').value;
        async function submitReset() {{
          const pw = document.getElementById('pw').value;
          const msg = document.getElementById('msg');
          const r = await fetch('/auth/reset-password', {{
            method: 'POST', headers: {{'Content-Type': 'application/json'}},
            body: JSON.stringify({{token: token, new_password: pw}})
          }});
          const data = await r.json().catch(() => ({{}}));
          msg.textContent = r.ok ? (data.message || 'Password reset.')
            : ((data.error && data.error.message) || 'Reset failed.');
        }}
        </script>
        "#
    )
}

// --- request bodies ------------------------------------------------------------------

pub struct Signup {
    pub email: String,
    pub password: String,
    pub display_name: Option<String>,
    pub captcha_token: Option<String>,
}

impl FromBody for Signup {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(Signup {
                email: email_field(m.field("email")),
                password: password_field(m, "password"),
                display_name: m.field("display_name").str().max_len(80).optional(),
                captcha_token: m.field("captcha_token").str().optional(),
            })
        })
        .required()
    }
}

pub struct Login {
    pub email: String,
    pub password: String,
    pub captcha_token: Option<String>,
}

impl FromBody for Login {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(Login {
                email: email_field(m.field("email")),
                password: m.field("password").str().required(),
                captcha_token: m.field("captcha_token").str().optional(),
            })
        })
        .required()
    }
}

/// `ForgotPasswordRequest` / `ResendVerificationRequest`.
pub struct EmailBody {
    pub email: String,
    pub captcha_token: Option<String>,
}

pub struct Forgot(pub EmailBody);
pub struct Resend(pub EmailBody);

impl FromBody for Forgot {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(Forgot(EmailBody {
                email: email_field(m.field("email")),
                captcha_token: m.field("captcha_token").str().optional(),
            }))
        })
        .required()
    }
}

impl FromBody for Resend {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| Some(Resend(EmailBody { email: email_field(m.field("email")), captcha_token: None }))).required()
    }
}

pub struct ResetPassword {
    pub token: String,
    pub new_password: String,
}

impl FromBody for ResetPassword {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(ResetPassword {
                token: m.field("token").str().required(),
                new_password: password_field(m, "new_password"),
            })
        })
        .required()
    }
}

pub struct ChangePassword {
    pub current_password: String,
    pub new_password: String,
}

impl FromBody for ChangePassword {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(ChangePassword {
                current_password: m.field("current_password").str().required(),
                new_password: password_field(m, "new_password"),
            })
        })
        .required()
    }
}

pub struct ChangeEmail {
    pub new_email: String,
    pub password: String,
}

impl FromBody for ChangeEmail {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(ChangeEmail {
                new_email: email_field(m.field("new_email")),
                password: m.field("password").str().required(),
            })
        })
        .required()
    }
}

pub struct UpdateProfile {
    pub display_name: Option<String>,
}

impl FromBody for UpdateProfile {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| Some(UpdateProfile { display_name: m.field("display_name").str().max_len(80).optional() }))
            .required()
    }
}

/// `token: str` (a required query parameter).
pub struct TokenQuery(pub String);

impl FromParams for TokenQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        TokenQuery(p.query("token").str().required())
    }
}

// --- routes ---------------------------------------------------------------------------

pub async fn signup(State(state): State<AppState>, req: Request) -> Response {
    let (_, _, body, parts) = match with_body::<(), NoParams, Signup>(&state, req).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    reply(signup_run(&state, &parts, body).await)
}

async fn signup_run(state: &AppState, parts: &axum::http::request::Parts, body: Signup) -> Result<Response, Rejection> {
    let s = state.settings();
    let ip = ip_of(parts);
    let (max, window) = get_rate_limit(state, "signup_rate_limit").await;
    check_rate_limit(state, &format!("signup:{ip}"), max, window).await?;
    if !verify_captcha(s, body.captcha_token.as_deref(), Some(&ip)).await? {
        return Err(captcha_failed());
    }
    let email = body.email.to_lowercase();
    if s.disposable_email_check && is_disposable(&email) {
        return Err(Rejection::new(
            StatusCode::BAD_REQUEST,
            "disposable_email",
            "Disposable email addresses aren't allowed",
        ));
    }
    if user_by_email(state, &email).await?.is_some() {
        return Err(Rejection::new(StatusCode::CONFLICT, "email_taken", "An account with this email already exists"));
    }
    ensure_not_breached(s, &body.password).await?;
    let hashed_password = hash_password_blocking(body.password).await;
    let created = dt::now();
    let updated = dt::now();
    let user = User {
        id: ObjectId::new(),
        email,
        hashed_password,
        display_name: body.display_name,
        is_active: true,
        is_verified: false,
        is_superuser: false,
        roles: Vec::new(),
        token_version: 0,
        github_id: None,
        email_bounced: false,
        created_at: dt::to_bson(created),
        updated_at: dt::to_bson(updated),
        last_login_at: None,
    };
    insert_user(state, &user).await?;
    let public = user_public(&user, created);
    emails::background(state, move |st| Some(emails::verification_email(st.settings(), &user)));
    Ok((StatusCode::CREATED, PydanticJson(public)).into_response())
}

pub async fn login(State(state): State<AppState>, req: Request) -> Response {
    let (_, _, body, parts) = match with_body::<(), NoParams, Login>(&state, req).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    reply(login_run(&state, &parts, body).await)
}

async fn login_run(state: &AppState, parts: &axum::http::request::Parts, body: Login) -> Result<Response, Rejection> {
    let s = state.settings();
    let ip = ip_of(parts);
    let email = body.email.to_lowercase();
    let (max, window) = get_rate_limit(state, "login_rate_limit").await;
    check_rate_limit(state, &format!("login:{ip}"), max, window).await?;
    if !verify_captcha(s, body.captcha_token.as_deref(), Some(&ip)).await? {
        return Err(captcha_failed());
    }
    let locked = lockout::lock_ttl(state, &email).await?;
    if locked > 0 {
        return Err(lockout::locked(locked));
    }
    let user = user_by_email(state, &email).await?;
    let valid = match &user {
        Some(u) => verify_password(body.password, &u.hashed_password).await?,
        None => false,
    };
    let mut user = match user.filter(|u| valid && u.is_active) {
        Some(u) => u,
        None => {
            lockout::record_failure(state, &email).await?;
            return Err(Rejection::new(StatusCode::UNAUTHORIZED, "invalid_credentials", "Incorrect email or password"));
        }
    };
    lockout::clear(state, &email).await?;
    if s.require_verified_for_login && !user.is_verified {
        return Err(Rejection::forbidden("email_unverified", "Verify your email address before logging in."));
    }
    let device = device_label(user_agent(parts).as_deref());
    let seen_device = match &device {
        Some(d) => store::coll(state, SESSIONS)?
            .find_one(doc! { "user_id": user.id, "device": d })
            .await
            .map_err(Rejection::internal)?
            .is_some(),
        None => false,
    };
    user.last_login_at = Some(dt::to_bson(dt::now()));
    save_user(state, &user).await?;
    let tokens = issue_tokens(state, Audience::Portal, user.id, user.token_version, device.clone()).await?;
    if s.security_email_notifications && !seen_device {
        emails::background(state, move |st| {
            Some(emails::new_login_email(st.settings(), &user, Some(&ip), device.as_deref()))
        });
    }
    Ok(PydanticJson(serde_json::to_value(tokens).map_err(Rejection::internal)?).into_response())
}

pub async fn me(CurrentUser(user): CurrentUser) -> Response {
    PydanticJson(user_public(&user, dt::from_bson(user.created_at))).into_response()
}

pub async fn update_profile(State(state): State<AppState>, req: Request) -> Response {
    let (CurrentUser(mut user), _, body, _) = match with_body::<CurrentUser, NoParams, UpdateProfile>(&state, req).await
    {
        Ok(v) => v,
        Err(r) => return r,
    };
    let run = async {
        if let Some(name) = body.display_name {
            let name = kiwi_core::pystr::strip(&name);
            user.display_name = (!name.is_empty()).then(|| name.to_owned());
            user.updated_at = dt::to_bson(dt::now());
            save_user(&state, &user).await?;
        }
        Ok(PydanticJson(user_public(&user, dt::from_bson(user.created_at))).into_response())
    };
    reply(run.await)
}

/// The user a purpose token names: `None` for any decode failure or a
/// missing `sub` (the caught `PyJWTError`/`KeyError`); a malformed id is a 500.
async fn token_user(
    state: &AppState,
    token: &str,
    purpose: &str,
) -> Result<(Option<User>, Option<jwt::Claims>), Rejection> {
    let Ok(claims) = jwt::decode_email_token(token, &crate::auth::auth_settings().secret_key, purpose) else {
        return Ok((None, None));
    };
    let Some(sub) = claims.get("sub").and_then(Value::as_str) else {
        return Ok((None, Some(claims)));
    };
    let user = user_by_id(state, oid(sub)?).await?;
    Ok((user, Some(claims)))
}

pub async fn verify_email(State(state): State<AppState>, req: Request) -> Response {
    let ((), TokenQuery(token), _) = match without_body::<(), TokenQuery>(&state, req).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let run = async {
        let Some(mut user) = token_user(&state, &token, VERIFY_PURPOSE).await?.0 else {
            return Ok(html_page(
                "Verification failed",
                "<p>This link is invalid or has expired. Request a new one from the portal.</p>",
                StatusCode::BAD_REQUEST,
            ));
        };
        if !user.is_verified {
            user.is_verified = true;
            user.updated_at = dt::to_bson(dt::now());
            save_user(&state, &user).await?;
        }
        Ok(html_page(
            "Email verified \u{2713}",
            "<p>Your email is confirmed. You can now create API tokens.</p>",
            StatusCode::OK,
        ))
    };
    reply(run.await)
}

pub async fn resend_verification(State(state): State<AppState>, req: Request) -> Response {
    let (_, _, Resend(body), parts) = match with_body::<(), NoParams, Resend>(&state, req).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let run = async {
        check_rate_limit(&state, &format!("resend:{}", ip_of(&parts)), 5, 3600).await?;
        let email = body.email.to_lowercase();
        if let Some(user) = user_by_email(&state, &email).await?
            && user.is_active
            && !user.is_verified
        {
            emails::background(&state, move |st| Some(emails::verification_email(st.settings(), &user)));
        }
        Ok(message(format!("If that account exists and isn't verified, a new link is on its way. {EMAIL_SPAM_NOTICE}")))
    };
    reply(run.await)
}

pub async fn change_password(State(state): State<AppState>, req: Request) -> Response {
    let (CurrentUser(mut user), _, body, parts) =
        match with_body::<CurrentUser, NoParams, ChangePassword>(&state, req).await {
            Ok(v) => v,
            Err(r) => return r,
        };
    let run = async {
        let s = state.settings();
        if !verify_password(body.current_password, &user.hashed_password).await? {
            return Err(Rejection::new(
                StatusCode::UNAUTHORIZED,
                "invalid_credentials",
                "Current password is incorrect",
            ));
        }
        ensure_not_breached(s, &body.new_password).await?;
        user.hashed_password = hash_password_blocking(body.new_password).await;
        revoke_all_sessions(&state, &mut user).await?;
        let device = device_label(user_agent(&parts).as_deref());
        let tokens = issue_tokens(&state, Audience::Portal, user.id, user.token_version, device).await?;
        if s.security_email_notifications {
            emails::background(&state, move |st| Some(emails::password_changed_email(st.settings(), &user)));
        }
        Ok(PydanticJson(serde_json::to_value(tokens).map_err(Rejection::internal)?).into_response())
    };
    reply(run.await)
}

pub async fn change_email(State(state): State<AppState>, req: Request) -> Response {
    let (CurrentUser(user), _, body, _) = match with_body::<CurrentUser, NoParams, ChangeEmail>(&state, req).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let run = async {
        if !verify_password(body.password, &user.hashed_password).await? {
            return Err(Rejection::new(StatusCode::UNAUTHORIZED, "invalid_credentials", "Password is incorrect"));
        }
        let new_email = body.new_email.to_lowercase();
        if new_email == user.email {
            return Err(bad_request("That is already your email address"));
        }
        if user_by_email(&state, &new_email).await?.is_some() {
            return Err(Rejection::new(StatusCode::CONFLICT, "email_taken", "That email is already in use"));
        }
        let text = format!("A confirmation link was sent to {new_email}. {EMAIL_SPAM_NOTICE}");
        emails::background(&state, move |st| Some(emails::email_change_verification(st.settings(), &user, &new_email)));
        Ok(message(text))
    };
    reply(run.await)
}

pub async fn verify_email_change(State(state): State<AppState>, req: Request) -> Response {
    let ((), TokenQuery(token), _) = match without_body::<(), TokenQuery>(&state, req).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let run = async {
        let (user, claims) = token_user(&state, &token, EMAIL_CHANGE_PURPOSE).await?;
        let claims = claims.unwrap_or_default();
        let user = user.filter(|u| claims.get("fp") == Some(&Value::String(password_fingerprint(&u.email))));
        let Some(mut user) = user else {
            return Ok(html_page(
                "Link expired",
                "<p>This confirmation link is invalid or has expired.</p>",
                StatusCode::BAD_REQUEST,
            ));
        };
        let new_email = match claims.get("new") {
            None => String::new(),
            Some(Value::String(s)) => s.to_lowercase(),
            Some(_) => return Err(Rejection::internal("email change token: `new` is not a string")),
        };
        let existing = if new_email.is_empty() { None } else { user_by_email(&state, &new_email).await? };
        if new_email.is_empty() || existing.is_some_and(|e| e.id != user.id) {
            return Ok(html_page(
                "Email unavailable",
                "<p>That email address is no longer available.</p>",
                StatusCode::CONFLICT,
            ));
        }
        let old_email = std::mem::replace(&mut user.email, new_email.clone());
        user.is_verified = true;
        user.email_bounced = false;
        revoke_all_sessions(&state, &mut user).await?;
        if state.settings().security_email_notifications {
            emails::background(&state, move |st| {
                Some(emails::email_changed_notice_to_old(st.settings(), &old_email, &new_email))
            });
        }
        Ok(html_page(
            "Email updated \u{2713}",
            "<p>Your email address has been changed and verified. Please log in again.</p>",
            StatusCode::OK,
        ))
    };
    reply(run.await)
}

pub async fn forgot_password(State(state): State<AppState>, req: Request) -> Response {
    let (_, _, Forgot(body), parts) = match with_body::<(), NoParams, Forgot>(&state, req).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let run = async {
        let s = state.settings();
        let ip = ip_of(&parts);
        let (max, window) = get_rate_limit(&state, "forgot_password_rate_limit").await;
        check_rate_limit(&state, &format!("forgot:{ip}"), max, window).await?;
        if !verify_captcha(s, body.captcha_token.as_deref(), Some(&ip)).await? {
            return Err(captcha_failed());
        }
        let email = body.email.to_lowercase();
        if let Some(user) = user_by_email(&state, &email).await?
            && user.is_active
        {
            emails::background(&state, move |st| Some(emails::password_reset_email(st.settings(), &user)));
        }
        Ok(message(format!("If that email is registered, a reset link is on its way. {EMAIL_SPAM_NOTICE}")))
    };
    reply(run.await)
}

pub async fn reset_password(State(state): State<AppState>, req: Request) -> Response {
    let (_, _, body, _) = match with_body::<(), NoParams, ResetPassword>(&state, req).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let run = async {
        let invalid = || bad_request("This reset link is invalid or has expired.");
        let (user, claims) = token_user(&state, &body.token, RESET_PURPOSE).await?;
        let claims = claims.ok_or_else(invalid)?;
        if !claims.contains_key("sub") {
            return Err(invalid());
        }
        let fp_ok = |u: &User| claims.get("fp") == Some(&Value::String(password_fingerprint(&u.hashed_password)));
        let Some(mut user) = user.filter(fp_ok) else {
            return Err(invalid());
        };
        ensure_not_breached(state.settings(), &body.new_password).await?;
        user.hashed_password = hash_password_blocking(body.new_password).await;
        revoke_all_sessions(&state, &mut user).await?;
        Ok(message("Your password has been reset. You can now log in.".into()))
    };
    reply(run.await)
}

pub async fn reset_password_form(State(state): State<AppState>, req: Request) -> Response {
    let ((), TokenQuery(token), _) = match without_body::<(), TokenQuery>(&state, req).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    html_page("Set a new password", &reset_form_body(&token), StatusCode::OK)
}
