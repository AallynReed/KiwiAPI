import logging

import httpx

from app.btt_blocklist import service as btt_blocklist
from app.core.config import settings
from app.core.http import KIWI_UA
from app.core.refresher import PeriodicRefresher
from app.core.utils import iso, utcnow
from app.steam_workshop.models import SteamWorkshopItem

logger = logging.getLogger("kiwi.steam_workshop")

APP_ID = 304050
QUERY_URL = "https://api.steampowered.com/IPublishedFileService/QueryFiles/v1/"
PLAYERS_URL = "https://api.steampowered.com/ISteamUser/GetPlayerSummaries/v2/"
PAGE = 100
MAX_PAGES = 200


def _int(value) -> int:
    try:
        return int(value or 0)
    except (TypeError, ValueError):
        return 0


def parse_items(details: list[dict]) -> list[dict]:
    """Steam's QueryFiles details -> item dicts, keeping only live public items."""
    items = []
    for d in details:
        if not isinstance(d, dict) or d.get("result", 1) != 1 or d.get("banned"):
            continue
        if _int(d.get("visibility")) != 0 or not d.get("publishedfileid"):
            continue
        items.append({
            "published_file_id": str(d["publishedfileid"]),
            "title": str(d.get("title") or "").strip(),
            "description": str(d.get("short_description") or d.get("file_description") or "").strip(),
            "creator": str(d.get("creator") or ""),
            "preview_url": str(d.get("preview_url") or ""),
            "file_size": _int(d.get("file_size")),
            "created": _int(d.get("time_created")),
            "updated": _int(d.get("time_updated")),
            "subscriptions": _int(d.get("subscriptions")),
            "favorites": _int(d.get("favorited")),
            "tags": [str(t.get("tag")) for t in d.get("tags") or () if isinstance(t, dict) and t.get("tag")],
        })
    return items


def visible(items: list[dict], blocked_items: set[str], blocked_authors: set[str]) -> list[dict]:
    return [i for i in items
            if i["published_file_id"] not in blocked_items and i["creator"] not in blocked_authors]


async def _query(client: httpx.AsyncClient) -> list[dict]:
    details, cursor = [], "*"
    for _ in range(MAX_PAGES):
        resp = await client.get(QUERY_URL, params={
            "key": settings.steam_web_api_key, "appid": APP_ID, "query_type": 1,
            "cursor": cursor, "numperpage": PAGE, "return_metadata": "true",
            "return_tags": "true", "return_short_description": "true",
        })
        resp.raise_for_status()
        body = resp.json().get("response") or {}
        page = body.get("publishedfiledetails") or []
        details += page
        nxt = body.get("next_cursor")
        if not page or not nxt or nxt == cursor:
            break
        cursor = nxt
    return details


async def _names(client: httpx.AsyncClient, steam_ids: list[str]) -> dict[str, str]:
    names = {}
    for n in range(0, len(steam_ids), 100):
        resp = await client.get(PLAYERS_URL, params={
            "key": settings.steam_web_api_key, "steamids": ",".join(steam_ids[n:n + 100])})
        resp.raise_for_status()
        for p in (resp.json().get("response") or {}).get("players") or ():
            names[str(p.get("steamid"))] = str(p.get("personaname") or "")
    return names


async def refresh() -> str:
    """Mirror the whole catalog. Items Steam no longer lists are dropped, but only
    after a sweep that came back with at least half of what is stored, so one bad
    answer cannot empty the tab."""
    if not settings.steam_web_api_key:
        return "skipped, no STEAM_WEB_API_KEY"
    async with httpx.AsyncClient(timeout=30, headers={"User-Agent": KIWI_UA}) as client:
        items = parse_items(await _query(client))
        names = await _names(client, sorted({i["creator"] for i in items if i["creator"]}))
    now = utcnow()
    coll = SteamWorkshopItem.get_pymongo_collection()
    for item in items:
        item["creator_name"] = names.get(item["creator"], "")
        await coll.update_one({"published_file_id": item["published_file_id"]},
                              {"$set": item | {"seen_at": now}}, upsert=True)
    stored = await SteamWorkshopItem.count()
    if items and len(items) * 2 >= stored:
        await SteamWorkshopItem.find({"seen_at": {"$lt": now}}).delete()
    return f"{len(items)} item(s)"


async def catalog() -> dict:
    served = await btt_blocklist.served()
    rows = await SteamWorkshopItem.find().sort("-subscriptions").to_list()
    newest = max((r.seen_at for r in rows), default=None)
    items = visible([r.model_dump(exclude={"id", "revision_id", "seen_at"}) for r in rows],
                    set(served["workshop"]), set(served["workshop_author"]))
    return {"items": items, "fetched_at": iso(newest) if newest else None}


_refresher = PeriodicRefresher(
    refresh,
    name="Steam Workshop catalog refresh",
    delay=lambda: settings.steam_workshop_refresh_seconds,
    log_result=lambda result: result,
)


def start_steam_workshop_refresher() -> None:
    _refresher.start()


async def stop_steam_workshop_refresher() -> None:
    await _refresher.stop()
