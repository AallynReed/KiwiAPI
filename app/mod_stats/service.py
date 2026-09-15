"""Daily statistics for one creator's mods on the Mods Hub, Trovesaurus and Steam Workshop.

The hub's counts come from Mongo. A project's ``download_count`` adds up every release,
so a player who takes each update counts once per update; the hub figure here is the
most-downloaded single release instead, which no player downloads twice.
Trovesaurus's come from its ``/api/mods-all``
catalog and Steam's from ``GetPublishedFileDetails``, which needs no key for public
items. Neither platform can be searched by our title, so the ids are TroveUI's own
``trovesaurus_ids.json`` and ``steam_ids.json``, keyed by mod title and read out of
the bare repo the custom art worker also pulls from. One snapshot is kept per UTC day;
the refresher checks hourly and only collects when today has none yet.
"""
import asyncio
import json
import logging

import httpx

from app.core.config import settings
from app.core.http import KIWI_UA, fetch
from app.core.refresher import PeriodicRefresher
from app.core.utils import iso, utcnow
from app.mod_stats.models import ModStatsSnapshot
from app.trove.mods_hub.models import ModProject, ModRelease

logger = logging.getLogger("kiwi.mod_stats")

TROVESAURUS_CATALOG_URL = "https://trovesaurus.com/api/mods-all"
STEAM_DETAILS_URL = "https://api.steampowered.com/ISteamRemoteStorage/GetPublishedFileDetails/v1/"
HISTORY_DAYS = 365


def _int(value) -> int:
    try:
        return int(value or 0)
    except (TypeError, ValueError):
        return 0


def build_rows(
    projects: list[dict],
    trovesaurus_ids: dict[str, int],
    steam_ids: dict[str, int],
    catalog: list[dict],
    steam_details: list[dict],
) -> list[dict]:
    """Join the hub projects to each platform's counts by title. Pure / no network.

    A platform is ``None`` for a mod with no id there, or whose id the platform no
    longer returns (unpublished on Trovesaurus, private or removed on Steam)."""
    trovesaurus = {str(m.get("id")): m for m in catalog if isinstance(m, dict)}
    steam = {str(d.get("publishedfileid")): d for d in steam_details if d.get("result") == 1}
    rows = []
    for p in projects:
        title = p["title"]
        ts = trovesaurus.get(str(trovesaurus_ids.get(title)))
        st = steam.get(str(steam_ids.get(title)))
        rows.append({
            "title": title,
            "handle": p["owner_handle"],
            "slug": p["slug"],
            "hub": {
                "top_release": p.get("top_release", 0),
                "downloads_7d": p.get("downloads_7d", 0),
                "stars": p.get("star_count", 0),
            },
            "trovesaurus": ts and {
                "id": _int(ts.get("id")),
                "downloads": _int(ts.get("totaldownloads")),
                "likes": _int(ts.get("likes")),
            },
            "steam": st and {
                "id": _int(st.get("publishedfileid")),
                "subscriptions": _int(st.get("subscriptions")),
                "lifetime_subscriptions": _int(st.get("lifetime_subscriptions")),
                "favorites": _int(st.get("favorited")),
                "views": _int(st.get("views")),
            },
        })
    rows.sort(key=lambda r: r["title"].lower())
    return rows


def totals(rows: list[dict]) -> dict:
    """Per-platform sums for one snapshot."""
    def total(platform: str, field: str) -> int:
        return sum((r[platform] or {}).get(field, 0) for r in rows)

    return {
        "hub_top_release": total("hub", "top_release"),
        "trovesaurus_downloads": total("trovesaurus", "downloads"),
        "steam_subscriptions": total("steam", "subscriptions"),
        "steam_lifetime_subscriptions": total("steam", "lifetime_subscriptions"),
    }


async def _repo_ids(name: str) -> dict[str, int]:
    proc = await asyncio.create_subprocess_exec(
        "git", "--git-dir", settings.custom_art_remote, "show", f"master:{name}",
        stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE,
    )
    out, err = await proc.communicate()
    if proc.returncode:
        raise RuntimeError(f"git show {name}: {err.decode().strip()}")
    return json.loads(out)


async def _steam_details(ids: list[int]) -> list[dict]:
    if not ids:
        return []
    form = {"itemcount": len(ids)} | {f"publishedfileids[{i}]": v for i, v in enumerate(ids)}
    async with httpx.AsyncClient(timeout=30, headers={"User-Agent": KIWI_UA}) as client:
        resp = await client.post(STEAM_DETAILS_URL, data=form)
        resp.raise_for_status()
    return resp.json()["response"].get("publishedfiledetails", [])


async def collect() -> list[dict]:
    projects = await ModProject.find(
        {"owner_handle": settings.mod_stats_handle, "visibility": "public", "taken_down": False},
    ).to_list()
    top_rows = await ModRelease.aggregate([
        {"$match": {"project_id": {"$in": [p.id for p in projects]}, "status": "published"}},
        {"$group": {"_id": "$project_id", "top": {"$max": "$download_count"}}},
    ]).to_list()
    top = {r["_id"]: int(r["top"]) for r in top_rows}
    trovesaurus_ids = await _repo_ids("trovesaurus_ids.json")
    steam_ids = await _repo_ids("steam_ids.json")
    catalog = (await fetch(TROVESAURUS_CATALOG_URL, timeout=60)).json()
    titles = {p.title for p in projects}
    details = await _steam_details([v for k, v in steam_ids.items() if k in titles])
    return build_rows(
        [p.model_dump() | {"top_release": top.get(p.id, 0)} for p in projects],
        trovesaurus_ids, steam_ids,
        catalog if isinstance(catalog, list) else [], details,
    )


async def refresh() -> int:
    """Take today's snapshot unless it already exists. Returns the mods recorded."""
    day = utcnow().date().isoformat()
    if await ModStatsSnapshot.find_one(ModStatsSnapshot.day == day):
        return 0
    rows = await collect()
    await ModStatsSnapshot.get_pymongo_collection().update_one(
        {"day": day}, {"$set": {"taken_at": utcnow(), "mods": rows}}, upsert=True,
    )
    return len(rows)


async def public_stats() -> dict:
    snaps = await ModStatsSnapshot.find_all().sort(-ModStatsSnapshot.day).limit(HISTORY_DAYS).to_list()
    if not snaps:
        return {"updated_at": None, "mods": [], "totals": totals([]), "history": []}
    latest = snaps[0]
    return {
        "updated_at": iso(latest.taken_at),
        "mods": latest.mods,
        "totals": totals(latest.mods),
        "history": [{"day": s.day, **totals(s.mods)} for s in reversed(snaps)],
    }


_refresher = PeriodicRefresher(
    refresh,
    name="Mod statistics snapshot",
    delay=3600,
    log_result=lambda count: f"{count} mod(s)" if count else "today already taken",
)


def start_mod_stats_refresher() -> None:
    _refresher.start()


async def stop_mod_stats_refresher() -> None:
    await _refresher.stop()
