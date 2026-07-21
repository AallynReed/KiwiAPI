"""Keep each linked SiteUser's cached Discord avatar hash fresh from the gateway.

The website stores only the Discord avatar HASH (``SiteUser.discord_avatar``) and
builds the ``cdn.discordapp.com`` URL on the fly - it never hosts the image. That
hash is normally re-synced when the user logs in via Discord OAuth
(app/site_auth/oauth.py), so between logins it goes stale if the user changes their
picture. We deliberately don't store a Discord refresh token (data minimization), so
there's no polling path.

The gateway bot, however, already sees a profile change for any user it shares a
guild with - discord.py's ``on_user_update`` event (which needs the members intent).
This module mirrors the new hash straight into the account: no relogin, no new stored
secret, nothing persisted beyond the one hash we already keep.
"""
from __future__ import annotations

import logging
from typing import Any

from app.core.utils import utcnow
from app.site_auth.models import SiteUser

logger = logging.getLogger("kiwi.bot")


def avatar_hash(user: Any) -> str | None:
    """The user's custom avatar hash (the ``a_`` prefix is kept for animated
    avatars), or ``None`` when they have no custom avatar - matching exactly what
    the OAuth login stores (``me.get("avatar")``)."""
    asset = getattr(user, "avatar", None)
    return asset.key if asset is not None else None


async def sync_avatar(user: Any) -> bool:
    """Mirror ``user``'s current Discord avatar hash onto the matching SiteUser.

    Best-effort and idempotent: a no-op (returns ``False``) when no account is
    linked to that Discord id or the stored hash already matches. Self-deleted
    accounts are tombstoned with ``discord_id=None`` so they never match. Returns
    ``True`` only when a row was actually updated.
    """
    discord_id = getattr(user, "id", None)
    if discord_id is None:
        return False
    new_hash = avatar_hash(user)
    account = await SiteUser.find_one(SiteUser.discord_id == int(discord_id))
    if account is None or account.discord_avatar == new_hash:
        return False
    account.discord_avatar = new_hash
    account.updated_at = utcnow()
    await account.save()
    logger.info("avatar sync: refreshed cached avatar for discord_id=%s", discord_id)
    return True
