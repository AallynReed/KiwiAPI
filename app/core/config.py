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
    # The leaderboards ingest endpoint accepts the bot's raw LeaderBot.cfg upload.
    # At ~20k entries/board a full dump is ~16 MB, so 20 MB leaves modest headroom.
    # Master-only via superuser API token, so this isn't an open spigot. (Keep the
    # proxy's client_max_body_size on /v1/leaderboards/insert >= this.)
    leaderboards_max_request_body_bytes: int = 20 * 1024 * 1024  # 20 MB
    # Same shape as leaderboards: master-only ingest of the bot's GrainusMod.cfg
    # market dump. Capped at 20 MB; in practice a dump is well under 5 MB but
    # we leave headroom for a wider interest list down the road.
    market_max_request_body_bytes: int = 20 * 1024 * 1024  # 20 MB

    # Leaderboards archive throttle. Queries for an anchor older than the
    # threshold (default 90 days) hit the archive collection - those reads are
    # cheap individually but a malicious caller could trawl the whole archive
    # with a tight loop, so apply a much tighter per-token limit on top of the
    # standard one. The standard cap stays in force; this is additive.
    # "Hot" window (days): the cache warmer pre-warms the latest capture of each
    # of these recent days and the leaderboards page surfaces this window in its
    # date picker. (All entries live in one partitioned Postgres table now - this
    # is a warm-cache / UI depth knob, not a storage tier.) Matches the archive
    # threshold below so "what's warm" and "what pays standard rate" line up.
    leaderboards_hot_retention_days: int = 3
    # Anchors older than this count as "archive" reads and pay the extra
    # per-token rate-limit bucket. Same 3-day window as the hot window so an
    # old/cold lookup ALSO pays the archive limit.
    leaderboards_archive_query_threshold_days: int = 3
    leaderboards_archive_rate_limit_max: int = 10
    leaderboards_archive_rate_limit_window_seconds: int = 60

    # Cheater detection (/v1/leaderboards/cheaters). All four numbers are
    # runtime-tunable; defaults below are the seed values.
    #   * z_threshold: Modified Z-score cutoff (Iglewicz & Hoaglin 1993).
    #     3.5 is the standard "strong outlier" line.
    #   * velocity_multiplier: a player's score-gain rate must exceed the
    #     board's peer 95th-percentile rate by this much to flag.
    #   * min_board_size: skip statistical analysis on boards with fewer
    #     than this many entries (sample too small to be meaningful).
    #   * cache_ttl_seconds: TTL of the in-memory result cache. Bot writes
    #     hourly so 1800s (30 min) is at most one capture behind.
    # Bumped from 3.5 to 5.0 with the move to elite-cohort MAD-Z: the
    # cohort is much tighter than the full board, so 3.5 was too loose
    # and produced thousands of false positives on heavy-tailed boards.
    # 5.0 is "strong outlier *within* the top players" territory.
    cheaters_z_threshold: float = 5.0
    cheaters_velocity_multiplier: float = 10.0
    cheaters_min_board_size: int = 20
    cheaters_cache_ttl_seconds: int = 1800
    # Comma-separated board UUIDs to exclude from cheater detection.
    # Empty string = analyse every board. Used for boards where the
    # statistical model has high false-positive rate (e.g. server-tally
    # boards like 1100/21012 where there's only one "player" anyway).
    cheaters_excluded_board_uuids: str = ""
    # Elite-cohort size as a fraction of the board's population. 0.05 =
    # top 5 % (or top 50, whichever is larger). Score-outlier check uses
    # this cohort as the baseline instead of the whole board - Trove
    # leaderboards are heavy-tailed and "above the population median"
    # describes every top-100 player, not just cheaters.
    cheaters_elite_cohort_pct: float = 0.05

    # Class activity (/class-activity). Counts are based on the Effort boards only
    # (4000+i); Paragon (5000+i) is excluded as ambiguous. The "clean" (established)
    # view keeps only players who clear BOTH floors below, snapshot at the window
    # end - a way to exclude brand-new characters and throwaway alts so the trend
    # reflects established players. Power Rank lives on the 1000+i leaderboard,
    # Effort on 4000+i. Both are runtime-tunable and take effect on the next
    # recompute. Set either to 0 to drop that gate.
    class_activity_power_rank_threshold: int = 25000
    class_activity_effort_threshold: int = 50

    # Market archive throttle. Market listings expire after 7 days (in-game),
    # so the "archive surface" here is anyone passing hide_expired=false on
    # /v1/market/listings - they're explicitly opting into the historical tail.
    # Tight per-token bucket; the standard cap stays in force.
    market_archive_rate_limit_max: int = 10
    market_archive_rate_limit_window_seconds: int = 60
    # Historical: the /unlock_* byte-patcher tools accepted a ~100 MB
    # Trove.exe upload here. Routes removed 2026-06 after anti-cheat
    # shipped - this knob is kept defined (rather than ripped out) so a
    # stale deployment config or override file referencing it doesn't
    # crash settings parsing. Free to delete after one full release.
    site_max_request_body_bytes: int = 110 * 1024 * 1024  # 110 MB (unused)

    # Where the BetterTroveTools showcase site (templates + static + assets) lives.
    # Bind-mounted into the api container from `./site` in the project root.
    site_root: str = "site"

    # Public URLs (used for docs links / CORS).
    # api_url  -> production data API surface (/v1, /health)
    # dev_url  -> developer portal: login, API-key management, Swagger
    # docs_url -> static documentation site
    # app_url  -> BetterTroveTools showcase site (the public user-facing pages -
    #             /leaderboards, /updates, /market, /login, /dashboard, etc.)
    api_url: str = "https://api.aallyn.net"
    dev_url: str = "https://dev.aallyn.net"
    docs_url: str = "https://docs.aallyn.net"
    app_url: str = "https://trove.aallyn.net"

    # Mongo connection - inside Docker the host is the compose service name.
    mongo_uri: str = "mongodb://localhost:27017"
    mongo_db: str = "kiwi"

    # Redis - backs rate limiting + short-lived caches (Phase C). If unset, the
    # app runs without it.
    redis_url: str | None = None

    # Postgres - the high-volume LEADERBOARDS domain only (entries/boards/players/
    # activity), partitioned by anchor + bulk-loaded via COPY. Everything else
    # stays in Mongo. Unset = the PG-backed leaderboards features are disabled.
    postgres_dsn: str | None = None
    postgres_pool_min: int = 2
    postgres_pool_max: int = 10

    @property
    def postgres_enabled(self) -> bool:
        return bool(self.postgres_dsn)

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

    # --- Discord OAuth ("Sign in with Discord") + interactions ---
    # client_id IS the Discord Application ID (public). public_key is the Ed25519
    # key (hex) that verifies incoming interaction webhooks. Only the secret is
    # sensitive - keep all three in .env, never in code.
    discord_client_id: str | None = None
    discord_client_secret: str | None = None
    discord_public_key: str | None = None
    # Bot token - NOT needed for OAuth login or the interactions webhook (those
    # use the public key). Only required to REGISTER slash commands, or to run a
    # gateway bot for real-time message/member/presence events. Optional.
    discord_bot_token: str | None = None
    # Reserved login scope/integration settings. The user-facing "Sign in with
    # Discord" (app/site_auth/oauth.py) requests "identify email guilds" directly
    # so the Dashboard can list the user's servers; these remain for future use.
    discord_oauth_scope: str = "identify email guilds"
    # Optional authorize ``integration_type`` ("1" = user-install). Empty/unset =
    # plain login. applications.commands needs "1" to install cleanly (no guild
    # picker). String (not int) so an empty compose value doesn't fail to parse.
    discord_oauth_integration_type: str | None = None
    # "Add to Discord" install link for the home page (separate from login). Set
    # to the exact Install Link from discord.dev, or leave blank to auto-build the
    # Discord-provided link from the client_id (which offers user- AND guild-
    # install when both contexts are enabled in the app's Installation settings).
    discord_install_url: str | None = None

    @property
    def discord_oauth_enabled(self) -> bool:
        return bool(self.discord_client_id and self.discord_client_secret)

    @property
    def discord_install_link(self) -> str | None:
        if self.discord_install_url:
            return self.discord_install_url
        if self.discord_client_id:
            return f"https://discord.com/oauth2/authorize?client_id={self.discord_client_id}"
        return None

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

    # --- Captcha (signup protection) - provider-agnostic ----------------------
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
    # Public (unauthenticated) access to the read-only `rotations` + `feeds`
    # scopes: allowed without a token, but at a stricter per-IP budget than an
    # authenticated token gets. Send a token with the scope for the full limit.
    public_anon_rate_limit_max: int = 30
    public_anon_rate_limit_window_seconds: int = 60  # 30 requests / minute / IP
    # Codexes are lightweight reference reads, so they get a wider budget (this ×
    # the base anon/per-token caps) metered in their own bucket. Default 5×:
    # 150 req/min/IP anonymous, 600 req/min/token authenticated.
    codexes_rate_limit_multiplier: int = 5
    # Bilibili thumbnail proxy: one feed-page render fires a burst of <img> loads,
    # so give the image proxy its own widened bucket (× the base anon/per-token
    # caps) rather than letting it exhaust the shared feeds budget. Default 10×:
    # 300 req/min/IP anonymous.
    bilibili_image_rate_limit_multiplier: int = 10
    # Ingest cooldown: per-token, per-endpoint backstop against a misbehaving
    # bot resubmitting the same dump over and over. Applies ONLY when the
    # caller authenticates via API token (the bot path); a master replaying a
    # captured cfg through the portal "Manual cfg ingest" card uses session-
    # JWT auth and bypasses this cap so back-fills aren't blocked. Each
    # ingest endpoint has its own bucket: a leaderboards push doesn't share
    # a budget with a market push. Returned as 429 with Retry-After.
    ingest_cooldown_max: int = 1
    ingest_cooldown_window_seconds: int = 300  # 1 submit / 5 min / endpoint / token

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
    # News is relayed into the durable `trove_news` archive (never pruned): the
    # feeds endpoint serves the newest few, the misc endpoint pages the full history.
    trove_news_feed_url: str = "https://trovegame.com/feed"
    trove_news_refresh_seconds: int = 1800  # background refresh cadence (30 min)

    # --- Community feeds (twitch / youtube / bilibili) ---
    # Fetched at source by app/trove/feeds.py (NOT relayed from the trovesaurus
    # bot any more): Twitch via a client-credentials app token + Helix, YouTube
    # via the Data API, Bilibili via an HTML scrape. Credentials come from the
    # environment with the SAME names the bot used, so values copy across 1:1.
    # The per-feed filter knobs (search query, excluded channels, cutoff, limits)
    # are runtime_config tunables (category "community_feeds"), not settings here.
    twitch_client_id: str | None = None      # env TWITCH_CLIENT_ID
    twitch_client_secret: str | None = None  # env TWITCH_CLIENT_SECRET
    yt_api_key: str | None = None            # env YT_API_KEY (YouTube Data API)
    twitch_game_id: str = "412756"           # Trove's Twitch category id (stable)
    trove_feeds_refresh_seconds: int = 300   # background fetch cadence (5 min)

    # --- BetterTroveTools releases relay (drives in-app update checks) ---
    # Polls the GitHub releases of the configured repo and stores them so the API
    # can serve "latest version per platform" without re-hitting GitHub each call.
    # GitHub allows 60 unauthenticated req/hr; 30-min cadence stays well below it.
    # Set BTT_RELEASES_TOKEN (a GitHub PAT) to lift the cap to 5000/hr if needed.
    btt_releases_repo: str = "AallynReed/BetterTroveTools"
    btt_releases_refresh_seconds: int = 1800
    btt_releases_token: str | None = None

    # --- Delve rotations (weekly community delve data, relayed from an external source) ---
    # The current week's floor data accumulates as players submit; a background task
    # refreshes it (on startup, hourly on the delve-Monday, then once daily at the
    # Trove reset) and stores one document per week. History is imported once (see
    # app/trove/delve_import.py). Set the source URL in the environment - the
    # refresher stays OFF until it's configured (the endpoints still serve imported
    # data either way).
    trove_delve_source_url: str = ""      # week-based delve-history source; set via env
    trove_delve_source_referer: str = ""  # optional Referer the source expects (sent with ?week=)

    # --- Chaos Chest (weekly featured-item rotation) ---
    # The featured item is relayed from Trovesaurus + cached; the weekly window is
    # also computed from server time as a deterministic fallback (served under the
    # rotations scope). Refreshed in the background so requests never hit upstream.
    trove_chaos_chest_url: str = "https://trovesaurus.com/api/chaos-chest"
    trove_chaos_refresh_seconds: int = 1800  # 30 min (the chest rotates weekly)

    # --- Trovesaurus events (community/in-game event calendar) ---
    # Fetched from the public Trovesaurus calendar feed; stored so we keep history
    # (events persist after they drop off the upstream feed). Categories are
    # free-form and discovered dynamically (served via a distinct query).
    trove_events_feed_url: str = "https://trovesaurus.com/calendar/feed"
    trove_events_refresh_seconds: int = 900   # background refresh cadence (15 min)
    trove_events_history_days: int = 365      # prune events whose end is older than this

    # --- Live event stream (SSE: GET /v1/events/stream) ---
    # Push model so consumers stop polling challenge/chaos-chest at the top of the
    # hour. Events fan out across uvicorn workers via Redis pub/sub on this channel
    # (exactly-once per change via a SET-with-GET dedup guard).
    events_channel: str = "kiwi:events"
    events_heartbeat_seconds: int = 20        # SSE keep-alive comment cadence
    events_watch_seconds: int = 30            # safety-net poll that re-publishes on change
    events_max_connections: int = 1000        # per-worker cap on concurrent stream clients

    # --- Game-file version archiver (Trion update CDN) ---
    # OFF by default: enabling it triggers a multi-GB first sync against Trion's CDN.
    # Turn on deliberately (per box) once the blob store path/disk is ready.
    trove_update_enabled: bool = False
    trove_update_base_url: str = "http://trove-update.dyn.triongames.com"
    trove_update_prefix: str = "/kiwi-live-client-patch/"  # yields an intentional // when joined
    trove_update_store_dir: str = "data/updates"           # content-addressed blob store (bind-mounted)
    trove_update_probe_seconds: int = 1200                 # per-branch probe cadence (20 min)
    trove_update_concurrency: int = 6                      # parallel file downloads

    # --- Ingest backlog (server-side replay store) ---
    # Every leaderboard dump the API receives is gzip-saved here keyed by anchor
    # (``<anchor>.cfg.gz``), so the whole history can be RE-INGESTED from the admin
    # panel with no browser upload (server reads from disk + paces itself). Files
    # dropped in manually on the host (``<unix>.cfg`` or ``.cfg.gz``) are picked up
    # too. Bind-mounted (host ``./.backlog``). ``retention_days = 0`` keeps it all
    # (each hourly dump is ~4 MB gzipped - set a limit if disk is tight).
    backlog_enabled: bool = True
    # ABSOLUTE so it always matches the bind-mount target (host ./.backlog ->
    # container /data/backlog) even if the BACKLOG_DIR env didn't get applied -
    # a relative "data/backlog" would resolve to /app/data/backlog (ephemeral,
    # NOT the mount) and silently show an empty backlog.
    backlog_dir: str = "/data/backlog"
    backlog_retention_days: int = 0

    # --- Trove server status prober ---
    # Auth tier: HTTPS liveness of the shared account-auth gateway
    # (auth.trionworlds.com). A structured HTTP response (<500) with valid
    # TLS = reachable; timeout / refused / TLS error / 5xx = down. Catches
    # full outages but stays up during world-only maintenance (Akamai).
    # Game tier (per environment): TCP-connect probe of the glsserver port
    # (6560) on the live / PTS game hosts - captured from pcap; port 6560 is
    # the stable login-to-game entry (world-instance ports like :3701x are
    # ephemeral). Accepted = playable; refused/timeout while auth up =
    # maintenance. Hosts/ports are also runtime-tunable (admin panel).
    trove_status_auth_url: str = "https://auth.trionworlds.com/auth"
    trove_status_probe_interval_seconds: int = 60          # probe cadence
    trove_status_timeout_seconds: float = 8.0              # per-probe timeout
    # Two public Live regions + PTS. The probe target is the REGIONAL
    # glsserver (login-to-game) box on :6560, captured from per-region
    # pcaps - EU = Amsterdam (ams-*), US = Dallas (dal-*), both shaped
    # {dc}-cXX-bYY.{dc}.triongames.com. NOTE: the trove-pc-live-*-game-N
    # .trovegame.com hosts are WORLD shards and do NOT listen on 6560 -
    # probing one is a permanent false "maintenance" (the bug this fixes).
    # These dal-/ams- boxes may be pool-assigned (adjacent IPs seen:
    # EU 51.77.91.79/.80, US 51.79.8.229), so all host/port pairs are
    # runtime-tunable from the admin panel - retarget without a redeploy
    # if a box rotates out.
    trove_status_eu_host: str = "ams-c12-b05.ams.triongames.com"
    trove_status_eu_port: int = 6560
    trove_status_us_host: str = "dal-c35-b05.dal.triongames.com"
    trove_status_us_port: int = 6560
    trove_status_pts_host: str = "auth-pcpts01.trovegame.com"
    trove_status_pts_port: int = 6560
    # Deep game probe. A region in maintenance STILL completes the TCP handshake
    # on 6560 (and even answers the glsserver hello) before dropping the
    # connection - observed directly in EU's maintenance traffic - so a
    # connect-only check reports a false "online". When on, the probe replays the
    # captured glsserver client hello and counts the region online ONLY if the
    # server HOLDS the connection open (a playable session socket) instead of
    # closing it right after the hello. Falls back to the connect-only verdict on
    # any anomaly, so it never does worse than before.
    trove_status_game_deep_probe: bool = True
    # Captured glsserver client "hello" (hex), replayed by the deep probe, PER
    # ENVIRONMENT. Empty = connect-only for that env (TCP-accept = online).
    #
    # EU + US are game glsservers (ams-/dal- *-game-* hosts): when up they HOLD a
    # hello-only probe's socket open, so the deep probe works (hold=online,
    # fast-FIN=maintenance). The same captured hello works for both (it's a
    # per-client-run ECDH opener, portable across gateways - EU and US sent the
    # byte-identical hello in the captures).
    #
    # PTS is an AUTH gateway (auth-pcpts01), NOT a game glsserver: even when UP it
    # DROPS a hello-only probe (it expects the client to keep going, which a probe
    # doesn't), so the deep probe can't tell PTS up from down. Hence PTS = "" =
    # connect-only. Set a hello here only if PTS is ever pointed at a real
    # *-game-* glsserver that holds the socket open.
    trove_status_eu_hello_hex: str = (
        "20000000003df232536bcb1518164c4685392572b843d0bcbb71be7f0abb098626e23accfb"
    )
    trove_status_us_hello_hex: str = (
        "20000000003df232536bcb1518164c4685392572b843d0bcbb71be7f0abb098626e23accfb"
    )
    trove_status_pts_hello_hex: str = ""
    # Seconds the server must hold the socket open after the hello to count as
    # online. Measured gap is huge: a region in maintenance FINs ~20-90ms after
    # the hello (EU capture), while a live gateway holds the socket open for
    # seconds waiting for the client to continue (US/PTS captures held 6-28s). So
    # ~1.5s sits comfortably between the two - long enough to see the maintenance
    # FIN, short enough to never reach a live server's idle-close.
    trove_status_game_hold_seconds: float = 1.5

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
    # Regex for additional allowed browser origins: any aallyn.net subdomain (the
    # Better Trove Tools hosted web build calls /v1/feeds, /v1/rotations, etc.
    # directly from the browser) plus local dev servers on any port. The Android
    # build uses native HTTP (no CORS), so it doesn't rely on this.
    cors_origin_regex: str = (
        r"https://([a-z0-9-]+\.)?aallyn\.net|http://(localhost|127\.0\.0\.1)(:\d+)?"
    )


settings = Settings()
