"""Public clubs directory for the ``/clubs`` page.

Joins the public ``Club`` docs (Mongo - set via the Discord dashboard) with their
ranking on the in-game club leaderboard (board ``1100`` in Postgres). Clubs are
ordered by leaderboard rank; public clubs not on the board follow, unranked.
"""
import logging

logger = logging.getLogger("kiwi.site.clubs")

_CLUB_LEADERBOARD_UUID = 1100


async def public_clubs_ordered() -> list[dict]:
    """Public clubs, ordered by their rank on board 1100 (unranked clubs last)."""
    from app.bot.models import Club

    clubs = await Club.find(Club.public == True).to_list()  # noqa: E712
    if not clubs:
        return []

    ranks: dict[str, int] = {}     # normalized club name -> leaderboard rank
    try:
        from app.trove.leaderboards import pg_store
        anchor = await pg_store.latest_anchor_for_board(_CLUB_LEADERBOARD_UUID)
        if anchor is not None:
            entries, _ = await pg_store.list_entries(
                _CLUB_LEADERBOARD_UUID, anchor, limit=2000, offset=0)
            for e in entries:
                key = (e.get("player_name") or "").strip().lower()
                if key and key not in ranks:
                    ranks[key] = e["rank"]
    except Exception:
        logger.warning("clubs: leaderboard %d unavailable", _CLUB_LEADERBOARD_UUID, exc_info=True)

    def _rank(c) -> int | None:
        return ranks.get((c.name or "").strip().lower())

    # ranked first (by rank), then the rest alphabetically.
    clubs.sort(key=lambda c: (0, _rank(c), "") if _rank(c) is not None
               else (1, 0, (c.name or "").lower()))
    return [
        {
            "name": c.name,
            "rank": _rank(c),
            "description": c.description,
            "banner_url": c.banner_url,
            "avatar_url": c.avatar_url,
            "discord_url": c.discord_url,
            "website_url": c.website_url,
        }
        for c in clubs
    ]
