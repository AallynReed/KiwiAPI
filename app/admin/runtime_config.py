"""Master-flippable runtime tunables (no env edit or restart needed).

REGISTRY (this file) declares the known keys + defaults; the RuntimeConfig
Mongo collection holds sparse overrides. ``get_setting`` returns the override
if present, else the registry default, cached per-key for a short TTL so hot
paths (e.g. the feedback rate limiter) skip a Mongo round-trip. ``set_setting``
validates type + range BEFORE persisting, so the DB only ever holds valid
values, and the cache preserves strict types (int stays int, never a JSON str).
"""

from __future__ import annotations

import json
import time
from dataclasses import dataclass
from datetime import datetime
from typing import Any, Literal

from beanie import Document, PydanticObjectId
from pydantic import Field
from pymongo import ASCENDING, IndexModel

from app.core.config import settings
from app.core.utils import utcnow

# ─── Document ────────────────────────────────────────────────────────────


class RuntimeConfig(Document):
    """One sparse override for a registered tunable. Absent → code default.

    ``value`` is JSON-encoded (encode/decode confined to set_setting/get_setting
    + cache) so one collection holds strings, ints, bools without per-type columns."""

    key: str  # registry key, e.g. "feedback.discord_webhook"
    value: str
    updated_at: datetime = Field(default_factory=utcnow)
    updated_by_user_id: PydanticObjectId | None = None

    class Settings:
        name = "runtime_config"
        indexes = [IndexModel([("key", ASCENDING)], unique=True)]


# ─── Registry ────────────────────────────────────────────────────────────


SettingType = Literal["str", "int", "bool", "float"]


@dataclass(frozen=True)
class TunableSetting:
    """One known tunable. Static metadata - values live in Mongo."""

    key: str
    default: Any
    type: SettingType
    category: str
    description: str
    secret: bool = False         # mask in UI by default (e.g. webhook URLs)
    min_value: int | float | None = None
    max_value: int | float | None = None
    # Optional allowed-values list for enum-like strings (None = any value
    # accepted within type).
    choices: tuple[str, ...] | None = None


def _t(**kw: Any) -> TunableSetting:
    """Tiny helper so the REGISTRY block below stays readable."""
    return TunableSetting(**kw)


# Every known tunable. ORDER preserved in dict iteration → admin UI shows
# them in this order, grouped by category. Add new entries at the bottom
# of their category block.
REGISTRY: dict[str, TunableSetting] = {
    # ── Feature toggles (hide a whole site feature, master-flippable) ─
    # Turn a feature OFF and it vanishes: the navbar entry is hidden, its
    # pages 404, and its API/site endpoints 404 - no code change or restart.
    # Existing stored data is untouched and reappears when toggled back on.
    "feature_mods_hub_enabled": _t(
        key="feature_mods_hub_enabled",
        default=settings.mods_hub_enabled,
        type="bool",
        category="features",
        description=(
            "Master switch for the Mods Hub. OFF hides the /mods pages + navbar "
            "link and 404s every Mods-Hub endpoint (/v1/mods/hub/*, /site/mods/*, "
            "the git server /git/mods/*) and the dashboard's My Mods tab. Stored "
            "mods/repos are kept and return when toggled back ON."
        ),
    ),
    "feature_market_enabled": _t(
        key="feature_market_enabled",
        default=True,
        type="bool",
        category="features",
        description=(
            "Master switch for the Market. OFF hides the /market page + navbar "
            "link and 404s the market endpoints (/v1/market/*, /site/market/*), "
            "including the bot's ingest - re-enable before the next dump if you "
            "want uninterrupted collection. Stored listings are kept."
        ),
    ),
    "feature_leaderboards_enabled": _t(
        key="feature_leaderboards_enabled",
        default=True,
        type="bool",
        category="features",
        description=(
            "Master switch for the Leaderboards. OFF hides the /leaderboards page "
            "+ navbar link and 404s every leaderboards endpoint (/v1/leaderboards/*, "
            "the /site/leaderboards/* board proxies, and the /player/<name> profile "
            "pages). The Player/Class Activity pages have their own toggles and are "
            "unaffected. Stored captures are kept and return when toggled back ON."
        ),
    ),
    "feature_player_activity_enabled": _t(
        key="feature_player_activity_enabled",
        default=True,
        type="bool",
        category="features",
        description=(
            "Master switch for the Player Activity page. OFF hides the /activity "
            "page + navbar link (and its OG card) and 404s /v1/activity/* plus the "
            "/site/leaderboards/activity* proxies. The underlying activity estimate "
            "still computes in the warmer (the leaderboards hero pulse uses it); "
            "this only hides the dedicated page + API."
        ),
    ),
    "feature_class_activity_enabled": _t(
        key="feature_class_activity_enabled",
        default=True,
        type="bool",
        category="features",
        description=(
            "Master switch for the Class Activity page. OFF hides the "
            "/class-activity page + navbar link and 404s /v1/class-activity/* plus "
            "the /site/leaderboards/class-activity/* proxies. Stored data is kept."
        ),
    ),
    "feature_clubs_enabled": _t(
        key="feature_clubs_enabled",
        default=True,
        type="bool",
        category="features",
        description=(
            "Master switch for the public Clubs directory. OFF hides the /clubs page "
            "+ navbar link (404). Clubs are a website-only page sourced from the "
            "Discord dashboard, so there is no /v1 API to gate. Stored club configs "
            "are untouched."
        ),
    ),
    "feature_updates_enabled": _t(
        key="feature_updates_enabled",
        default=True,
        type="bool",
        category="features",
        description=(
            "Master switch for the Updates archive browser. OFF hides the /updates "
            "page + navbar link and 404s /v1/updates/* plus the /site/updates/* "
            "proxies. The background CDN archiver keeps mirroring regardless; this "
            "only hides the read surface. Stored versions are kept."
        ),
    ),
    "feature_codexes_enabled": _t(
        key="feature_codexes_enabled",
        default=True,
        type="bool",
        category="features",
        description=(
            "Master switch for the Codexes browser. OFF hides the /codexes page + "
            "navbar link and 404s /v1/codexes/* plus the /site/codexes/* proxies "
            "(including blueprint renders). Stored codex data is kept."
        ),
    ),
    "feature_server_status_enabled": _t(
        key="feature_server_status_enabled",
        default=True,
        type="bool",
        category="features",
        description=(
            "Master switch for the Trove server-status feature. OFF hides the "
            "/status page + navbar link (and its OG card) and 404s the status "
            "endpoints (/v1/misc/trove-status[/history] + /site/trove-status*). The "
            "background prober keeps probing; this only hides the read surface."
        ),
    ),
    "feature_giveaways_enabled": _t(
        key="feature_giveaways_enabled",
        default=True,
        type="bool",
        category="features",
        description=(
            "Master switch for Giveaways. OFF hides the /giveaways page + navbar "
            "link and 404s the public giveaways endpoints (/v1/giveaways/*, "
            "/site/giveaways). The admin management endpoints stay reachable so you "
            "can still administer draws while the public surface is hidden."
        ),
    ),
    "feature_commands_enabled": _t(
        key="feature_commands_enabled",
        default=True,
        type="bool",
        category="features",
        description=(
            "Master switch for the Trove Commands reference. OFF hides the /commands "
            "page + navbar link (404). It's a static client-rendered page (data in "
            "site/static/commands.json), so there is no /v1 API to gate."
        ),
    ),
    "feature_server_time_enabled": _t(
        key="feature_server_time_enabled",
        default=True,
        type="bool",
        category="features",
        description=(
            "Master switch for the Server Time page. OFF hides the /server-time "
            "page + navbar link and 404s its same-origin /site/server-time proxy. "
            "It's a client-rendered world-clock + Discord-timestamp page reusing the "
            "public rotations time, so the shared /v1/rotations/server-time endpoint "
            "(also used by the landing page) is NOT gated by this."
        ),
    ),
    "feature_webhooks_enabled": _t(
        key="feature_webhooks_enabled",
        default=True,
        type="bool",
        category="features",
        description=(
            "Master switch for outbound (Discord) webhooks. OFF hides the Webhooks "
            "section in the User Dashboard, 404s its CRUD endpoints (/v1/webhooks/*), "
            "and stops enqueuing event deliveries (challenge / mod_release / "
            "game_update). Stored webhooks are untouched and resume when toggled ON."
        ),
    ),
    "feature_dm_subscriptions_enabled": _t(
        key="feature_dm_subscriptions_enabled",
        default=True,
        type="bool",
        category="features",
        description=(
            "Master switch for Discord DM subscriptions. OFF hides the DM Alerts "
            "section in the User Dashboard, 404s its CRUD endpoints "
            "(/v1/dm-subscriptions/*), and stops delivering DM alerts (challenge / "
            "corruxion / fluxion / game_update / market watchlist). Stored "
            "subscriptions are untouched and resume when toggled ON."
        ),
    ),
    "feature_image_studio_enabled": _t(
        key="feature_image_studio_enabled",
        default=True,
        type="bool",
        category="features",
        description=(
            "Master switch for the Image Studio (user-designed images for embeds + "
            "standalone). OFF hides the Dashboard section and 404s its endpoints "
            "(/v1/images/*) and the public render URL (/site/images/*.png). Stored "
            "designs are untouched and resume when toggled ON."
        ),
    ),
    "feature_calendar_enabled": _t(
        key="feature_calendar_enabled",
        default=True,
        type="bool",
        category="features",
        description=(
            "Master switch for the Calendar page. OFF hides the /calendar page + "
            "navbar link and 404s its same-origin /site/calendar/* proxy. It's a "
            "client-rendered live-rotations + event board reusing the public "
            "rotations compute, so the shared /site/rotations proxy (also used by "
            "the homepage) is NOT gated by this."
        ),
    ),
    "feature_streams_enabled": _t(
        key="feature_streams_enabled",
        default=True,
        type="bool",
        category="features",
        description=(
            "Master switch for the Streams page. OFF hides the /streams page + navbar "
            "link. It's a client-rendered aggregator of the shared community feeds "
            "(/site/feeds/videos + /site/feeds/news, also used by the dashboard), so "
            "those proxies are NOT gated by this - only the dedicated page is hidden."
        ),
    ),
    "feature_btt_releases_enabled": _t(
        key="feature_btt_releases_enabled",
        default=True,
        type="bool",
        category="features",
        description=(
            "Master switch for the App Releases page. OFF hides the /releases page + "
            "navbar link and 404s its same-origin /site/btt/* proxies. The desktop "
            "app's own update checks hit the public /v1/btt/* API (driven by scopes, "
            "not this flag), so update prompts keep working while the page is hidden."
        ),
    ),
    "feature_classes_enabled": _t(
        key="feature_classes_enabled",
        default=True,
        type="bool",
        category="features",
        description=(
            "Master switch for the Classes reference page. OFF hides the /classes "
            "page + navbar link and 404s its same-origin /site/stats/classes proxy. "
            "The public /v1/stats/classes API (driven by scopes) is NOT gated by this "
            "- only the dedicated reference page is hidden. It's static game data."
        ),
    ),
    "feature_star_chart_enabled": _t(
        key="feature_star_chart_enabled",
        default=True,
        type="bool",
        category="features",
        description=(
            "Master switch for the Star Chart planner page. OFF hides the /star-chart "
            "page + navbar link. It's a client-rendered interactive builder that reads "
            "the static /static/star_chart.json straight from the asset mount (no "
            "same-origin proxy or /v1 API), so only the dedicated page is hidden."
        ),
    ),
    "feature_gem_simulator_enabled": _t(
        key="feature_gem_simulator_enabled",
        default=True,
        type="bool",
        category="features",
        description=(
            "Master switch for the Gem Simulator page. OFF hides the /gem-simulator "
            "page + navbar link. It's a client-rendered gem roller/augmenter (static "
            "/static/gem-engine.js, a JS port of the gem model) with state kept in the "
            "browser's localStorage (no same-origin proxy or /v1 API), so only the "
            "dedicated page is hidden."
        ),
    ),
    "feature_gem_evaluator_enabled": _t(
        key="feature_gem_evaluator_enabled",
        default=True,
        type="bool",
        category="features",
        description=(
            "Master switch for the Gem Evaluator page. OFF hides the /gem-evaluator "
            "page + navbar link and 404s its same-origin proxies (/site/gems/evaluate, "
            "/site/gems/stat-range, /site/gems/lookups). The page scores a typed-in gem "
            "(quality %, Power Rank, focus-material plan) via the gems:read service layer."
        ),
    ),
    "feature_gem_builds_enabled": _t(
        key="feature_gem_builds_enabled",
        default=True,
        type="bool",
        category="features",
        description=(
            "Master switch for the Gem Builds optimizer page. OFF hides the /gem-builds "
            "page + navbar link and 404s its same-origin proxies (/site/gems/builds/*). "
            "The page ranks the top gem proc layouts by damage coefficient for a "
            "class/subclass/food/ally config via the gems:read service layer."
        ),
    ),
    "feature_calculators_enabled": _t(
        key="feature_calculators_enabled",
        default=True,
        type="bool",
        category="features",
        description=(
            "Master switch for the Calculators page (Power Rank, Mastery, Magic Find, "
            "Light). OFF hides the /calculators page + navbar link. The tabs are "
            "client-rendered from static stat tables; the Magic Find tab's optional "
            "star-chart preview uses the /site/gems/parse-star-chart proxy."
        ),
    ),
    "feature_delves_enabled": _t(
        key="feature_delves_enabled",
        default=False,
        type="bool",
        category="features",
        description=(
            "Master switch for the weekly Delve rotation data (relayed from an "
            "external community source). OFF (the default) 404s the delve endpoints "
            "(/v1/rotations/delves + /v1/rotations/delves/weeks) and pauses the "
            "background refresher so no delve data is fetched. Stored rotations are "
            "kept and the refresher resumes when toggled back ON (needs "
            "TROVE_DELVE_SOURCE_URL set). Does not affect the Depth-15 'longshade' "
            "biome rotation, which is computed locally and unrelated to this source."
        ),
    ),
    "feature_store_enabled": _t(
        key="feature_store_enabled",
        default=True,
        type="bool",
        category="features",
        description=(
            "Master switch for the in-game Store catalog. OFF 404s every store "
            "endpoint (/v1/store/*), including the bot's ingest - re-enable "
            "before the next dump if you want uninterrupted collection. Stored "
            "products/categories are kept and return when toggled back ON."
        ),
    ),

    # ── Cheater / alt-cluster calculation (independent compute switches) ───
    # Distinct from the cheater_detection TUNING knobs below: these turn the
    # (expensive) detection compute ON/OFF in the leaderboards warmer. Each gates
    # its own half - cheater detection the per-player checks, alt-clusters the
    # cluster pass - and either can run alone; a disabled half hides only its own
    # tab. Only when BOTH are OFF does the warmer skip the work entirely.
    "feature_cheater_detection_enabled": _t(
        key="feature_cheater_detection_enabled",
        default=True,
        type="bool",
        category="features",
        description=(
            "Switch for the Possible-cheaters analysis (the per-player score-"
            "outlier + rank-gap + velocity + weekly-uptime checks). OFF stops the "
            "leaderboards warmer from running those checks each cycle, makes the "
            "cheaters payload return empty players, and hides the Possible-cheaters "
            "tab on the /leaderboards page. Independent of feature_alt_clusters_"
            "enabled - either can run alone; only when BOTH are OFF does the warmer "
            "skip the compute entirely (an empty 'disabled' payload). Tuning knobs "
            "in the 'Cheater detection' category are ignored while this is OFF."
        ),
    ),
    "feature_alt_clusters_enabled": _t(
        key="feature_alt_clusters_enabled",
        default=True,
        type="bool",
        category="features",
        description=(
            "Switch for ALT-CLUSTER detection (name-stem + co-movement + schedule "
            "fusion - the O(n²), multi-capture history pass, the heaviest part of "
            "the compute). OFF skips the cluster pass: the payload returns "
            "clusters=[] and the Alt-clusters tab is hidden. Independent of "
            "feature_cheater_detection_enabled - it can run with cheater detection "
            "OFF (clusters-only) or off while cheater detection runs. Only when "
            "BOTH are OFF is the whole compute skipped."
        ),
    ),
    "feature_leaderboard_renames_enabled": _t(
        key="feature_leaderboard_renames_enabled",
        default=True,
        type="bool",
        category="features",
        description=(
            "Master switch for player-rename detection. OFF stops the "
            "leaderboards warmer from running the live rename pass each capture, "
            "404s the rename endpoints (/v1/leaderboards/renames, "
            "/site/leaderboards/renames), and hides the Possible-renames tab on "
            "the /leaderboards page. Already-recorded renames are kept and "
            "reappear when toggled back ON. Tuning knobs live in the 'Player "
            "rename detection' category."
        ),
    ),

    # ── Feedback (/v1/misc/feedback) ─────────────────────────────────
    "feedback.discord_webhook": _t(
        key="feedback.discord_webhook",
        default="",
        type="str",
        category="feedback",
        description=(
            "Discord webhook URL the API POSTs to whenever a new piece of "
            "feedback is submitted. Empty disables the webhook; the feedback "
            "is still saved to the database. Format: "
            "https://discord.com/api/webhooks/<id>/<token>"
        ),
        secret=True,
    ),
    "feedback.per_ip_max": _t(
        key="feedback.per_ip_max",
        default=3,
        type="int",
        category="feedback",
        description=(
            "Maximum feedback submissions accepted from one IP within "
            "feedback.per_ip_window_seconds. Lower = tighter anti-spam, "
            "higher = more lenient. Hit returns 429 with Retry-After."
        ),
        min_value=1,
        max_value=100,
    ),
    "feedback.per_ip_window_seconds": _t(
        key="feedback.per_ip_window_seconds",
        default=3600,
        type="int",
        category="feedback",
        description=(
            "Sliding-window length for the per-IP cap, in seconds. Default "
            "3600 (1 hour). Make it shorter to recover quicker after a "
            "burst from one user; longer to keep them throttled."
        ),
        min_value=60,
        max_value=86400,
    ),
    "feedback.global_max": _t(
        key="feedback.global_max",
        default=60,
        type="int",
        category="feedback",
        description=(
            "Maximum feedback submissions accepted ACROSS ALL IPs within "
            "feedback.global_window_seconds. Backstop against coordinated "
            "many-IP spam waves. Never surfaced to clients (so spammers "
            "can't tune against it); just silently drops requests once hit."
        ),
        min_value=10,
        max_value=10000,
    ),
    "feedback.global_window_seconds": _t(
        key="feedback.global_window_seconds",
        default=3600,
        type="int",
        category="feedback",
        description="Sliding-window length for the global cap, in seconds.",
        min_value=60,
        max_value=86400,
    ),

    # ── API rate limits (per-token + anonymous-per-IP) ────────────────
    # These two pairs gate the bulk of public+authenticated traffic. The
    # keys match the legacy ``settings.*`` names so existing logs / SDK
    # docs stay readable; defaults come straight from settings.py so a
    # fresh deploy with no DB overrides behaves identically to before.
    "api_rate_limit_max": _t(
        key="api_rate_limit_max",
        default=settings.api_rate_limit_max,
        type="int",
        category="api_rate_limits",
        description=(
            "Maximum requests per token per window for the global "
            "per-token bucket. Hits return 429 with X-RateLimit-* headers."
        ),
        min_value=1, max_value=100000,
    ),
    "api_rate_limit_window_seconds": _t(
        key="api_rate_limit_window_seconds",
        default=settings.api_rate_limit_window_seconds,
        type="int",
        category="api_rate_limits",
        description="Sliding-window length for the per-token bucket, in seconds.",
        min_value=10, max_value=86400,
    ),
    "public_anon_rate_limit_max": _t(
        key="public_anon_rate_limit_max",
        default=settings.public_anon_rate_limit_max,
        type="int",
        category="api_rate_limits",
        description=(
            "Per-IP cap for anonymous access to public scopes (e.g. live "
            "rotations / server time without a token). A token carrying "
            "the matching scope earns the wider per-token limit instead."
        ),
        min_value=1, max_value=100000,
    ),
    "public_anon_rate_limit_window_seconds": _t(
        key="public_anon_rate_limit_window_seconds",
        default=settings.public_anon_rate_limit_window_seconds,
        type="int",
        category="api_rate_limits",
        description="Sliding-window length for the per-IP anon bucket, in seconds.",
        min_value=10, max_value=86400,
    ),

    # ── Auth-flow rate limits (per-IP, per-flow) ──────────────────────
    "signup_rate_limit_max": _t(
        key="signup_rate_limit_max",
        default=settings.signup_rate_limit_max,
        type="int",
        category="auth_rate_limits",
        description="Maximum signup attempts per IP per window.",
        min_value=1, max_value=1000,
    ),
    "signup_rate_limit_window_seconds": _t(
        key="signup_rate_limit_window_seconds",
        default=settings.signup_rate_limit_window_seconds,
        type="int",
        category="auth_rate_limits",
        description="Signup throttle window, in seconds.",
        min_value=60, max_value=86400,
    ),
    "login_rate_limit_max": _t(
        key="login_rate_limit_max",
        default=settings.login_rate_limit_max,
        type="int",
        category="auth_rate_limits",
        description=(
            "Maximum login attempts per IP per window. Failed AND "
            "successful both count - this is brute-force defence."
        ),
        min_value=1, max_value=1000,
    ),
    "login_rate_limit_window_seconds": _t(
        key="login_rate_limit_window_seconds",
        default=settings.login_rate_limit_window_seconds,
        type="int",
        category="auth_rate_limits",
        description="Login throttle window, in seconds.",
        min_value=60, max_value=86400,
    ),
    "forgot_password_rate_limit_max": _t(
        key="forgot_password_rate_limit_max",
        default=settings.forgot_password_rate_limit_max,
        type="int",
        category="auth_rate_limits",
        description="Maximum 'forgot password' submissions per IP per window.",
        min_value=1, max_value=1000,
    ),
    "forgot_password_rate_limit_window_seconds": _t(
        key="forgot_password_rate_limit_window_seconds",
        default=settings.forgot_password_rate_limit_window_seconds,
        type="int",
        category="auth_rate_limits",
        description="'Forgot password' throttle window, in seconds.",
        min_value=60, max_value=86400,
    ),

    # ── Archive-query rate limits (extra cap on cold-data trawls) ─────
    "leaderboards_hot_retention_days": _t(
        key="leaderboards_hot_retention_days",
        default=settings.leaderboards_hot_retention_days,
        type="int",
        category="archive_rate_limits",
        description=(
            "How many recent days count as \"hot\": the cache warmer "
            "pre-warms the latest capture of each of these days, and the "
            "leaderboards page surfaces this window in its date picker. "
            "(The data all lives in one partitioned Postgres table now - "
            "this is a warm-cache / UI depth knob, not a storage tier.) "
            "Higher values pre-warm more anchors; lower values warm fewer."
        ),
        min_value=1, max_value=3650,
    ),
    "leaderboards_archive_query_threshold_days": _t(
        key="leaderboards_archive_query_threshold_days",
        default=settings.leaderboards_archive_query_threshold_days,
        type="int",
        category="archive_rate_limits",
        description=(
            "Age (in days) past which a leaderboard query counts as an "
            "ARCHIVE read and pays the extra rate limit below. Conventionally "
            "matches leaderboards_hot_retention_days so the \"is this an "
            "old/cold lookup\" question and the \"do you pay the archive "
            "rate limit\" question have the same answer."
        ),
        min_value=1, max_value=3650,
    ),
    "leaderboards_archive_rate_limit_max": _t(
        key="leaderboards_archive_rate_limit_max",
        default=settings.leaderboards_archive_rate_limit_max,
        type="int",
        category="archive_rate_limits",
        description=(
            "Per-token cap for leaderboard ARCHIVE queries (anchors older "
            "than leaderboards_archive_query_threshold_days). Applied IN "
            "ADDITION to the standard token cap, so a tight value here "
            "doesn't slow down hot reads."
        ),
        min_value=1, max_value=10000,
    ),
    "leaderboards_archive_rate_limit_window_seconds": _t(
        key="leaderboards_archive_rate_limit_window_seconds",
        default=settings.leaderboards_archive_rate_limit_window_seconds,
        type="int",
        category="archive_rate_limits",
        description="Leaderboards-archive throttle window, in seconds.",
        min_value=10, max_value=86400,
    ),
    "market_archive_rate_limit_max": _t(
        key="market_archive_rate_limit_max",
        default=settings.market_archive_rate_limit_max,
        type="int",
        category="archive_rate_limits",
        description=(
            "Per-token cap for market ARCHIVE queries (hide_expired=false). "
            "Same idea as the leaderboards archive cap."
        ),
        min_value=1, max_value=10000,
    ),
    "market_archive_rate_limit_window_seconds": _t(
        key="market_archive_rate_limit_window_seconds",
        default=settings.market_archive_rate_limit_window_seconds,
        type="int",
        category="archive_rate_limits",
        description="Market-archive throttle window, in seconds.",
        min_value=10, max_value=86400,
    ),

    # ── Ingest cooldown (per-token, per-endpoint) ─────────────────────
    # Backstop against a misbehaving bot resubmitting the same dump every
    # few seconds (see ingest_log for the kind of duplicate-anchor spam
    # this catches). Only applies when the caller authenticates via API
    # token (the bot path); session-JWT calls from the portal "Manual cfg
    # ingest" card bypass it so the master can replay back-fills without
    # waiting out the window. Each ingest endpoint is bucketed
    # independently - submitting a leaderboards dump doesn't eat into the
    # market dump's budget.
    "ingest_cooldown_max": _t(
        key="ingest_cooldown_max",
        default=settings.ingest_cooldown_max,
        type="int",
        category="ingest_cooldown",
        description=(
            "Maximum successful ingest submissions per token per endpoint "
            "per window. Default 1 (one submit per window). Returns 429 "
            "with Retry-After once exhausted. Set to a higher integer if "
            "you want the bot to be able to submit multiple captures in a "
            "single window."
        ),
        min_value=1, max_value=1000,
    ),
    "ingest_cooldown_window_seconds": _t(
        key="ingest_cooldown_window_seconds",
        default=settings.ingest_cooldown_window_seconds,
        type="int",
        category="ingest_cooldown",
        description=(
            "Sliding-window length for the per-token per-endpoint ingest "
            "cap, in seconds. Default 300 (5 minutes). Bot scrapes "
            "leaderboards on the hour, so 300 is generous; market scrapes "
            "are roughly every 5–10 min so this matches the natural cadence."
        ),
        min_value=10, max_value=86400,
    ),

    # ── Per-scope multipliers (widen the standard caps for heavy reads) ─
    "codexes_rate_limit_multiplier": _t(
        key="codexes_rate_limit_multiplier",
        default=settings.codexes_rate_limit_multiplier,
        type="int",
        category="scope_multipliers",
        description=(
            "Multiplier applied to BOTH the per-token and per-IP caps for "
            "codexes:read. Codex pages are read-heavy (dozens of GETs per "
            "session); the default 5× lets normal browsing work without "
            "shoving everyone into a wider global default. "
            "⚠ Requires an API container restart to take effect - the "
            "multiplier is bound into the FastAPI dependency tree at startup."
        ),
        min_value=1, max_value=100,
    ),
    "bilibili_image_rate_limit_multiplier": _t(
        key="bilibili_image_rate_limit_multiplier",
        default=settings.bilibili_image_rate_limit_multiplier,
        type="int",
        category="scope_multipliers",
        description=(
            "Multiplier for the bilibili thumbnail proxy. Galleries fan out "
            "into many image requests at once, so a higher value avoids "
            "spuriously throttling normal viewing. "
            "⚠ Requires an API container restart to take effect."
        ),
        min_value=1, max_value=100,
    ),

    # ── Rate-limit alert digest knobs ─────────────────────────────────
    "rate_limit_alert_threshold": _t(
        key="rate_limit_alert_threshold",
        default=settings.rate_limit_alert_threshold,
        type="int",
        category="rate_limit_alerts",
        description=(
            "Minimum number of 429s in the digest window before an alert "
            "email goes out. Avoids notifying for normal occasional hits."
        ),
        min_value=1, max_value=10000,
    ),
    "rate_limit_digest_window_hours": _t(
        key="rate_limit_digest_window_hours",
        default=settings.rate_limit_digest_window_hours,
        type="int",
        category="rate_limit_alerts",
        description=(
            "Lookback window for the rate-limit digest email, in hours. "
            "24 means the daily summary covers the last 24h of 429s."
        ),
        min_value=1, max_value=168,
    ),

    # ── Cheater detection (/v1/leaderboards/cheaters) ────────────────
    "cheaters_z_threshold": _t(
        key="cheaters_z_threshold",
        default=settings.cheaters_z_threshold,
        type="float",
        category="cheater_detection",
        description=(
            "Modified Z-score cutoff (MAD-based, Iglewicz & Hoaglin 1993). "
            "A player whose score exceeds the board's median by more than "
            "this many robust-z-scores is flagged. 3.5 = standard 'strong "
            "outlier' line. Raise to be stricter (fewer false positives); "
            "lower to be more aggressive."
        ),
        min_value=1.0, max_value=20.0,
    ),
    "cheaters_velocity_multiplier": _t(
        key="cheaters_velocity_multiplier",
        default=settings.cheaters_velocity_multiplier,
        type="float",
        category="cheater_detection",
        description=(
            "Velocity check: a player's score-gain rate (Δscore/Δtime) "
            "must exceed the board's peer 95th-percentile rate by at least "
            "this multiplier to flag. 10× is conservative; 5× is moderate."
        ),
        min_value=2.0, max_value=100.0,
    ),
    "cheaters_min_board_size": _t(
        key="cheaters_min_board_size",
        default=settings.cheaters_min_board_size,
        type="int",
        category="cheater_detection",
        description=(
            "Skip boards with fewer than this many entries. Small samples "
            "produce unreliable median/MAD/p95 statistics."
        ),
        min_value=5, max_value=500,
    ),
    "cheaters_cache_ttl_seconds": _t(
        key="cheaters_cache_ttl_seconds",
        default=settings.cheaters_cache_ttl_seconds,
        type="int",
        category="cheater_detection",
        description=(
            "TTL of the in-memory result cache for /v1/leaderboards/cheaters. "
            "Bot writes hourly so 1800s (30 min) is at most one capture "
            "behind. Lower for fresher results at the cost of compute."
        ),
        min_value=60, max_value=86400,
    ),
    "cheaters_excluded_board_uuids": _t(
        key="cheaters_excluded_board_uuids",
        default=settings.cheaters_excluded_board_uuids,
        type="str",
        category="cheater_detection",
        description=(
            "Comma-separated board UUIDs to skip during cheater detection. "
            "Useful for boards where statistical detection is noisy by "
            "design (e.g. server-tally boards like 1100/21012). Whitespace "
            "is ignored. Empty = analyse every board. Example: "
            "'1100, 21012, 5001'."
        ),
    ),
    "cheaters_elite_cohort_pct": _t(
        key="cheaters_elite_cohort_pct",
        default=settings.cheaters_elite_cohort_pct,
        type="float",
        category="cheater_detection",
        description=(
            "Elite-cohort size for the score-outlier check, as a fraction "
            "of the board's population (or top 50, whichever is larger). "
            "0.05 = top 5%. The cohort defines the baseline distribution "
            "against which a player's outlier-ness is measured; for "
            "heavy-tailed leaderboards, measuring against the FULL "
            "population produces false positives because every top player "
            "is mathematically far from the median. Lower = stricter "
            "(smaller cohort, harder to be an outlier within it)."
        ),
        min_value=0.01, max_value=0.5,
    ),
    "cheaters_cluster_min_size": _t(
        key="cheaters_cluster_min_size",
        default=settings.cheaters_cluster_min_size,
        type="int",
        category="cheater_detection",
        description=(
            "Alt-cluster check: minimum number of similarly-named accounts "
            "(shared name stem, e.g. anana1 … anana20) sitting at near-"
            "identical scores before the family is flagged as a coordinated "
            "'alt army'. 3 is sensitive; raise to only surface larger packs."
        ),
        min_value=2, max_value=50,
    ),
    "cheaters_cluster_score_band_pct": _t(
        key="cheaters_cluster_score_band_pct",
        default=settings.cheaters_cluster_score_band_pct,
        type="float",
        category="cheater_detection",
        description=(
            "Alt-cluster check: how close scores must be to count as "
            "'near-identical', as a fraction of the score. 0.02 = within 2%. "
            "The family's densest subset fitting inside this band is the "
            "cluster; the tighter the actual spread, the higher the "
            "confidence. Lower = stricter (only true near-ties cluster)."
        ),
        min_value=0.0001, max_value=0.25,
    ),
    "cheaters_cluster_max_edit_distance": _t(
        key="cheaters_cluster_max_edit_distance",
        default=settings.cheaters_cluster_max_edit_distance,
        type="int",
        category="cheater_detection",
        description=(
            "Alt-cluster check: max Levenshtein edit distance for merging "
            "near-identical name stems into one family, catching typo'd "
            "variants (anana / anan / annna). 0 disables fuzzy merging "
            "(exact stems only). 2 is a good balance; higher risks merging "
            "unrelated short names."
        ),
        min_value=0, max_value=4,
    ),
    "cheaters_cluster_size_full": _t(
        key="cheaters_cluster_size_full",
        default=settings.cheaters_cluster_size_full,
        type="int",
        category="cheater_detection",
        description=(
            "Alt-cluster check: family size at which the size component of "
            "cluster confidence saturates to its max (it ramps from "
            "cheaters_cluster_min_size up to this). 8 means an 8+-account "
            "family gets full credit for size; closeness and board count "
            "still modulate the final confidence."
        ),
        min_value=3, max_value=100,
    ),
    "cheaters_cluster_excluded_board_uuids": _t(
        key="cheaters_cluster_excluded_board_uuids",
        default=settings.cheaters_cluster_excluded_board_uuids,
        type="str",
        category="cheater_detection",
        description=(
            "Comma-separated board UUIDs to skip during ALT-CLUSTER detection "
            "(both the name-stem and co-movement methods). INDEPENDENT of "
            "cheaters_excluded_board_uuids (the per-player blacklist) - a board "
            "can be excluded from one check and not the other. Useful for boards "
            "where many accounts legitimately share a name stem at tied scores "
            "(e.g. capped/server-tally boards). Whitespace ignored. Empty = scan "
            "every board. Example: '1100, 21012, 5001'."
        ),
    ),
    "cheaters_comovement_candidate_top_n": _t(
        key="cheaters_comovement_candidate_top_n",
        default=settings.cheaters_comovement_candidate_top_n,
        type="int",
        category="cheater_detection",
        description=(
            "Co-movement check (the PRIMARY, name-agnostic alt/bot signal): per "
            "board, only the top-N accounts by rank get their per-hour score "
            "series loaded across the week, then grouped by who gains in lockstep. "
            "Rings sit near the top, and this caps the multi-capture load. "
            "Set to 0 to DISABLE co-movement detection entirely."
        ),
        min_value=0, max_value=5000,
    ),
    "cheaters_comovement_min_hourly_gain": _t(
        key="cheaters_comovement_min_hourly_gain",
        default=settings.cheaters_comovement_min_hourly_gain,
        type="float",
        category="cheater_detection",
        description=(
            "Co-movement check: an hour only counts for an account if its score "
            "rose by at least this much that hour. Filters idle/noise hours - two "
            "accounts both sitting idle (gain 0) is not a signal. Raise on "
            "large-score boards to ignore trivial ticks."
        ),
        min_value=0.0, max_value=1e12,
    ),
    "cheaters_comovement_gain_percentile": _t(
        key="cheaters_comovement_gain_percentile",
        default=settings.cheaters_comovement_gain_percentile,
        type="float",
        category="cheater_detection",
        description=(
            "Co-movement check: an account's hour only counts if its gain is in "
            "the TOP (1 - this) of that board's gains that hour. 0.90 = only the "
            "top 10% gainers each hour are considered, so a crowd all gaining a "
            "common rate (a popular event) drops out instead of forming a fake "
            "ring; rings live among the anomalously-high gainers. Board-scale-"
            "agnostic. Raise toward 0.99 to focus on only the very top; lower to "
            "cast a wider (noisier) net. 0 disables the percentile gate."
        ),
        min_value=0.0, max_value=0.999,
    ),
    "cheaters_comovement_gain_tolerance_pct": _t(
        key="cheaters_comovement_gain_tolerance_pct",
        default=settings.cheaters_comovement_gain_tolerance_pct,
        type="float",
        category="cheater_detection",
        description=(
            "Co-movement check: two accounts' hourly gains count as 'the same' "
            "when within this relative tolerance (they land in the same bucket). "
            "0.05 = within 5%. Lower = stricter (only near-exact matches co-move)."
        ),
        min_value=0.0, max_value=0.5,
    ),
    "cheaters_comovement_min_matching_hours": _t(
        key="cheaters_comovement_min_matching_hours",
        default=settings.cheaters_comovement_min_matching_hours,
        type="int",
        category="cheater_detection",
        description=(
            "Co-movement check: minimum number of matching hours (same hour, same "
            "gain bucket) before accounts are flagged as co-moving. 3 = moderate. "
            "Higher = fewer false positives (won't flag everyone grinding one "
            "popular event), slower to catch a fresh ring."
        ),
        min_value=2, max_value=168,
    ),
    "cheaters_comovement_min_match_ratio": _t(
        key="cheaters_comovement_min_match_ratio",
        default=settings.cheaters_comovement_min_match_ratio,
        type="float",
        category="cheater_detection",
        description=(
            "Co-movement check: two accounts co-move only if their matching hours "
            "are at least this FRACTION of the rarer one's active (top-percentile) "
            "hours - not just an absolute count. This is the key guard against a "
            "few coincidental matches over a long week chaining the whole "
            "hardcore-grinder crowd into one giant false 'ring': true alts match "
            "~all their hours, legit pairs match a small fraction. 0.7 = move "
            "together >=70% of the time. Raise toward 1.0 to require near-perfect "
            "lockstep."
        ),
        min_value=0.0, max_value=1.0,
    ),
    "cheaters_comovement_min_density": _t(
        key="cheaters_comovement_min_density",
        default=settings.cheaters_comovement_min_density,
        type="float",
        category="cheater_detection",
        description=(
            "Co-movement check: a formed group is kept only if it's TIGHT - at "
            "least this fraction of all possible member-pairs are co-moving edges. "
            "A loose transitive chain (A-B-C-D linked only by adjacent pairs) has "
            "low density and is rejected; loose members are peeled off the dense "
            "core. 1.0 = require a full clique (every member moves with every "
            "other); 0.6 tolerates a few missing edges in a real ring."
        ),
        min_value=0.0, max_value=1.0,
    ),
    "cheaters_comovement_min_group_size": _t(
        key="cheaters_comovement_min_group_size",
        default=settings.cheaters_comovement_min_group_size,
        type="int",
        category="cheater_detection",
        description=(
            "Co-movement check: minimum number of lockstep accounts to flag the "
            "group. 2 surfaces pairs; raise to only show larger rings."
        ),
        min_value=2, max_value=100,
    ),
    "cheaters_comovement_max_cell_accounts": _t(
        key="cheaters_comovement_max_cell_accounts",
        default=settings.cheaters_comovement_max_cell_accounts,
        type="int",
        category="cheater_detection",
        description=(
            "Co-movement check: ignore an (hour, gain-bucket) cell shared by more "
            "than this many accounts - that's a common-event spike (everyone "
            "grinding the new daily at once), not a coordinated ring. Also bounds "
            "the pairwise-within-cell cost. Lower = stricter + cheaper."
        ),
        min_value=2, max_value=2000,
    ),
    "cheaters_comovement_recompute_seconds": _t(
        key="cheaters_comovement_recompute_seconds",
        default=settings.cheaters_comovement_recompute_seconds,
        type="int",
        category="cheater_detection",
        description=(
            "Co-movement check: recompute the (heavier, multi-capture) co-movement "
            "pass at most this often. The result is cached by weekly-window and "
            "reused across the more-frequent warm cycles, so a 30-min warm doesn't "
            "re-scan the whole week each time. 3600 = once an hour."
        ),
        min_value=300, max_value=86400,
    ),
    "cheaters_schedule_min_active_hours": _t(
        key="cheaters_schedule_min_active_hours",
        default=settings.cheaters_schedule_min_active_hours,
        type="int",
        category="cheater_detection",
        description=(
            "Schedule correlation: an account needs at least this many active "
            "(score-rose) hours this week to be schedule-clustered - below it "
            "there's too little rhythm to compare. Schedule matching catches alts "
            "that grind DIFFERENT content but log in/out together (which the "
            "gain-magnitude co-movement check misses). 0 disables the schedule "
            "producer."
        ),
        min_value=0, max_value=168,
    ),
    "cheaters_schedule_min_similarity": _t(
        key="cheaters_schedule_min_similarity",
        default=settings.cheaters_schedule_min_similarity,
        type="float",
        category="cheater_detection",
        description=(
            "Schedule correlation: two accounts link when the Jaccard overlap of "
            "their active-hour sets is at least this (0.8 = 80% of their combined "
            "active hours coincide). Schedule ALONE is a weak signal (many people "
            "play the same evenings), so a schedule-only group stays low-confidence "
            "unless fusion corroborates it with co-movement / name / footprint."
        ),
        min_value=0.0, max_value=1.0,
    ),
    "cheaters_fusion_corroboration_bonus": _t(
        key="cheaters_fusion_corroboration_bonus",
        default=settings.cheaters_fusion_corroboration_bonus,
        type="float",
        category="cheater_detection",
        description=(
            "Signal fusion: each INDEPENDENT signal that agrees on a group beyond "
            "the first adds this much confidence (capped at 0.98). The core idea - "
            "a group flagged by co-movement AND schedule AND a shared name is far "
            "more certain than one flagged by a single signal. 0 disables the "
            "corroboration bonus (each cluster keeps its single-signal confidence)."
        ),
        min_value=0.0, max_value=0.2,
    ),
    "cheaters_footprint_min_jaccard": _t(
        key="cheaters_footprint_min_jaccard",
        default=settings.cheaters_footprint_min_jaccard,
        type="float",
        category="cheater_detection",
        description=(
            "Signal fusion: a group counts as sharing a board FOOTPRINT (a "
            "corroborating signal) when its members' board-set Jaccard averages at "
            "least this. Alts grind the same content, so they tend to appear on the "
            "same set of boards. 0.6 = their board lists overlap ~60%."
        ),
        min_value=0.0, max_value=1.0,
    ),
    "cheaters_weekly_uptime_fraction": _t(
        key="cheaters_weekly_uptime_fraction",
        default=settings.cheaters_weekly_uptime_fraction,
        type="float",
        category="cheater_detection",
        description=(
            "Per-player WEEKLY check (folded into the Possible-cheaters tab): flag "
            "a player whose score rose in at least this FRACTION of the captures "
            "since the weekly reset. No human plays 85%+ of every hour for days - "
            "that's a no-sleep bot, which the last-hour velocity check can't see "
            "(each hour looks normal on its own). Only applies once enough of the "
            "week has elapsed (≥48 captures). 0 disables the weekly uptime check."
        ),
        min_value=0.0, max_value=1.0,
    ),

    # ── Player rename detection (/v1/leaderboards/renames) ───────────
    "renames_max_gap_seconds": _t(
        key="renames_max_gap_seconds",
        default=settings.renames_max_gap_seconds,
        type="int",
        category="renames",
        description=(
            "Only compare two captures for renames when they are within this "
            "many seconds of each other. Captures are hourly, so 5400s (1.5h) "
            "is 'the adjacent capture plus slack for a delayed one' - never a "
            "multi-hour outage gap, across which score drift + population churn "
            "would corrupt the fingerprint match. Pairs beyond this are skipped."
        ),
        min_value=3600, max_value=86400,
    ),
    "renames_min_boards": _t(
        key="renames_min_boards",
        default=settings.renames_min_boards,
        type="int",
        category="renames",
        description=(
            "A rename is emitted only when the vanished and appeared names match "
            "on at least this many LIFETIME boards (Trove/Geode Mastery, Power "
            "Rank - boards that never reset and carry a score across a rename). "
            "2+ near-identical Mastery-scale scores coinciding by chance between "
            "two accounts is astronomically unlikely - the core false-positive "
            "guard. Raise to be even stricter."
        ),
        min_value=1, max_value=10,
    ),
    "renames_score_drift_pct": _t(
        key="renames_score_drift_pct",
        default=settings.renames_score_drift_pct,
        type="float",
        category="renames",
        description=(
            "Per-board score match tolerance (relative). A renamed player keeps "
            "grinding, so the new name's score may sit slightly ABOVE the old "
            "(never below, on an accumulating board). A board matches when the "
            "rise (score_to - score_from) / score_from is between 0 and this. "
            "0.02 = up to 2% higher. Tighter = only near-exact carries match."
        ),
        min_value=0.0, max_value=0.25,
    ),
    "renames_min_score": _t(
        key="renames_min_score",
        default=settings.renames_min_score,
        type="float",
        category="renames",
        description=(
            "Ignore lifetime-board entries whose score is below this when "
            "fingerprinting. Tiny/round early scores aren't distinctive and "
            "collide across many players. Higher = fingerprint only on "
            "meaningful, high-entropy scores (fewer, more certain matches)."
        ),
        min_value=0.0, max_value=1e9,
    ),
    "renames_min_confidence": _t(
        key="renames_min_confidence",
        default=settings.renames_min_confidence,
        type="float",
        category="renames",
        description=(
            "Minimum blended confidence to persist/emit a rename. Conservative "
            "matching (mutual-exclusive best match on >= renames_min_boards "
            "boards) already keeps survivors high; this is the floor below which "
            "a candidate is dropped rather than guessed. 0.5 is permissive-ish; "
            "raise toward 0.8 to only record near-certain renames."
        ),
        min_value=0.0, max_value=1.0,
    ),
    "renames_excluded_board_uuids": _t(
        key="renames_excluded_board_uuids",
        default=settings.renames_excluded_board_uuids,
        type="str",
        category="renames",
        description=(
            "Comma-separated LIFETIME-board UUIDs to exclude from the rename "
            "fingerprint (e.g. a board where many accounts legitimately tie). "
            "Whitespace ignored. Empty = use every lifetime board. Resetting "
            "(daily/weekly) boards are never used regardless."
        ),
    ),
    "renames_cache_ttl_seconds": _t(
        key="renames_cache_ttl_seconds",
        default=settings.renames_cache_ttl_seconds,
        type="int",
        category="renames",
        description=(
            "Cache-Control max-age on the rename-list endpoints. The record is a "
            "cheap indexed read and only changes when a new capture lands, so this "
            "is just CDN/browser politeness."
        ),
        min_value=0, max_value=86400,
    ),

    # ── Last played (profile activity heuristic) ─────────────────────
    "last_played_excluded_board_uuids": _t(
        key="last_played_excluded_board_uuids",
        default=settings.last_played_excluded_board_uuids,
        type="str",
        category="last_played",
        description=(
            "Comma-separated board UUIDs to EXCLUDE from the /player 'Last "
            "played' estimate. Last-played is the most recent capture where the "
            "player's score ROSE on any non-excluded board - real activity, "
            "unlike 'Last seen' which is just their latest appearance (a player "
            "on a lifetime board like Trove Mastery appears in every capture "
            "forever). Exclude boards whose score moves WITHOUT the player "
            "playing - e.g. the club tally boards (Club Power Rank 1100, Club XP "
            "21012) that rise when other club members play. Whitespace ignored. "
            "Empty = use every board. Example: '1100, 21012'."
        ),
    ),

    # ── Capture completeness guard (reject bad leaderboard dumps) ─────
    "capture_completeness_enabled": _t(
        key="capture_completeness_enabled",
        default=settings.capture_completeness_enabled,
        type="bool",
        category="ingest_quality",
        description=(
            "Reject a live leaderboard capture that arrives materially incomplete "
            "vs the previous good capture (missing whole boards, or a board "
            "truncated part-way) - the in-game scrape can return a short dump when "
            "the leaderboards are briefly down or crash mid-run. A rejected dump is "
            "NOT stored (it's still saved to the backlog + logged for audit); the "
            "hour becomes a clean time-gap that activity / cheater / rename / delta "
            "calculations already bridge over, instead of a poisoned capture that "
            "reads absent players as 'newly active'. OFF stores every dump as-is."
        ),
    ),
    "capture_max_missing_boards": _t(
        key="capture_max_missing_boards",
        default=settings.capture_max_missing_boards,
        type="int",
        category="ingest_quality",
        description=(
            "Reject a capture when MORE than this many boards present in the "
            "previous good capture are absent from the new one. 1 tolerates a "
            "single benign blip (a quiet board momentarily reporting nothing); set "
            "0 to reject on ANY missing board. Detection is by board id/presence, "
            "not entry counts. Ignored at reset boundaries (board rotation is "
            "expected there)."
        ),
        min_value=0, max_value=50,
    ),
    "capture_collapse_frac": _t(
        key="capture_collapse_frac",
        default=settings.capture_collapse_frac,
        type="float",
        category="ingest_quality",
        description=(
            "Also reject when a board present in BOTH captures collapsed below this "
            "fraction of its previous entry count (a board that failed part-way "
            "through the scrape, e.g. 20k -> 8k = 0.4). Only boards with a real "
            "prior population (>= 100 entries) count. 0 disables this collapse "
            "check (presence-only). The one count-based trigger, for gross mid-run "
            "cuts a pure presence check would miss."
        ),
        min_value=0.0, max_value=1.0,
    ),
    "capture_min_prev_boards": _t(
        key="capture_min_prev_boards",
        default=settings.capture_min_prev_boards,
        type="int",
        category="ingest_quality",
        description=(
            "Only run the completeness guard when the previous capture had at least "
            "this many boards - avoids judging completeness on a cold start or "
            "sparse early history (where a small board set is normal, not a "
            "failure)."
        ),
        min_value=1, max_value=200,
    ),

    # ── Class activity (/class-activity) ──────────────────────────────
    "class_activity_power_rank_threshold": _t(
        key="class_activity_power_rank_threshold",
        default=settings.class_activity_power_rank_threshold,
        type="int",
        category="class_activity",
        description=(
            "Power-Rank floor for the Class Activity \"clean\" (established) view "
            "(the page default). A player counts toward a class's clean estimate "
            "only when their Power Rank on that class (the 1000+i leaderboard) is at "
            "least this value - one of two gates (with the Effort floor) that filter "
            "out brand-new characters and throwaway alts. Set to 0 to drop this "
            "gate. The \"All\" toggle on the page is unaffected. Takes effect on the "
            "next class-activity recompute (latest window each capture; full history "
            "on a backfill)."
        ),
        min_value=0, max_value=10_000_000,
    ),
    "class_activity_effort_threshold": _t(
        key="class_activity_effort_threshold",
        default=settings.class_activity_effort_threshold,
        type="int",
        category="class_activity",
        description=(
            "Effort floor for the Class Activity \"clean\" (established) view. A "
            "player counts toward a class's clean estimate only when their Effort on "
            "that class (the 4000+i leaderboard) is at least this value. Combined "
            "with the Power-Rank floor (both must pass). Set to 0 to drop this gate. "
            "Takes effect on the next class-activity recompute."
        ),
        min_value=0, max_value=10_000_000,
    ),
    "class_activity_xp_threshold": _t(
        key="class_activity_xp_threshold",
        default=settings.class_activity_xp_threshold,
        type="int",
        category="class_activity",
        description=(
            "XP floor for the Class Activity \"clean\" (established) view. A "
            "player counts toward a class's clean estimate only when their score "
            "on the XP stats leaderboard (uuid 21005) is at least this value at "
            "the window end - the third gate alongside the Power-Rank and Effort "
            "floors (all must pass). Set to 0 to drop this gate. Takes effect on "
            "the next class-activity recompute (latest window each capture; full "
            "history on a backfill)."
        ),
        min_value=0, max_value=1_000_000_000,
    ),

    # ── Trove server status - per-environment game endpoints ──────────
    # The auth tier (auth.trionworlds.com HTTPS) needs no config. The game
    # tier is a TCP-connect probe of the glsserver port (6560) per
    # environment. Hosts/ports default to the captured trovegame.com
    # hostnames in config.py; override here to track endpoint changes
    # without a redeploy. Empty host / port 0 disables that environment's
    # game probe (verdict falls back to auth-only for it).
    "trove_status_eu_host": _t(
        key="trove_status_eu_host",
        default=settings.trove_status_eu_host,
        type="str",
        category="trove_status",
        description=(
            "Hostname/IP of the EU Live game glsserver to TCP-probe. The "
            "stable port is 6560 (world-instance ports like :3701x are "
            "ephemeral). Retarget here if EU's glsserver host changes."
        ),
    ),
    "trove_status_eu_port": _t(
        key="trove_status_eu_port",
        default=settings.trove_status_eu_port,
        type="int",
        category="trove_status",
        description="TCP port of the EU Live game glsserver (default 6560). 0 disables.",
        min_value=0, max_value=65535,
    ),
    "trove_status_us_host": _t(
        key="trove_status_us_host",
        default=settings.trove_status_us_host,
        type="str",
        category="trove_status",
        description=(
            "Hostname/IP of the US Live game glsserver to TCP-probe. Default "
            "is the Dallas glsserver seen in the US capture; if that box is "
            "session-assigned, point this at a stable US host (e.g. "
            "trove-pc-live-us-game-5.trovegame.com) or a raw IP."
        ),
    ),
    "trove_status_us_port": _t(
        key="trove_status_us_port",
        default=settings.trove_status_us_port,
        type="int",
        category="trove_status",
        description="TCP port of the US Live game glsserver (default 6560). 0 disables.",
        min_value=0, max_value=65535,
    ),
    "trove_status_pts_host": _t(
        key="trove_status_pts_host",
        default=settings.trove_status_pts_host,
        type="str",
        category="trove_status",
        description="Hostname/IP of the PTS game glsserver to TCP-probe.",
    ),
    "trove_status_pts_port": _t(
        key="trove_status_pts_port",
        default=settings.trove_status_pts_port,
        type="int",
        category="trove_status",
        description="TCP port of the PTS game glsserver (default 6560). 0 disables.",
        min_value=0, max_value=65535,
    ),
    "trove_status_game_deep_probe": _t(
        key="trove_status_game_deep_probe",
        default=settings.trove_status_game_deep_probe,
        type="bool",
        category="trove_status",
        description=(
            "Deep game probe. OFF = connect-only (a down server that still accepts "
            "TCP on 6560 reads as a false 'online'). ON = after connecting, replay "
            "the glsserver hello and call the region online only if the server HOLDS "
            "the socket open; a server that drops right after the hello is flagged "
            "down. Any anomaly falls back to the connect-only verdict. Toggle OFF "
            "instantly if it ever misflags a live region."
        ),
    ),
    "trove_status_game_random_opener": _t(
        key="trove_status_game_random_opener",
        default=settings.trove_status_game_random_opener,
        type="bool",
        category="trove_status",
        description=(
            "Send a FRESH RANDOM ephemeral opener each deep probe instead of replaying "
            "a captured hello. The real client's opener is a per-connection random key "
            "(proven via live instrumentation), and a live glsserver holds the socket "
            "for any well-formed opener - so a random one behaves like a real client "
            "and NEVER goes stale on a Trove protocol update. Off = legacy replay of "
            "trove_status_{env}_hello_hex (goes stale → false 'down'). Leave on."
        ),
    ),
    "trove_status_eu_hello_hex": _t(
        key="trove_status_eu_hello_hex",
        default=settings.trove_status_eu_hello_hex,
        type="str",
        category="trove_status",
        description=(
            "Per-env deep-probe ENABLE flag for EU: non-empty = run the deep probe, "
            "empty = connect-only. With trove_status_game_random_opener on (default) "
            "the content is NOT sent (a fresh random opener is used); the string only "
            "needs to be non-empty. The captured hex is kept as the legacy-replay "
            "fallback (random off) - but a captured opener goes stale, so leave random on."
        ),
    ),
    "trove_status_us_hello_hex": _t(
        key="trove_status_us_hello_hex",
        default=settings.trove_status_us_hello_hex,
        type="str",
        category="trove_status",
        description=(
            "Per-env deep-probe ENABLE flag for US: non-empty = run the deep probe, "
            "empty = connect-only. Same semantics as the EU flag (random opener by "
            "default; the hex content is only replayed in legacy mode)."
        ),
    ),
    "trove_status_pts_hello_hex": _t(
        key="trove_status_pts_hello_hex",
        default=settings.trove_status_pts_hello_hex,
        type="str",
        category="trove_status",
        description=(
            "Captured glsserver client hello (hex) for PTS. Default EMPTY = "
            "connect-only, because the PTS endpoint (auth-pcpts01) is an AUTH "
            "gateway that DROPS a hello-only probe even when up, so the deep probe "
            "would false-flag maintenance. Set a hello only if PTS points at a real "
            "*-game-* glsserver that holds the socket open."
        ),
    ),
    "trove_status_game_hold_seconds": _t(
        key="trove_status_game_hold_seconds",
        default=settings.trove_status_game_hold_seconds,
        type="float",
        category="trove_status",
        description=(
            "Seconds the server must hold the socket open after the hello to count "
            "as online. A maintenance server drops within a beat; a playable one "
            "keeps the session socket open. Kept well under the per-probe timeout."
        ),
        min_value=0.2, max_value=10.0,
    ),
    "trove_status_probe_attempts": _t(
        key="trove_status_probe_attempts",
        default=settings.trove_status_probe_attempts,
        type="int",
        category="trove_status",
        description=(
            "Forgiveness: how many times to retry EACH probe (auth + each region's "
            "game socket) back-to-back within one cycle before marking it down. A "
            "probe counts ONLINE if any attempt succeeds (stops early on the first "
            "success); only this many CONSECUTIVE failures flip it to down. Absorbs "
            "a transient miss or a brief local network blip that would otherwise "
            "show a false outage. Retries are immediate (see the retry-delay knob), "
            "NOT one per minute. 1 = old behaviour (first failure = down)."
        ),
        min_value=1, max_value=10,
    ),
    "trove_status_probe_retry_delay_seconds": _t(
        key="trove_status_probe_retry_delay_seconds",
        default=settings.trove_status_probe_retry_delay_seconds,
        type="float",
        category="trove_status",
        description=(
            "Gap between the back-to-back retries above, in seconds (NOT the full "
            "probe interval). A short pause lets a momentary glitch clear without "
            "hammering. Default 2s; 0 = retry instantly."
        ),
        min_value=0.0, max_value=30.0,
    ),

    # ── Community feeds (/v1/feeds/{youtube,bilibili,twitch}) ──────────
    # Filter knobs for the natively-fetched video/stream feeds (see
    # app/trove/feeds.py). The list-shaped knobs are comma-separated
    # strings (runtime_config has no list type) - whitespace ignored, case
    # folded on parse, same convention as cheaters_excluded_board_uuids.
    # Twitch needs no filter knobs (it just lists every live Trove stream);
    # its category id + credentials live in settings.py / .env.
    "feeds_youtube_query": _t(
        key="feeds_youtube_query",
        default="Trove game",
        type="str",
        category="community_feeds",
        description=(
            "Search query the YouTube Data API runs for the /v1/feeds/youtube "
            "feed. Default 'Trove game' (the bare word 'Trove' pulls in too "
            "much unrelated treasure-hunting content)."
        ),
    ),
    "feeds_youtube_excluded_channels": _t(
        key="feeds_youtube_excluded_channels",
        default="scyushi,mikailstream,codelaunch,jmdeathpunchdd",
        type="str",
        category="community_feeds",
        description=(
            "Comma-separated YouTube channel names to drop from the feed "
            "(case-insensitive exact match on channel title). Use for "
            "off-topic or spammy channels that keep ranking for the query."
        ),
    ),
    "feeds_youtube_excluded_title_terms": _t(
        key="feeds_youtube_excluded_title_terms",
        default="trinket trove",
        type="str",
        category="community_feeds",
        description=(
            "EXCLUDE list for the /v1/feeds/youtube relevance filter: comma-"
            "separated whole-word terms; a video whose title/description/tags "
            "contain any of them is dropped. Default catches the unrelated "
            "'Trinket Trove' series."
        ),
    ),
    "feeds_youtube_require_terms": _t(
        key="feeds_youtube_require_terms",
        default="trove",
        type="str",
        category="community_feeds",
        description=(
            "REQUIRE list for the /v1/feeds/youtube relevance filter: comma-"
            "separated whole-word terms a video's title/description/tags must "
            "ALL contain (case-insensitive). Default 'trove'. Leave blank to "
            "require nothing (not recommended - the search alone is noisy)."
        ),
    ),
    "feeds_youtube_relevance_terms": _t(
        key="feeds_youtube_relevance_terms",
        default=(
            "trovesaurus,gamigo,trion,geode,delve,paragon,mastery,cornerstone,"
            "chaos chest,shadow tower,club world,radiant,lunar lancer,"
            "candy barbarian,neon ninja,shadow hunter,dino tamer,boomeranger,"
            "dracolyte,vanguardian,chloromancer,fae trickster,pirate captain,"
            "tomb raiser,solarion,gunslinger"
        ),
        type="str",
        category="community_feeds",
        description=(
            "SIGNAL list for the /v1/feeds/youtube relevance filter: comma-"
            "separated Trove-distinctive whole-word terms. A video is kept only "
            "if its title/description/tags contain at least one of these OR it "
            "sits in the gaming category (feeds_youtube_video_category_id). This "
            "is the self-curated relevance model - tune it to widen/tighten what "
            "counts as Trove. Leave blank to disable the signal gate."
        ),
    ),
    "feeds_youtube_video_category_id": _t(
        key="feeds_youtube_video_category_id",
        default="20",
        type="str",
        category="community_feeds",
        description=(
            "YouTube video category id that counts as a relevance SIGNAL for "
            "/v1/feeds/youtube (default '20' = Gaming). A video in this category "
            "passes the relevance gate even without a feeds_youtube_relevance_terms "
            "match. NOTE: it's the video's real (uploader-assigned) category from "
            "videos.list, used as a soft signal - NOT a hard search filter (which "
            "would drop legit miscategorised Trove videos). Blank to ignore category."
        ),
    ),
    "feeds_bilibili_keyword": _t(
        key="feeds_bilibili_keyword",
        default="宝藏世界trove",
        type="str",
        category="community_feeds",
        description=(
            "Search keyword scraped from search.bilibili.com for the "
            "/v1/feeds/bilibili feed. Default is the Chinese name for Trove "
            "(宝藏世界) plus 'trove'."
        ),
    ),
    "feeds_video_cutoff_days": _t(
        key="feeds_video_cutoff_days",
        default=90,
        type="int",
        category="community_feeds",
        description=(
            "Drop YouTube/Bilibili videos older than this many days. Default "
            "90 (~3 months) keeps the feed current without going stale-empty "
            "during quiet stretches."
        ),
        min_value=1, max_value=365,
    ),
    "feeds_per_channel_max": _t(
        key="feeds_per_channel_max",
        default=3,
        type="int",
        category="community_feeds",
        description=(
            "Maximum videos kept per channel/creator before the global "
            "newest-N trim, so one prolific uploader can't dominate the feed."
        ),
        min_value=1, max_value=20,
    ),
    "feeds_max_items": _t(
        key="feeds_max_items",
        default=10,
        type="int",
        category="community_feeds",
        description=(
            "Number of videos served per YouTube/Bilibili feed after "
            "per-channel capping and newest-first sorting."
        ),
        min_value=1, max_value=50,
    ),
}


# ─── Lookups ─────────────────────────────────────────────────────────────


_CACHE: dict[str, tuple[Any, float]] = {}
_CACHE_TTL_SECONDS = 5.0


class UnknownSettingError(KeyError):
    """Raised when callers reference a tunable that isn't in REGISTRY.
    Distinct from a built-in KeyError so the router can map it to 404."""


class InvalidSettingError(ValueError):
    """Raised when a write fails validation (wrong type, out of range,
    choice not in list). Mapped to 400 at the router."""


def _spec(key: str) -> TunableSetting:
    spec = REGISTRY.get(key)
    if spec is None:
        raise UnknownSettingError(key)
    return spec


def _validate(spec: TunableSetting, value: Any) -> Any:
    """Coerce + range-check a write. Returns the canonical value to persist."""
    if spec.type == "str":
        if not isinstance(value, str):
            raise InvalidSettingError(f"{spec.key} expects a string")
        if spec.choices is not None and value not in spec.choices:
            raise InvalidSettingError(
                f"{spec.key} must be one of {list(spec.choices)}"
            )
        return value
    if spec.type == "bool":
        if not isinstance(value, bool):
            raise InvalidSettingError(f"{spec.key} expects a boolean")
        return value
    if spec.type == "int":
        if isinstance(value, bool) or not isinstance(value, int):
            raise InvalidSettingError(f"{spec.key} expects an integer")
        if spec.min_value is not None and value < spec.min_value:
            raise InvalidSettingError(
                f"{spec.key} must be >= {spec.min_value}"
            )
        if spec.max_value is not None and value > spec.max_value:
            raise InvalidSettingError(
                f"{spec.key} must be <= {spec.max_value}"
            )
        return value
    if spec.type == "float":
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            raise InvalidSettingError(f"{spec.key} expects a number")
        v = float(value)
        if spec.min_value is not None and v < spec.min_value:
            raise InvalidSettingError(
                f"{spec.key} must be >= {spec.min_value}"
            )
        if spec.max_value is not None and v > spec.max_value:
            raise InvalidSettingError(
                f"{spec.key} must be <= {spec.max_value}"
            )
        return v
    raise InvalidSettingError(f"unsupported type {spec.type!r}")


async def get_setting(key: str) -> Any:
    """Resolved value for a tunable: Mongo override if present, else
    registry default. Cached per-key for ``_CACHE_TTL_SECONDS``."""
    spec = _spec(key)
    now = time.monotonic()
    cached = _CACHE.get(key)
    if cached is not None and (now - cached[1]) < _CACHE_TTL_SECONDS:
        return cached[0]
    doc = await RuntimeConfig.find_one(RuntimeConfig.key == key)
    if doc is None:
        value = spec.default
    else:
        try:
            value = json.loads(doc.value)
        except json.JSONDecodeError:
            # Bad row in DB shouldn't take down hot paths - fall back to
            # default. ``set_setting`` validates before persisting so this
            # only fires if the DB was edited by hand.
            value = spec.default
    _CACHE[key] = (value, now)
    return value


async def set_setting(
    key: str, value: Any, updated_by: PydanticObjectId | None,
) -> Any:
    """Persist an override. Returns the validated value actually stored."""
    spec = _spec(key)
    validated = _validate(spec, value)
    encoded = json.dumps(validated)
    doc = await RuntimeConfig.find_one(RuntimeConfig.key == key)
    if doc is None:
        await RuntimeConfig(
            key=key, value=encoded, updated_at=utcnow(),
            updated_by_user_id=updated_by,
        ).insert()
    else:
        doc.value = encoded
        doc.updated_at = utcnow()
        doc.updated_by_user_id = updated_by
        await doc.save()
    _CACHE.pop(key, None)
    return validated


async def reset_setting(key: str) -> None:
    """Drop the override → next read returns the code default."""
    _spec(key)  # validate key exists
    doc = await RuntimeConfig.find_one(RuntimeConfig.key == key)
    if doc is not None:
        await doc.delete()
    _CACHE.pop(key, None)


async def get_rate_limit(prefix: str) -> tuple[int, int]:
    """Fetch the ``(max, window_seconds)`` pair for one rate-limit family.

    Every limit registered above follows the ``<prefix>_max`` /
    ``<prefix>_window_seconds`` naming convention. This helper keeps
    consumer code from having to spell both names out at every call site.
    """
    max_ = await get_setting(f"{prefix}_max")
    window = await get_setting(f"{prefix}_window_seconds")
    return max_, window


async def list_all() -> list[dict]:
    """Every known tunable + its current resolved value, for the admin UI."""
    docs = await RuntimeConfig.find_all().to_list()
    overrides = {d.key: d for d in docs}
    out: list[dict] = []
    for key, spec in REGISTRY.items():
        override = overrides.get(key)
        if override is not None:
            try:
                current = json.loads(override.value)
            except json.JSONDecodeError:
                current = spec.default  # treat corrupt as missing
            updated_at = override.updated_at
            updated_by = override.updated_by_user_id
            is_default = False
        else:
            current = spec.default
            updated_at = None
            updated_by = None
            is_default = True
        out.append({
            "key": key,
            "category": spec.category,
            "type": spec.type,
            "description": spec.description,
            "secret": spec.secret,
            "min_value": spec.min_value,
            "max_value": spec.max_value,
            "choices": list(spec.choices) if spec.choices else None,
            "default": spec.default,
            "value": current,
            "is_default": is_default,
            "updated_at": updated_at,
            "updated_by_user_id": str(updated_by) if updated_by else None,
        })
    return out
