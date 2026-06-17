"""Kiwi Discord slash-command definitions + interaction dispatch.

``COMMAND_DEFS`` is the source of truth for the commands registered with Discord
(pushed from the admin panel - see ``app/discord/registration.py``). ``handle``
maps an incoming APPLICATION_COMMAND interaction to its response.

All responses are PUBLIC (never ephemeral) - the bot is meant to be visible in
the channel. The HTTP + signature layer lives in ``router.py``; this module is
pure command logic.

Adding a command: append a def to ``COMMAND_DEFS``, add a branch in ``handle``,
re-run the admin "Push to Discord" button.
"""
from __future__ import annotations

import logging

from app import i18n
from app.discord.embeds import (
    ACTIVITY_LABELS,
    activity_embed,
    bonuses_embed,
    challenge_embed,
    changelog_embed,
    chaos_embed,
    corruxion_embed,
    download_embed,
    fluxion_embed,
    giveaways_embed,
    longshade_embed,
    ping_embed,
    servertime_embed,
    stampy_embed,
    status_embed,
    trove_news_embed,
    web_embed,
    wild_mana_embed,
)
from app.i18n import t

logger = logging.getLogger("kiwi.discord")

# Interaction + response type ids (Discord API) and the EPHEMERAL message flag.
_TYPE_APPLICATION_COMMAND = 2
_RESP_MESSAGE = 4
_FLAG_EPHEMERAL = 64

# Application-command + option types, and installation / usage contexts.
_CMD_CHAT_INPUT = 1
_OPT_STRING = 3
_INTEGRATION_GUILD, _INTEGRATION_USER = 0, 1
_CONTEXT_GUILD, _CONTEXT_BOT_DM, _CONTEXT_PRIVATE = 0, 1, 2

# Period choices for /activity (value = the api period token; default 7d).
_ACTIVITY_CHOICES = [
    {"name": "1 Day", "value": "1d"},
    {"name": "7 Days", "value": "7d"},
    {"name": "1 Month", "value": "1m"},
    {"name": "3 Months", "value": "3m"},
    {"name": "6 Months", "value": "6m"},
    {"name": "1 Year", "value": "1y"},
    {"name": "All Time", "value": "all"},
]


def _cmd(name: str, description: str, options: list | None = None) -> dict:
    """A CHAT_INPUT command def, usable as a guild app OR a user-installed app,
    in servers, bot DMs, and private channels."""
    d = {
        "name": name,
        "type": _CMD_CHAT_INPUT,
        "description": description,
        "integration_types": [_INTEGRATION_GUILD, _INTEGRATION_USER],
        "contexts": [_CONTEXT_GUILD, _CONTEXT_BOT_DM, _CONTEXT_PRIVATE],
    }
    if options:
        d["options"] = options
    return d


COMMAND_DEFS = [
    _cmd("status", "Show live Trove server status (EU / US / PTS)."),
    _cmd("activity", "Show the Trove player-activity trend with a chart.", options=[{
        "name": "period",
        "description": "Time range (default: 7 days)",
        "type": _OPT_STRING,
        "required": False,
        "choices": _ACTIVITY_CHOICES,
    }]),
    _cmd("chaos", "Show this week's Chaos Chest featured item."),
    _cmd("servertime", "Show the current Trove server time (UTC−11) and next resets."),
    _cmd("bonuses", "Show today's daily bonus and this week's weekly bonus."),
    _cmd("hourly_challenge", "Show the current and previous hourly challenge."),
    _cmd("longshade", "Show the current Depth-15 delve biome rotation."),
    _cmd("giveaways", "Show current, upcoming, and recently-ended giveaways."),
    _cmd("corruxion", "Corruxion merchant - is it here now, or when does it arrive?"),
    _cmd("fluxion", "Fluxion merchant - its voting/selling stage and timing."),
    _cmd("stampy", "The Stampy event - current/next window and biome."),
    _cmd("wild_mana", "The weekly Wild Mana biome rotation."),
    _cmd("trove_news", "The latest Trove news."),
    _cmd("download", "Download the latest Better Trove Tools (all platforms)."),
    _cmd("web", "Use Better Trove Tools in your browser."),
    _cmd("change_log", "Better Trove Tools changelog (recent changes)."),
    _cmd("ping", "Check the Kiwi API is online and responsive."),
]


def _embed(embed: dict, ephemeral: bool = False) -> dict:
    """A CHANNEL_MESSAGE_WITH_SOURCE response carrying one embed.

    Public by default (the bot is meant to be visible). ``ephemeral`` is used
    only for /change_log, which the user asked to keep private.
    """
    data: dict = {"embeds": [embed]}
    if ephemeral:
        data["flags"] = _FLAG_EPHEMERAL
    return {"type": _RESP_MESSAGE, "data": data}


def _message(content: str) -> dict:
    """A public plain-text response (no ephemeral flag - the bot is visible)."""
    return {"type": _RESP_MESSAGE, "data": {"content": content}}


def _option(interaction: dict, name: str, default=None):
    """Read a top-level command option value by name."""
    for opt in (interaction.get("data") or {}).get("options") or []:
        if opt.get("name") == name:
            return opt.get("value")
    return default


async def handle(interaction: dict) -> dict:
    """Build the interaction response for an APPLICATION_COMMAND.

    Dispatches on the command name. Wrapped in a guard so a handler error returns
    a valid (visible) message and is logged, instead of a 500 that Discord shows
    as "the application did not respond".
    """
    if interaction.get("type") != _TYPE_APPLICATION_COMMAND:
        return _message(t("🥝 Unsupported interaction."))

    # Speak the server's configured bot language (dashboard setting), falling back
    # to the user's Discord locale, then English. Set once - the embed builders
    # read it via the i18n context.
    i18n.set_current_language(await i18n.guild_language(
        interaction.get("guild_id"),
        interaction.get("guild_locale") or interaction.get("locale"),
    ))

    name = (interaction.get("data") or {}).get("name")
    try:
        from app.bot import stats  # count usage (best-effort) for the master stats
        await stats.record_command(name)
    except Exception:
        logger.warning("command usage record failed", exc_info=True)
    try:
        if name == "status":
            return _embed(await status_embed())
        if name == "activity":
            period = _option(interaction, "period", "7d")
            return _embed(await activity_embed(period if period in ACTIVITY_LABELS else "7d"))
        if name == "chaos":
            return _embed(await chaos_embed())
        if name == "servertime":
            return _embed(servertime_embed())
        if name == "bonuses":
            return _embed(bonuses_embed())
        if name == "hourly_challenge":
            return _embed(await challenge_embed())
        if name == "longshade":
            return _embed(longshade_embed())
        if name == "giveaways":
            return _embed(await giveaways_embed())
        if name == "corruxion":
            return _embed(corruxion_embed())
        if name == "fluxion":
            return _embed(fluxion_embed())
        if name == "stampy":
            return _embed(stampy_embed())
        if name == "wild_mana":
            return _embed(wild_mana_embed())
        if name == "trove_news":
            return _embed(await trove_news_embed())
        if name == "download":
            return _embed(await download_embed())
        if name == "web":
            return _embed(web_embed())
        if name == "change_log":
            return _embed(await changelog_embed(), ephemeral=True)
        if name == "ping":
            return _embed(await ping_embed())
    except Exception:
        logger.exception("discord command /%s failed", name)
        return _message(t("🥝 Sorry, `/{name}` hit a snag. Try again in a moment.", name=name))

    return _message(t("🥝 Unknown command: {name}", name=name))
