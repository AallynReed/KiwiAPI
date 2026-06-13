"""Push Kiwi's Discord slash commands to Discord (bulk overwrite).

Editing ``COMMAND_DEFS`` changes nothing until they're PUT to Discord's API; that
push is triggered from the admin panel (superuser) so there's no manual CLI step.
A global push can take up to ~1h to appear in clients; a per-guild push (pass a
guild id) is instant and handy while iterating.
"""
from __future__ import annotations

import logging

import httpx

from app.core.config import settings
from app.discord.commands import COMMAND_DEFS

logger = logging.getLogger("kiwi.discord")

_API = "https://discord.com/api/v10"


class DiscordRegistrationError(Exception):
    """Config missing, Discord unreachable, or Discord rejected the push."""


def _require_creds() -> tuple[str, str]:
    app_id = settings.discord_client_id
    token = settings.discord_bot_token
    if not app_id or not token:
        raise DiscordRegistrationError(
            "discord_client_id and discord_bot_token must be set in the environment."
        )
    return app_id, token


async def _put_commands(url: str, payload: list, token: str) -> list[dict]:
    """Bulk-overwrite the command set at ``url`` (global or guild). Raises
    ``DiscordRegistrationError`` on an unreachable / non-2xx Discord (the message
    carries Discord's response so the panel can show why)."""
    try:
        async with httpx.AsyncClient(timeout=30) as client:
            resp = await client.put(url, headers={"Authorization": f"Bot {token}"}, json=payload)
    except httpx.HTTPError as exc:
        raise DiscordRegistrationError(f"Could not reach Discord: {exc}") from exc
    if resp.status_code >= 300:
        raise DiscordRegistrationError(
            f"Discord rejected the request ({resp.status_code}): {resp.text[:400]}"
        )
    return resp.json()


async def register_commands(guild_id: str | None = None) -> list[dict]:
    """Bulk-overwrite the application's commands with ``COMMAND_DEFS``.

    Returns the command objects Discord echoes back.
    """
    app_id, token = _require_creds()
    base = f"{_API}/applications/{app_id}/commands"
    url = base if not guild_id else f"{_API}/applications/{app_id}/guilds/{guild_id}/commands"
    cmds = await _put_commands(url, COMMAND_DEFS, token)
    logger.info(
        "Pushed %d Discord command(s)%s", len(cmds),
        f" to guild {guild_id}" if guild_id else " globally",
    )
    return cmds


async def clear_guild_commands(guild_id: str) -> None:
    """Remove ALL guild-scoped slash commands for one guild (bulk-overwrite with an
    empty set). Kills duplicates left over from an instant per-guild test push - the
    global commands then show alone. Never touches the global command set, so a
    guild id is required."""
    app_id, token = _require_creds()
    url = f"{_API}/applications/{app_id}/guilds/{guild_id}/commands"
    await _put_commands(url, [], token)
    logger.info("Cleared guild-scoped Discord commands for guild %s", guild_id)
