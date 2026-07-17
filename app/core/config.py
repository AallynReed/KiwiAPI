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
    # The mod tools and the whole Mods Hub write surface (commits, .tmod releases,
    # banner/preview images) accept .tmod files under /v1/mods/, so they get a
    # larger cap. Match the proxy's client_max_body_size (>= 20m) on /v1/mods/.
    mods_max_request_body_bytes: int = 20 * 1024 * 1024  # 20 MB

    # --- Mods Hub (git-like mod sharing; see app/trove/mods_hub) -------------
    mods_hub_enabled: bool = True
    # Content-addressed blob store for the hub (file blobs + compiled .tmod +
    # banner/preview images), reusing the update-archive CAS. Bind-mounted.
    mods_store_dir: str = "data/mods"
    mods_hub_max_file_bytes: int = 20 * 1024 * 1024     # per file in a commit
    mods_image_max_bytes: int = 5 * 1024 * 1024         # per banner/preview image
    mods_hub_max_files_per_commit: int = 500
    # Git: per-project bare repos live under <mods_store_dir>/git; the
    # authenticated smart-HTTP server is mounted at /git/mods/*. A push packfile
    # can be large, so /git/* gets its own generous body cap.
    mods_git_enabled: bool = True
    mods_git_max_body_bytes: int = 100 * 1024 * 1024    # push packfile cap
    # Master-only bot cfg ingests. A full LeaderBot.cfg dump is ~16 MB; the market
    # (GrainusMod.cfg) dump is well under 5 MB. Keep the proxy's client_max_body_size
    # on /v1/leaderboards/insert and /v1/market/insert >= these.
    leaderboards_max_request_body_bytes: int = 20 * 1024 * 1024  # 20 MB
    market_max_request_body_bytes: int = 20 * 1024 * 1024  # 20 MB
    # One screenshot upload; the endpoint also caps per-file size itself.
    ocr_max_request_body_bytes: int = 12 * 1024 * 1024  # 12 MB

    # "Hot" window (days): the warmer pre-warms the latest capture of each of these
    # recent days and the leaderboards page date-picker surfaces this window. (All
    # entries live in one partitioned Postgres table now - this is a warm-cache / UI
    # depth knob, not a storage tier.) Matches the archive threshold so "what's warm"
    # and "what pays standard rate" line up.
    leaderboards_hot_retention_days: int = 3
    # Anchors older than this count as "archive" reads and pay an extra, tighter
    # per-token bucket ON TOP of the standard cap - a caller can trawl the archive
    # cheaply per-read but not in a tight loop. Same 3-day window as the hot window.
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
    # Alt-cluster detection: flag packs of similarly-named accounts sitting
    # at near-identical scores (coordinated "alt army"). A family needs at
    # least this many accounts to be a cluster.
    cheaters_cluster_min_size: int = 3
    # Scores within this relative band count as "near-identical" - the
    # band a family's densest near-score subset must fit inside. 0.02 = 2 %.
    cheaters_cluster_score_band_pct: float = 0.02
    # Max Levenshtein distance for merging near-identical name stems
    # (catches typo'd variants like anana/anan/annna). 0 disables fuzzy
    # merging (exact stems only).
    cheaters_cluster_max_edit_distance: int = 2
    # Family size at which the cluster's size-confidence term saturates to
    # 1.0 (it ramps 0→1 from cluster_min_size up to this). 8+ alts = blatant.
    cheaters_cluster_size_full: int = 8
    # Comma-separated board UUIDs to skip during alt-cluster detection -
    # INDEPENDENT of cheaters_excluded_board_uuids (the per-player blacklist).
    # Empty = scan every board.
    cheaters_cluster_excluded_board_uuids: str = ""

    # ── Co-movement (the PRIMARY, name-agnostic alt/bot signal) ──────────
    # Flags groups of accounts whose hourly score gains land in the same
    # bucket in the same hours, across the captures since the last weekly
    # reset - i.e. they progress in lockstep, regardless of name. Name
    # similarity is only an optional confidence booster on top.
    #
    # Per-board candidate cap: only the top-N by rank get their per-hour
    # series loaded (rings sit near the top). Bounds the multi-anchor load.
    # 0 disables co-movement detection entirely.
    cheaters_comovement_candidate_top_n: int = 400
    # An hour "counts" for an account only if its score rose by at least
    # this much that hour (filters idle/noise; idle-matching is not a signal).
    cheaters_comovement_min_hourly_gain: float = 1.0
    # AND its gain must be in the top (1 - percentile) of that board's gains
    # that hour. Focuses co-movement on the anomalously-high gainers (where
    # rings live) and is board-scale-agnostic, so a crowd all gaining a common
    # rate (a popular event) drops out instead of clustering. 0.90 = top 10%.
    cheaters_comovement_gain_percentile: float = 0.90
    # Two hourly gains within this relative tolerance share a bucket (so
    # "near-same delta" matches). 0.05 = within 5%.
    cheaters_comovement_gain_tolerance_pct: float = 0.05
    # Minimum number of matching hours (same hour, same gain bucket) before
    # a pair/group is flagged as co-moving.
    cheaters_comovement_min_matching_hours: int = 3
    # AND those matches must be at least this FRACTION of the rarer account's
    # active (top-percentile) hours. Stops a few coincidental matches across a
    # long week from chaining the hardcore-grinder crowd into one giant fake
    # "ring" - true alts match ~all their hours, legit pairs match a small
    # fraction. 0.7 = the pair must move together >=70% of the time.
    cheaters_comovement_min_match_ratio: float = 0.7
    # A formed group is kept only if it's TIGHT: at least this fraction of all
    # possible member-pairs are co-moving edges (else it's a loose transitive
    # chain, not a coordinated ring). Loose hangers-on are peeled off the dense
    # core. 1.0 = require a full clique; 0.6 tolerates a few missing edges.
    cheaters_comovement_min_density: float = 0.6
    # Minimum accounts in a co-moving group to flag it.
    cheaters_comovement_min_group_size: int = 2
    # Skip an (hour, gain-bucket) cell shared by more than this many accounts
    # - that's a common-event spike (everyone grinding the new daily), not a
    # ring. Bounds the co-occurrence cost AND sharpens the signal.
    cheaters_comovement_max_cell_accounts: int = 40
    # Co-movement changes slowly; recompute it at most this often (seconds),
    # reusing the cached result across the more-frequent warm cycles.
    cheaters_comovement_recompute_seconds: int = 3600

    # ── Schedule correlation + signal fusion ─────────────────────────────
    # Schedule: accounts active/idle in the SAME hours all week (same login/
    # logout rhythm) - catches alts that grind DIFFERENT content but play
    # together, which co-movement (gain-magnitude) misses. An account needs at
    # least this many active hours to be schedule-clustered (else too little
    # signal). 0 disables the schedule producer.
    cheaters_schedule_min_active_hours: int = 6
    # Two accounts link by schedule when the Jaccard overlap of their active-
    # hour sets is at least this. 0.8 = 80% of their combined active hours
    # coincide. Schedule ALONE is weak (lots of people play evenings), so it
    # only reaches high confidence when fusion corroborates it.
    cheaters_schedule_min_similarity: float = 0.8
    # Fusion: each INDEPENDENT signal that agrees on a group beyond the first
    # adds this much confidence (capped at 0.98). The whole point - a group
    # flagged by co-movement AND schedule AND name is far more certain than one.
    cheaters_fusion_corroboration_bonus: float = 0.06
    # Board-footprint corroboration: a group's members count as sharing a
    # footprint when their board-set Jaccard averages at least this.
    cheaters_footprint_min_jaccard: float = 0.6
    # Per-player WEEKLY uptime check: flag a player active (score rose) in at
    # least this fraction of the captures since the weekly reset. No human plays
    # 85%+ of every hour for days - that's a no-sleep bot. The last-hour velocity
    # check can't see this (each individual hour looks normal). 0 disables it.
    cheaters_weekly_uptime_fraction: float = 0.85

    # Class activity (/class-activity). Counts come from the Effort boards only
    # (4000+i); Paragon (5000+i) is excluded as ambiguous. The "clean" (established)
    # view keeps only players clearing BOTH floors at the window end, excluding
    # brand-new characters and throwaway alts. Power Rank is the 1000+i board.
    # Runtime-tunable; set either to 0 to drop that gate.
    class_activity_power_rank_threshold: int = 25000
    class_activity_effort_threshold: int = 50
    # Third clean-view floor: a player counts toward the clean view only when
    # their score on the XP stats board (uuid 21005) is at least this value at
    # the window end. Runtime-tunable; 0 = off.
    class_activity_xp_threshold: int = 2_000_000

    # Market archive throttle: hide_expired=false on /v1/market/listings opts into
    # the historical tail (listings expire after 7 days in-game). Tight per-token
    # bucket on top of the standard cap.
    market_archive_rate_limit_max: int = 10
    market_archive_rate_limit_window_seconds: int = 60
    # Historical: governed the /unlock_* byte-patcher uploads (routes removed
    # 2026-06). Kept defined so a stale override referencing it doesn't crash
    # settings parsing. Free to delete after one full release.
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

    # Internal (service-to-service) base URL for the API, used by the gateway bot
    # to read Postgres-backed data (activity, and future leaderboard/cheater
    # features) over the compose network - the bot container has Mongo + Redis but
    # no Postgres. Defaults to the compose service name; override in dev if needed.
    internal_api_url: str = "http://api:8000"

    # Mongo connection - inside Docker the host is the compose service name.
    mongo_uri: str = "mongodb://localhost:27016"
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
    # Vestigial for login: app/site_auth/oauth.py requests its scopes directly.
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
    # Codexes are lightweight reference reads: wider budget (× base caps) in their
    # own bucket. Default 5× = 150 req/min/IP anon, 600 req/min/token.
    codexes_rate_limit_multiplier: int = 5
    # Bilibili thumbnail proxy: one feed render fires a burst of <img> loads, so it
    # gets its own widened bucket rather than exhausting the shared feeds budget.
    bilibili_image_rate_limit_multiplier: int = 10
    # Ingest cooldown: per-token, per-endpoint backstop against a bot resubmitting
    # the same dump on a loop. API-token (bot) path only - see require_master_ingest.
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

    # --- Site page-view analytics ---
    # Track showcase-site page loads (views + cookieless unique visitors) for the
    # dev-portal "Site Analytics" admin tab. Raw page-view events auto-expire after
    # the retention window (TTL); set enabled=False to turn tracking off entirely.
    pageview_tracking_enabled: bool = True
    pageview_retention_days: int = 90

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

    # Outbound webhooks (Discord) - the Redis list used as the delivery queue.
    webhooks_queue: str = "kiwi:webhooks:queue"
    # Discord DM subscriptions - the Redis list used as the DM delivery queue.
    dm_subs_queue: str = "kiwi:dmsubs:queue"

    # --- Game-file version archiver (Trion update CDN) ---
    # OFF by default: enabling it triggers a multi-GB first sync against Trion's CDN.
    # Turn on deliberately (per box) once the blob store path/disk is ready.
    trove_update_enabled: bool = False
    trove_update_base_url: str = "http://trove-update.dyn.triongames.com"
    trove_update_prefix: str = "/kiwi-live-client-patch/"  # yields an intentional // when joined
    trove_update_store_dir: str = "data/updates"           # content-addressed blob store (bind-mounted)
    # Dev-only fallback for VFX-preview asset resolution: a local PopcornFX project
    # tree (e.g. the extracted VFX/ folder) used to resolve a .pkfx's textures/meshes
    # by basename when the updates archive isn't populated. UNSET in production - the
    # live game tree (updates archive) is the real source.
    pkfx_dev_vfx_dir: str = ""
    trove_update_probe_seconds: int = 1200                 # per-branch probe cadence (20 min)
    trove_update_concurrency: int = 6                      # parallel file downloads

    # --- Blueprint image rendering (app/trove/render) ---
    trove_render_branch: str = "live-us"       # archive branch blueprints are pulled from (matches codex/updates)
    trove_local_game_dir: str = ""             # dev only: a local Trove install to read blueprints from
    trove_render_cache_ttl: int = 86400        # Redis TTL for rendered PNGs (seconds)

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
    # Auth tier: liveness of the shared account-LOGIN gateway. We POST the real
    # Glyph login route (/auth/v1_2) with throwaway creds and read it reachable
    # ONLY when the reply is Trion-shaped (an X-Trionworlds-* header or a
    # "Signature:" ticket body). A bare "404 page not found" (the auth app not
    # routing - i.e. logins erroring), a 5xx, or a connection failure = down.
    # NOTE: a plain GET <500 check is NOT enough - when logins are down this host
    # answers every path with a 404, which is <500 and falsely read as "Reachable".
    # Game tier (per environment): TCP-connect probe of the glsserver port
    # (6560) on the live / PTS game hosts - captured from pcap; port 6560 is
    # the stable login-to-game entry (world-instance ports like :3701x are
    # ephemeral). Accepted = playable; refused/timeout while auth up =
    # maintenance. Hosts/ports are also runtime-tunable (admin panel).
    trove_status_auth_url: str = "https://auth.trionworlds.com/auth/v1_2"
    trove_status_probe_interval_seconds: int = 60          # probe cadence
    trove_status_timeout_seconds: float = 8.0              # per-probe timeout
    # Forgiveness: a single failed scan can be a transient miss or a local network
    # blip, so don't flip a server to "down" on the first failure. Each probe is
    # retried back-to-back up to N times within the SAME cycle (not the full probe
    # interval between tries) and counts online if ANY attempt succeeds; only N
    # consecutive failures mark it down. Both runtime-tunable (admin panel).
    trove_status_probe_attempts: int = 3                   # tries per probe before "down"
    trove_status_probe_retry_delay_seconds: float = 2.0    # gap between back-to-back retries
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
    # Send a FRESH RANDOM ephemeral opener each deep probe instead of replaying a
    # captured hello. Frida on the live client proved the real opener's 32-byte body
    # is regenerated per connection (a random X25519 pubkey), and a live glsserver
    # holds the socket for any well-formed opener (verified live, EU+US) - so a
    # random opener behaves like a real client and NEVER goes stale on a Trove
    # protocol update. Off = legacy replay of trove_status_{env}_hello_hex (goes
    # stale: the server rejects an old captured opener, which made a live EU read
    # as down). Leave on.
    trove_status_game_random_opener: bool = True
    # Per-ENV deep-probe enable flag. NON-EMPTY = run the deep probe for that env;
    # EMPTY = connect-only (TCP-accept = online). With trove_status_game_random_opener
    # on (the default), the deep probe sends a FRESH RANDOM opener and the hex CONTENT
    # below is NOT sent - the string just needs to be non-empty to enable deep here.
    # (The content is only replayed in legacy mode, random_opener=False.)
    #
    # EU + US are game glsservers (ams-/dal- *-game-* hosts): when up they HOLD the
    # opener's socket open (hold=online, fast-FIN=maintenance), so the deep probe
    # works. PTS is an AUTH gateway (auth-pcpts01), NOT a game glsserver: it DROPS
    # the opener even when UP, so the deep probe can't tell PTS up from down → PTS
    # stays "" = connect-only. Set a value here only if PTS is ever pointed at a
    # real *-game-* glsserver that holds the socket open.
    #
    # The retained hex is a real captured opener (kept as the legacy-replay fallback
    # and as a non-empty enable flag). NOTE: a captured opener goes STALE - Frida on
    # the live client proved the real opener is a per-connection RANDOM ephemeral key,
    # so don't rely on replay; leave random_opener on.
    trove_status_eu_hello_hex: str = (
        "20000000003df232536bcb1518164c4685392572b843d0bcbb71be7c06bb098626e23accfb"
    )
    trove_status_us_hello_hex: str = (
        "20000000003df232536bcb1518164c4685392572b843d0bcbb71be7c06bb098626e23accfb"
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
