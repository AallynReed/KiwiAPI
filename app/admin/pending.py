"""Review-queue counts for the dev-portal sidebar badges.

Each admin-panel tab that holds a *queue* (something waiting on a master decision)
reports how many items are waiting, so the sidebar can show a count instead of the
master having to open every module to find out. Everything here is a
``count_documents`` over an indexed field - cheap enough to poll on every tab
switch.

Adding a queue: append an entry to ``_SOURCES`` keyed by the portal tab id. The
label is what the sidebar tooltip shows, so write it as a plural noun phrase.
"""
from collections.abc import Awaitable, Callable
from typing import NamedTuple


class _Source(NamedTuple):
    tab: str                              # portal tab id (TABS in portal/html/app.js)
    label: str                            # tooltip wording, e.g. "2 requests to review"
    count: Callable[[], Awaitable[int]]


async def _art(status: str) -> int:
    from app.custom_art.models import ArtRequest
    return await ArtRequest.find(ArtRequest.status == status).count()


async def _art_steam() -> int:
    from app.custom_art.models import ArtRequest
    return await ArtRequest.find(ArtRequest.steam_pending == True).count()  # noqa: E712


async def _trove_claims() -> int:
    from app.site_auth.models import SiteUser
    return await SiteUser.find({"claimed_trove_name": {"$ne": None},
                                "claim_verified": False}).count()


async def _username_requests() -> int:
    from app.site_auth.models import UsernameChangeRequest
    return await UsernameChangeRequest.find(
        UsernameChangeRequest.status == "pending").count()


async def _mod_reports() -> int:
    from app.trove.mods_hub.models import ContentReport
    return await ContentReport.find(ContentReport.resolved == False).count()  # noqa: E712


async def _stray_mods() -> int:
    from app.trove.mods_hub.models import ModProject
    return await ModProject.find(ModProject.is_stray == True,  # noqa: E712
                                 ModProject.stray_status == "pending").count()


async def _mod_claims() -> int:
    from app.trove.mods_hub.models import ModClaimRequest
    return await ModClaimRequest.find(ModClaimRequest.status == "pending").count()


_SOURCES: tuple[_Source, ...] = (
    _Source("customart", "requests to review", lambda: _art("pending")),
    _Source("customart", "approved, waiting on a release", lambda: _art("approved")),
    _Source("customart", "failed builds to retry", lambda: _art("failed")),
    _Source("customart", "waiting on a Steam push", _art_steam),
    _Source("claims", "name claims to verify", _trove_claims),
    _Source("claims", "username change requests", _username_requests),
    _Source("mods", "open content reports", _mod_reports),
    _Source("mods", "stray mods awaiting approval", _stray_mods),
    _Source("mods", "mod claim requests", _mod_claims),
)


async def pending_counts() -> dict:
    """``{"counts": {tab: total}, "detail": {tab: [{label, count}, ...]}}``.

    Only non-zero entries are reported, so the portal can badge a tab on a plain
    truthy check and the tooltip lists just what is actually waiting. A queue that
    errors is skipped rather than failing the whole sidebar.
    """
    counts: dict[str, int] = {}
    detail: dict[str, list[dict]] = {}
    for source in _SOURCES:
        try:
            n = await source.count()
        except Exception:
            continue
        if n <= 0:
            continue
        counts[source.tab] = counts.get(source.tab, 0) + n
        detail.setdefault(source.tab, []).append({"label": source.label, "count": n})
    return {"counts": counts, "detail": detail}
