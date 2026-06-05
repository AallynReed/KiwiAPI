from typing import Literal

from pydantic_settings import BaseSettings, SettingsConfigDict


class Settings(BaseSettings):
    """Application configuration, loaded from environment / .env file."""

    model_config = SettingsConfigDict(
        env_file=".env",
        env_file_encoding="utf-8",
        extra="ignore",
    )

    app_name: str = "Kiwi API"
    debug: bool = False

    # Max request body accepted by the app (defence in depth; the proxy should
    # also cap, e.g. nginx `client_max_body_size 8m`).
    max_request_body_bytes: int = 8 * 1024 * 1024  # 8 MB
    # The mod tools accept/return whole .tmod files, so they get a larger cap. Set
    # the proxy's client_max_body_size to match (>= 20m) on the /v1/mods/ paths.
    mods_max_request_body_bytes: int = 20 * 1024 * 1024  # 20 MB

    # Public URLs (used for docs links / CORS).
    # api_url  -> production data API surface (/v1, /health)
    # dev_url  -> developer portal: login, API-key management, Swagger
    # docs_url -> static documentation site
    api_url: str = "https://api.aallyn.net"
    dev_url: str = "https://dev.aallyn.net"
    docs_url: str = "https://docs.aallyn.net"

    # Mongo connection — inside Docker the host is the compose service name.
    mongo_uri: str = "mongodb://localhost:27017"
    mongo_db: str = "kiwi"

    # Redis — backs rate limiting + short-lived caches (Phase C). If unset, the
    # app runs without it.
    redis_url: str | None = None

    # --- Session auth (JWT issued at login, used to manage account + tokens) ---
    # MUST be overridden in production. Generate one with:
    #   python -c "import secrets; print(secrets.token_urlsafe(48))"
    secret_key: str = "CHANGE_ME_insecure_dev_secret_do_not_use_in_production"
    jwt_algorithm: str = "HS256"
    # Short-lived access token; the portal silently refreshes it via the
    # long-lived refresh token (stored server-side as a Session).
    access_token_expire_minutes: int = 15
    refresh_token_expire_days: int = 30

    # --- GitHub OAuth ("Sign in with GitHub") ---
    github_client_id: str | None = None
    github_client_secret: str | None = None

    @property
    def github_oauth_enabled(self) -> bool:
        return bool(self.github_client_id and self.github_client_secret)

    # --- API tokens (issued by users, used to authenticate API queries) ---
    api_token_prefix: str = "kiwi"
    token_creation_daily_limit: int = 3  # tokens a user can mint per day
    token_inactivity_revoke_days: int = 182  # auto-revoke if unused this long (~6 months)
    # Coalesce token last-used/request-count writes to at most one per interval
    # per token (requires Redis; otherwise every request writes). Seconds.
    token_touch_interval_seconds: int = 60

    # Idempotency-Key (replay safety for write requests; requires Redis).
    idempotency_lock_seconds: int = 60       # how long an in-flight key is held
    idempotency_result_seconds: int = 86400  # how long a completed result is replayable
    # Warn the owner by email this many days before a token expires.
    token_expiry_warning_days: int = 7

    # --- Captcha (signup protection) — provider-agnostic ----------------------
    # hCaptcha and Cloudflare Turnstile share the same verify contract, so the
    # only difference is the provider name (and the keys you supply).
    # If captcha_secret is empty, captcha verification is SKIPPED (dev only).
    captcha_provider: Literal["hcaptcha", "turnstile"] = "turnstile"
    captcha_secret: str | None = None
    captcha_sitekey: str | None = None  # public key, surfaced to the signup form

    # --- Rate limits (fixed window, enforced via Mongo) ---
    # Signup: limit account creation per client IP to curb spam.
    signup_rate_limit_max: int = 5
    signup_rate_limit_window_seconds: int = 3600  # 5 signups / hour / IP
    # Login: limit brute-force attempts per IP.
    login_rate_limit_max: int = 10
    login_rate_limit_window_seconds: int = 300  # 10 attempts / 5 min / IP
    # API queries: per-token throughput cap (protects compute-heavy endpoints).
    api_rate_limit_max: int = 120
    api_rate_limit_window_seconds: int = 60  # 120 requests / minute / token

    # Minimum password length enforced at signup.
    password_min_length: int = 8

    # --- Login & signup security ---
    require_verified_for_login: bool = True
    login_max_attempts: int = 5            # failures before lockout
    login_attempt_window_seconds: int = 900
    login_lockout_seconds: int = 900       # 15 min lockout
    password_breach_check: bool = True     # reject HIBP-breached passwords
    disposable_email_check: bool = True
    security_email_notifications: bool = True  # new-login / password / token emails

    # --- Admin bootstrap ---
    # If both are set, on startup the app creates this account (or promotes an
    # existing one) to superuser. ADMIN_EMAIL / ADMIN_PASSWORD in the environment.
    admin_email: str | None = None
    admin_password: str | None = None

    # --- Usage metrics ---
    # Per-request events are retained this many days, then auto-expired (TTL).
    usage_retention_days: int = 30

    # --- Email (SMTP via the aallyn.net Postfix server) ---
    # If smtp_host is unset, email is DISABLED and links are logged instead.
    # For a local Postfix relay: host=host.docker.internal, port=25, no auth/TLS.
    smtp_host: str | None = None
    smtp_port: int = 25
    smtp_username: str | None = None
    smtp_password: str | None = None
    smtp_starttls: bool = False  # True on submission port 587
    smtp_ssl: bool = False       # True on implicit-TLS port 465
    mail_from: str = "no-reply@aallyn.net"
    mail_from_name: str = "Kiwi"

    email_verification_expire_hours: int = 48
    password_reset_expire_hours: int = 1
    forgot_password_rate_limit_max: int = 5
    forgot_password_rate_limit_window_seconds: int = 3600  # 5 / hour / IP

    # --- Email delivery (durable outbox + retry) ---
    # Mail is queued to an outbox and delivered by a background worker so request
    # latency never depends on SMTP, and transient failures are retried.
    email_worker_poll_seconds: int = 10
    email_max_attempts: int = 5
    email_retry_base_seconds: int = 60  # exponential backoff: base * 2**(attempt-1)
    email_outbox_retention_days: int = 30  # auto-expire delivered/dead records

    # --- GitHub secret scanning (partner webhook) ---
    # When a token is leaked publicly, GitHub posts it here so we can auto-revoke.
    # Signature verification (ECDSA over the raw body) is on by default; disabling
    # it should only ever be for local testing.
    github_secret_scanning_verify: bool = True
    github_secret_scanning_keys_url: str = (
        "https://api.github.com/meta/public_keys/secret_scanning"
    )

    # Block minting API tokens until the account's email is verified.
    require_verified_for_tokens: bool = True

    # --- Trove game data (server-time + news relay) ---
    trove_news_feed_url: str = "https://trovegame.com/feed"
    trove_news_refresh_seconds: int = 1800  # background refresh cadence (30 min)
    trove_news_keep: int = 50               # max cached articles retained

    # --- Relayed feeds (twitch / youtube / bilibili) ---
    # We don't re-fetch from Twitch/YouTube/Bilibili ourselves — the trovesaurus
    # bot already does (with its own credentials) and exposes the results. We relay
    # + cache. Switch to http://host.docker.internal:19501 for the on-box bot.
    trovesaurus_base_url: str = "https://trovesaurus.aallyn.net"
    trove_feeds_refresh_seconds: int = 300  # relay refresh cadence (5 min)

    # --- Trovesaurus events (community/in-game event calendar) ---
    # Fetched from the public Trovesaurus calendar feed; stored so we keep history
    # (events persist after they drop off the upstream feed). Categories are
    # free-form and discovered dynamically (served via a distinct query).
    trove_events_feed_url: str = "https://trovesaurus.com/calendar/feed"
    trove_events_refresh_seconds: int = 900   # background refresh cadence (15 min)
    trove_events_history_days: int = 365      # prune events whose end is older than this

    # --- Game-file version archiver (Trion update CDN) ---
    # OFF by default: enabling it triggers a multi-GB first sync against Trion's CDN.
    # Turn on deliberately (per box) once the blob store path/disk is ready.
    trove_update_enabled: bool = False
    trove_update_base_url: str = "http://trove-update.dyn.triongames.com"
    trove_update_prefix: str = "/kiwi-live-client-patch/"  # yields an intentional // when joined
    trove_update_store_dir: str = "data/updates"           # content-addressed blob store (bind-mounted)
    trove_update_probe_seconds: int = 1200                 # per-branch probe cadence (20 min)
    trove_update_concurrency: int = 6                      # parallel file downloads

    # --- Rate-limit alerting (daily digest email to the admin) ---
    rate_limit_alert_email: str | None = "aallyn@aallyn.net"
    rate_limit_alert_threshold: int = 20  # only email if >= this many 429s in the window
    rate_limit_digest_window_hours: int = 24

    @property
    def email_enabled(self) -> bool:
        return bool(self.smtp_host)

    # Browser origins allowed to call the API directly (docs site + dev portal).
    cors_origins: list[str] = [
        "https://docs.aallyn.net",
        "https://dev.aallyn.net",
        "http://localhost:25468",
        "http://127.0.0.1:25468",
        "http://localhost:15546",
        "http://127.0.0.1:15546",
    ]


settings = Settings()
