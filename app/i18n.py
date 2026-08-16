"""Server-side i18n for the bot's user-facing text (Discord embeds, announcements,
board, images, errors).

Mirrors the website's runtime approach (see ``site/static/i18n.js``): English is
the source of truth and lives in the code; each locale is a
``{normalized-English: translation}`` map. We look the English string up in two
catalogs, in order, and the FIRST hit wins:

  1. ``app/locales/<lang>.json`` - bot-specific overlay (bot wording, our own
     translations); takes precedence so bot phrasing isn't pulled off-key by a
     site string that happens to normalize the same.
  2. ``site/static/locales/<lang>.json`` - the website's catalog, reused so any
     bot string that matches a site string is translated for free.

Anything missing falls back to English. ``t()`` then applies ``str.format(**fmt)``
so placeholders (kept verbatim in the translations, e.g. ``"Ends {when}"``) fill
in after lookup. The supported language set mirrors the site's language picker.
"""
from __future__ import annotations

import contextvars
import json
import logging
import re
from functools import lru_cache
from pathlib import Path

logger = logging.getLogger("kiwi.i18n")

# (code, endonym) - the language list, identical to site/static/i18n.js LANGS.
# Endonyms are shown in their own language and never translated.
LANGS: list[tuple[str, str]] = [
    ("en", "English"),
    ("fr", "Français"),
    ("de", "Deutsch"),
    ("pt-PT", "Português"),
    ("es", "Español"),
    ("ru", "Русский"),
    ("ja", "日本語"),
    ("ko", "한국어"),
    ("zh-CN", "简体中文"),
    ("th", "ไทย"),
]
SUPPORTED: frozenset[str] = frozenset(c for c, _ in LANGS)
DEFAULT_LANG = "en"

_REPO_ROOT = Path(__file__).resolve().parent.parent
_SITE_LOCALES = _REPO_ROOT / "site" / "static" / "locales"
_BOT_LOCALES = Path(__file__).resolve().parent / "locales"

_WS = re.compile(r"\s+")


def _norm(s: str) -> str:
    """Collapse whitespace + trim, matching the site's ``norm`` so the same key
    resolves in both catalogs regardless of source-formatting differences."""
    return _WS.sub(" ", s).strip()


def normalize_lang(lang: str | None) -> str:
    """A supported language code, or the English default."""
    return lang if lang in SUPPORTED else DEFAULT_LANG


# The active language for the current async context. Set once at a boundary
# (a slash-command dispatch, or per-language in the announcer) so the embed/
# board builders can just call ``t("...")`` without threading a ``lang`` arg
# through every signature. ContextVars are per-task, so concurrent requests in
# different languages don't interfere.
_current_lang: contextvars.ContextVar[str] = contextvars.ContextVar(
    "kiwi_lang", default=DEFAULT_LANG)


def set_current_language(lang: str | None) -> None:
    """Set the active language for this async context (validated)."""
    _current_lang.set(normalize_lang(lang))


def current_language() -> str:
    return _current_lang.get()


def _load(path: Path) -> dict[str, str]:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return {}
    except (OSError, ValueError):
        logger.warning("i18n: could not read locale file %s", path, exc_info=True)
        return {}
    return {_norm(k): v for k, v in data.items()
            if isinstance(k, str) and isinstance(v, str) and v}


@lru_cache(maxsize=len(LANGS))
def _catalog(lang: str) -> dict[str, str]:
    """Merged ``{normalized-English: translation}`` for ``lang`` (bot overlay wins).
    Cached for the process; English is the empty dict (identity)."""
    if lang == "en" or lang not in SUPPORTED:
        return {}
    merged = _load(_SITE_LOCALES / f"{lang}.json")
    merged.update(_load(_BOT_LOCALES / f"{lang}.json"))     # bot overlay precedence
    return merged


def t(text: str, lang: str | None = None, /, **fmt) -> str:
    """Translate ``text`` (an English source string) into ``lang`` (or the active
    context language when ``lang`` is None), then fill any ``{placeholders}`` from
    ``fmt``. Unknown language or missing translation → English. Keep
    ``{placeholders}`` verbatim in the translations."""
    out = text
    code = normalize_lang(lang if lang is not None else _current_lang.get())
    if code != "en":
        hit = _catalog(code).get(_norm(text))
        if hit:
            out = hit
    if not fmt:
        return out
    try:
        return out.format(**fmt)
    except (KeyError, IndexError, ValueError):
        # A malformed translation must never break a Discord post - fall back to
        # formatting the English source (which we author and know is valid).
        try:
            return text.format(**fmt)
        except (KeyError, IndexError, ValueError):
            return text


# ── language resolution (per Discord guild / interaction) ────────────────────

# Discord client locales -> our supported codes (best-effort, for DMs or guilds
# without a configured language). Discord uses e.g. "en-US", "pt-BR", "zh-TW".
_DISCORD_LOCALE: dict[str, str] = {
    "en-US": "en", "en-GB": "en", "fr": "fr", "de": "de",
    "pt-BR": "pt-PT", "pt-PT": "pt-PT", "ru": "ru",
    "ja": "ja", "zh-CN": "zh-CN", "zh-TW": "zh-CN",
    "ko": "ko", "es-ES": "es", "es-419": "es", "th": "th",
}


def discord_locale_to_lang(loc: str | None) -> str:
    """Map a Discord client/guild locale to a supported code (English fallback)."""
    if not loc:
        return DEFAULT_LANG
    if loc in SUPPORTED:
        return loc
    return _DISCORD_LOCALE.get(loc, DEFAULT_LANG)


async def guild_language(guild_id, fallback_locale: str | None = None) -> str:
    """The bot's configured language for a guild (``GuildConfig.language``). Falls
    back to the Discord ``fallback_locale`` mapping, then English. Never raises."""
    if guild_id:
        try:
            from app.bot.models import GuildConfig
            cfg = await GuildConfig.find_one(GuildConfig.guild_id == int(guild_id))
            if cfg is not None and cfg.language in SUPPORTED:
                return cfg.language
        except Exception:  # noqa: BLE001 - a config/DB hiccup must never break a reply
            logger.warning("i18n: guild_language lookup failed for %s", guild_id, exc_info=True)
    return discord_locale_to_lang(fallback_locale)
