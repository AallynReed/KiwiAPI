"""Design-feedback tallies.

One plain Mongo collection (atomic ``$inc`` upsert, no Beanie model), in the same
shape as ``app/bot/stats.py``: a counter per ``(UTC day, design, theme, vote)``.
Nothing about the voter is stored - no IP, no hash, no cookie id - so the only
thing this can ever answer is "how many up and down votes, and on which look".
Repeat votes are held back by the browser (localStorage) and a per-IP rate limit.
"""
from datetime import timedelta

from app.core.database import get_db
from app.core.utils import utcnow

COLLECTION = "design_feedback"

DESIGNS = ("voxel", "classic")
THEMES = ("auto", "light", "dark")
VOTES = ("up", "down")


async def record_vote(design: str, theme: str, vote: str) -> None:
    now = utcnow()
    day = f"{now:%Y-%m-%d}"
    await get_db()[COLLECTION].update_one(
        {"_id": f"{day}|{design}|{theme}|{vote}"},
        {"$inc": {"count": 1},
         "$set": {"day": day, "design": design, "theme": theme, "vote": vote,
                  "updated_at": now}},
        upsert=True,
    )


async def summary(days: int) -> dict:
    """Up/down totals over the last ``days`` UTC days, split by design, by theme,
    and per day (oldest first), for the Dev Portal's Site Analytics tab."""
    since = f"{utcnow() - timedelta(days=days - 1):%Y-%m-%d}"
    rows = await get_db()[COLLECTION].find({"day": {"$gte": since}}).to_list(length=None)

    def blank() -> dict:
        return {"up": 0, "down": 0}

    total = blank()
    by_design: dict[str, dict] = {d: blank() for d in DESIGNS}
    by_theme: dict[str, dict] = {t: blank() for t in THEMES}
    by_day: dict[str, dict] = {}
    for r in rows:
        vote, n = r.get("vote"), int(r.get("count") or 0)
        if vote not in VOTES:
            continue
        total[vote] += n
        by_design.setdefault(r.get("design") or "?", blank())[vote] += n
        by_theme.setdefault(r.get("theme") or "?", blank())[vote] += n
        by_day.setdefault(r.get("day") or "?", blank())[vote] += n

    return {
        "days": days,
        "total": total,
        "by_design": by_design,
        "by_theme": by_theme,
        "series": [{"day": d, **v} for d, v in sorted(by_day.items())],
    }
