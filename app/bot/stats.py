"""Bot usage statistics.

Two plain Mongo collections (atomic ``$inc`` / upsert - no Beanie model needed):
- ``bot_presence``: a singleton ``{_id:"current"}`` doc the gateway bot updates with
  the servers it's in + the users it can see (sum of guild member counts).
- ``bot_command_usage``: one doc per slash command (``_id=<name>``), incremented by
  the API's interactions endpoint each time the command runs.

The admin endpoint (``GET /admin/bot/stats``, master-only) reads both for the Dev
Portal. All writes are best-effort - stats must never break a command or the bot.
"""
import logging

from app.core.database import get_db
from app.core.utils import utcnow

logger = logging.getLogger("kiwi.bot.stats")

_PRESENCE = "bot_presence"
_COMMANDS = "bot_command_usage"


async def record_presence(guild_count: int, member_count: int) -> None:
    """Upsert the gateway bot's reach (servers + users it can see)."""
    try:
        await get_db()[_PRESENCE].update_one(
            {"_id": "current"},
            {"$set": {"guild_count": guild_count, "member_count": member_count,
                      "updated_at": utcnow()}},
            upsert=True,
        )
    except Exception:
        logger.warning("stats: presence write failed", exc_info=True)


async def record_command(name: str) -> None:
    """Increment a slash command's usage counter (atomic upsert)."""
    if not name:
        return
    try:
        await get_db()[_COMMANDS].update_one(
            {"_id": name},
            {"$inc": {"count": 1}, "$set": {"last_used_at": utcnow()}},
            upsert=True,
        )
    except Exception:
        logger.warning("stats: command write failed", exc_info=True)


async def get_stats() -> dict:
    """Presence + per-command usage, most-used first - for the master Dev Portal."""
    db = get_db()
    presence = await db[_PRESENCE].find_one({"_id": "current"}) or {}
    commands = []
    async for c in db[_COMMANDS].find().sort("count", -1):
        commands.append({
            "name": c["_id"],
            "count": int(c.get("count", 0)),
            "last_used_at": c.get("last_used_at"),
        })
    return {
        "guild_count": presence.get("guild_count"),
        "member_count": presence.get("member_count"),
        "updated_at": presence.get("updated_at"),
        "total_commands": sum(c["count"] for c in commands),
        "commands": commands,
    }
