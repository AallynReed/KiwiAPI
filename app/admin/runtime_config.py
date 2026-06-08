"""Runtime-tunable settings: master can change them via the admin panel,
no env-var edit or container restart needed.

How it stacks:

  ┌─ REGISTRY (this file) — declares the *known* keys with their defaults,
  │  types, categories, descriptions. Static; adding a new tunable means
  │  adding an entry here and shipping a deploy.
  │
  └─ RuntimeConfig collection in Mongo — sparse overrides. Only keys the
     master has explicitly set live here. ``get_setting(key)`` returns the
     override if present, else the registry default.

Reads are cached per-key with a short TTL (5s by default) so a hot path
like the feedback rate limiter doesn't issue a Mongo round-trip on every
request. Writes invalidate the cache entry for the affected key.

Adding a new tunable:
    1. Append a ``TunableSetting`` entry to ``REGISTRY``.
    2. Read it with ``await runtime_config.get_setting("your.key")``.
    3. Add a description so the admin UI explains what it does.

Type discipline: every tunable declares its primitive type and (for
numeric ones) a valid range. ``set_setting`` raises on invalid input
BEFORE persisting, so the DB only ever holds valid values. Cache keeps
strict types — int stays int, never a string from the JSON envelope.
"""

from __future__ import annotations

import json
import time
from dataclasses import dataclass, field
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

    ``value`` is JSON-encoded so the same collection can hold strings,
    ints, bools, and (later) compound values without per-type columns.
    The encode/decode happens in :func:`set_setting` / :func:`get_setting`
    and the cache, never by callers."""

    key: str  # registry key, e.g. "feedback.discord_webhook"
    value: str  # JSON-encoded — decoded by get_setting()
    updated_at: datetime = Field(default_factory=utcnow)
    updated_by_user_id: PydanticObjectId | None = None  # audit trail

    class Settings:
        name = "runtime_config"
        indexes = [IndexModel([("key", ASCENDING)], unique=True)]


# ─── Registry ────────────────────────────────────────────────────────────


SettingType = Literal["str", "int", "bool", "float"]


@dataclass(frozen=True)
class TunableSetting:
    """One known tunable. Static metadata — values live in Mongo."""

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
            "successful both count — this is brute-force defence."
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
            "Storage tier cutoff: rows older than this many days are "
            "moved from the fast LeaderboardEntry collection into "
            "LeaderboardEntryArchive at the tail of each insert. With "
            "hourly captures, 3 days = ~72 anchors per board kept hot. "
            "Lower values keep the hot collection's index footprint "
            "small; higher values keep more reads on the fast path."
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
            "matches leaderboards_hot_retention_days so the \"is this a "
            "hot or cold collection lookup\" question and the \"do you pay "
            "the archive rate limit\" question have the same answer."
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
            "⚠ Requires an API container restart to take effect — the "
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
            # Bad row in DB shouldn't take down hot paths — fall back to
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
