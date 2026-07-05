"""Giveaways business logic - the vault, giveaway lifecycle, entries, and views.

The worker (``app/giveaways/worker.py``) ticks ``run_due()`` to open scheduled
giveaways at ``starts_at`` and draw open ones at ``ends_at``, so ``status`` is the
source of truth for the public lists below.
"""
import logging
import secrets
from datetime import timedelta

from beanie import PydanticObjectId
from beanie.operators import In
from pymongo.errors import DuplicateKeyError

from app.core.email_outbox import queue_email
from app.core.errors import APIError, ErrorCode
from app.core.utils import utcnow
from app.giveaways.models import (
    CodeStatus,
    Giveaway,
    GiveawayEntry,
    GiveawayStatus,
    PrizeCode,
    VaultItem,
)
from app.giveaways.schemas import (
    EnterResponse,
    GiveawayAdminView,
    GiveawayCreate,
    GiveawayPublicView,
    GiveawayUpdate,
    MyGiveawayView,
    VaultCodesAdd,
    VaultCodeUpdate,
    VaultCodeView,
    VaultItemCreate,
    VaultItemUpdate,
    VaultItemView,
)
from app.site_auth.models import SiteUser

logger = logging.getLogger("kiwi.giveaways")


def _oid(value: str) -> PydanticObjectId:
    try:
        return PydanticObjectId(value)
    except Exception:
        raise APIError(400, ErrorCode.bad_request, "Invalid id")


async def _entry_count(gid: PydanticObjectId) -> int:
    return await GiveawayEntry.find(GiveawayEntry.giveaway_id == gid).count()


# ── Views ───────────────────────────────────────────────────────────────────

def _code_view(c: PrizeCode) -> VaultCodeView:
    return VaultCodeView(
        id=str(c.id), code=c.code, status=c.status,
        giveaway_id=str(c.giveaway_id) if c.giveaway_id else None,
        awarded_to_email=c.awarded_to_email, awarded_at=c.awarded_at,
        created_at=c.created_at,
    )


def _item_view(item: VaultItem, counts: dict[str, int]) -> VaultItemView:
    return VaultItemView(
        id=str(item.id), name=item.name, description=item.description,
        available=counts.get("available", 0), reserved=counts.get("reserved", 0),
        awarded=counts.get("awarded", 0), total=counts.get("total", 0),
        created_at=item.created_at,
    )


def _admin_view(g: Giveaway, item_name: str | None) -> GiveawayAdminView:
    return GiveawayAdminView(
        id=str(g.id), title=g.title, description=g.description,
        prize_name=g.prize_name, status=g.status,
        starts_at=g.starts_at, ends_at=g.ends_at, entry_count=g.entry_count,
        vault_item_id=str(g.vault_item_id) if g.vault_item_id else None,
        vault_item_name=item_name,
        prize_code_id=str(g.prize_code_id) if g.prize_code_id else None,
        winner_user_id=str(g.winner_user_id) if g.winner_user_id else None,
        winner_username=g.winner_username, winner_email=g.winner_email,
        drawn_at=g.drawn_at, created_at=g.created_at,
    )


def _public_view(g: Giveaway) -> GiveawayPublicView:
    return GiveawayPublicView(
        id=str(g.id), title=g.title, description=g.description,
        prize_name=g.prize_name, status=g.status,
        starts_at=g.starts_at, ends_at=g.ends_at, entry_count=g.entry_count,
        winner_username=g.winner_username,
    )


# ── Vault: items (drawers) + their codes ────────────────────────────────────

async def _code_counts(item_id: PydanticObjectId) -> dict[str, int]:
    out = {"available": 0, "reserved": 0, "awarded": 0, "total": 0}
    for c in await PrizeCode.find(PrizeCode.vault_item_id == item_id).to_list():
        out["total"] += 1
        out[c.status.value] = out.get(c.status.value, 0) + 1
    return out


async def list_items() -> list[VaultItemView]:
    items = await VaultItem.find().sort("-created_at").to_list()
    return [_item_view(it, await _code_counts(it.id)) for it in items]


async def create_item(req: VaultItemCreate) -> VaultItemView:
    item = VaultItem(name=req.name, description=req.description)
    await item.insert()
    return _item_view(item, {})


async def update_item(item_id: str, req: VaultItemUpdate) -> VaultItemView:
    item = await VaultItem.get(_oid(item_id))
    if item is None:
        raise APIError(404, ErrorCode.not_found, "Drawer not found")
    if req.name is not None:
        item.name = req.name
    if req.description is not None:
        item.description = req.description
    item.updated_at = utcnow()
    await item.save()
    return _item_view(item, await _code_counts(item.id))


async def delete_item(item_id: str) -> None:
    iid = _oid(item_id)
    item = await VaultItem.get(iid)
    if item is None:
        raise APIError(404, ErrorCode.not_found, "Drawer not found")
    in_use = await PrizeCode.find(
        PrizeCode.vault_item_id == iid, PrizeCode.status != CodeStatus.available,
    ).count()
    if in_use:
        raise APIError(
            400, ErrorCode.bad_request,
            "This drawer has codes reserved by or awarded through giveaways - cancel those first.",
        )
    await PrizeCode.find(PrizeCode.vault_item_id == iid).delete()  # drop its available codes
    await item.delete()


async def list_codes(item_id: str) -> list[VaultCodeView]:
    rows = await PrizeCode.find(
        PrizeCode.vault_item_id == _oid(item_id)
    ).sort("-created_at").to_list()
    return [_code_view(c) for c in rows]


async def add_codes(item_id: str, req: VaultCodesAdd) -> dict:
    """Bulk-drop codes into a drawer. Strips blanks, caps length, and de-dupes
    both within the batch and against codes already in the drawer."""
    iid = _oid(item_id)
    if await VaultItem.get(iid) is None:
        raise APIError(404, ErrorCode.not_found, "Drawer not found")
    seen: set[str] = set()
    cleaned: list[str] = []
    for raw in req.codes:
        s = (raw or "").strip()
        if not s or len(s) > 400 or s in seen:
            continue
        seen.add(s)
        cleaned.append(s)
    if not cleaned:
        raise APIError(400, ErrorCode.bad_request, "No valid codes to add.")
    existing = {
        c.code for c in await PrizeCode.find(PrizeCode.vault_item_id == iid).to_list()
    }
    docs = [PrizeCode(vault_item_id=iid, code=s) for s in cleaned if s not in existing]
    if docs:
        await PrizeCode.insert_many(docs)
    return {"added": len(docs), "skipped": len(cleaned) - len(docs)}


async def update_code(code_id: str, req: VaultCodeUpdate) -> VaultCodeView:
    code = await PrizeCode.get(_oid(code_id))
    if code is None:
        raise APIError(404, ErrorCode.not_found, "Code not found")
    if code.status == CodeStatus.awarded:
        raise APIError(400, ErrorCode.bad_request, "An awarded code can't be edited.")
    code.code = req.code
    await code.save()
    return _code_view(code)


async def delete_code(code_id: str) -> None:
    code = await PrizeCode.get(_oid(code_id))
    if code is None:
        raise APIError(404, ErrorCode.not_found, "Code not found")
    if code.status != CodeStatus.available:
        raise APIError(
            400, ErrorCode.bad_request,
            "This code is reserved by or awarded through a giveaway - cancel that first.",
        )
    await code.delete()


# ── Giveaways (admin) ───────────────────────────────────────────────────────

async def _item_name(item_id: PydanticObjectId | None) -> str | None:
    if item_id is None:
        return None
    it = await VaultItem.get(item_id)
    return it.name if it else None


async def list_admin() -> list[GiveawayAdminView]:
    rows = await Giveaway.find().sort("-created_at").to_list()
    out: list[GiveawayAdminView] = []
    for g in rows:
        out.append(_admin_view(g, await _item_name(g.vault_item_id)))
    return out


async def _reserve_from_item(item_id: PydanticObjectId, giveaway_id: PydanticObjectId) -> PrizeCode:
    """Reserve one available code from a drawer for a giveaway."""
    code = await PrizeCode.find_one(
        PrizeCode.vault_item_id == item_id,
        PrizeCode.status == CodeStatus.available,
    )
    if code is None:
        raise APIError(400, ErrorCode.bad_request, "That drawer has no available codes - add some first.")
    code.status = CodeStatus.reserved
    code.giveaway_id = giveaway_id
    await code.save()
    return code


async def _release_code(code_id: PydanticObjectId | None) -> None:
    if code_id is None:
        return
    code = await PrizeCode.get(code_id)
    if code and code.status == CodeStatus.reserved:
        code.status = CodeStatus.available
        code.giveaway_id = None
        await code.save()


async def create(req: GiveawayCreate) -> GiveawayAdminView:
    now = utcnow()
    if req.ends_at <= req.starts_at:
        raise APIError(400, ErrorCode.bad_request, "End must be after start.")
    if req.ends_at <= now:
        raise APIError(400, ErrorCode.bad_request, "End date is in the past.")
    item = await VaultItem.get(_oid(req.vault_item_id))
    if item is None:
        raise APIError(404, ErrorCode.not_found, "Drawer not found")
    # Require an available code before committing (so we never strand a giveaway
    # with no prize).
    if await PrizeCode.find(
        PrizeCode.vault_item_id == item.id, PrizeCode.status == CodeStatus.available,
    ).count() < 1:
        raise APIError(400, ErrorCode.bad_request, "That drawer has no available codes - add some first.")
    g = Giveaway(
        title=req.title,
        # Prize name + description come from the drawer (written once); the admin
        # can still override the description per-giveaway by sending one.
        description=req.description if req.description is not None else item.description,
        prize_name=item.name,
        vault_item_id=item.id,
        starts_at=req.starts_at, ends_at=req.ends_at,
        status=GiveawayStatus.open if req.starts_at <= now else GiveawayStatus.scheduled,
    )
    await g.insert()
    code = await _reserve_from_item(item.id, g.id)
    g.prize_code_id = code.id
    await g.save()
    return _admin_view(g, item.name)


async def update(giveaway_id: str, req: GiveawayUpdate) -> GiveawayAdminView:
    g = await Giveaway.get(_oid(giveaway_id))
    if g is None:
        raise APIError(404, ErrorCode.not_found, "Giveaway not found")
    if g.status in (GiveawayStatus.drawn, GiveawayStatus.closed, GiveawayStatus.cancelled):
        raise APIError(400, ErrorCode.bad_request, "A finished giveaway can't be edited.")
    if req.title is not None:
        g.title = req.title
    if req.description is not None:
        g.description = req.description
    if req.starts_at is not None:
        g.starts_at = req.starts_at
    if req.ends_at is not None:
        g.ends_at = req.ends_at
    if g.ends_at <= g.starts_at:
        raise APIError(400, ErrorCode.bad_request, "End must be after start.")
    # Switch the prize drawer: release the old reserved code, reserve one from
    # the new drawer, and re-snapshot the prize name.
    if req.vault_item_id is not None and req.vault_item_id != (str(g.vault_item_id) if g.vault_item_id else None):
        item = await VaultItem.get(_oid(req.vault_item_id))
        if item is None:
            raise APIError(404, ErrorCode.not_found, "Drawer not found")
        await _release_code(g.prize_code_id)
        code = await _reserve_from_item(item.id, g.id)
        g.vault_item_id = item.id
        g.prize_code_id = code.id
        g.prize_name = item.name
    # Re-derive scheduled/open from the (possibly new) start date.
    now = utcnow()
    if g.status in (GiveawayStatus.scheduled, GiveawayStatus.open):
        g.status = GiveawayStatus.open if g.starts_at <= now < g.ends_at else (
            GiveawayStatus.scheduled if g.starts_at > now else g.status
        )
    g.updated_at = now
    await g.save()
    return _admin_view(g, await _item_name(g.vault_item_id))


async def cancel(giveaway_id: str) -> GiveawayAdminView:
    g = await Giveaway.get(_oid(giveaway_id))
    if g is None:
        raise APIError(404, ErrorCode.not_found, "Giveaway not found")
    if g.status in (GiveawayStatus.drawn, GiveawayStatus.closed):
        raise APIError(400, ErrorCode.bad_request, "This giveaway has already finished.")
    await _release_code(g.prize_code_id)
    g.status = GiveawayStatus.cancelled
    g.updated_at = utcnow()
    await g.save()
    return _admin_view(g, await _item_name(g.vault_item_id))


async def draw_now(giveaway_id: str) -> GiveawayAdminView:
    """Force an immediate draw (admin override) regardless of the end date."""
    g = await Giveaway.get(_oid(giveaway_id))
    if g is None:
        raise APIError(404, ErrorCode.not_found, "Giveaway not found")
    if g.status not in (GiveawayStatus.open, GiveawayStatus.scheduled):
        raise APIError(400, ErrorCode.bad_request, "Only an open/scheduled giveaway can be drawn.")
    await _draw(g)
    return _admin_view(g, await _item_name(g.vault_item_id))


# ── Public + entries ────────────────────────────────────────────────────────

async def list_public() -> list[GiveawayPublicView]:
    rows = (
        await Giveaway.find(Giveaway.status != GiveawayStatus.cancelled)
        .sort("-starts_at").limit(50).to_list()
    )
    return [_public_view(g) for g in rows]


async def list_ongoing() -> list[GiveawayPublicView]:
    """Open giveaways (accepting entries), soonest to end first."""
    rows = (
        await Giveaway.find(Giveaway.status == GiveawayStatus.open)
        .sort("+ends_at").to_list()
    )
    return [_public_view(g) for g in rows]


async def list_upcoming() -> list[GiveawayPublicView]:
    """Scheduled giveaways not yet open, soonest-starting first."""
    rows = (
        await Giveaway.find(Giveaway.status == GiveawayStatus.scheduled)
        .sort("+starts_at").to_list()
    )
    return [_public_view(g) for g in rows]


async def list_ended(days: int = 7) -> list[GiveawayPublicView]:
    """Giveaways ended in the last ``days`` days, most-recently-ended first.
    Ended = ``drawn`` (had a winner) or ``closed`` (no entrants); ``cancelled``
    is excluded (didn't run to completion)."""
    cutoff = utcnow() - timedelta(days=max(1, days))
    rows = (
        await Giveaway.find(
            In(Giveaway.status, [GiveawayStatus.drawn, GiveawayStatus.closed]),
            Giveaway.ends_at >= cutoff,
        ).sort("-ends_at").to_list()
    )
    return [_public_view(g) for g in rows]


async def my_entry_ids(user: SiteUser) -> list[str]:
    rows = await GiveawayEntry.find(GiveawayEntry.site_user_id == user.id).to_list()
    return [str(r.giveaway_id) for r in rows]


async def my_participations(user: SiteUser) -> list[MyGiveawayView]:
    """Every giveaway the user entered, with the prize code attached to the ones
    they won (so the dashboard can show it any time). Won first, then newest."""
    entries = await GiveawayEntry.find(GiveawayEntry.site_user_id == user.id).to_list()
    if not entries:
        return []
    gids = list({e.giveaway_id for e in entries})
    giveaways = {g.id: g for g in await Giveaway.find({"_id": {"$in": gids}}).to_list()}
    # Codes awarded to THIS user, keyed by giveaway - only ever their own.
    won_codes = {
        c.giveaway_id: c.code
        for c in await PrizeCode.find(PrizeCode.awarded_to == user.id).to_list()
    }
    out: list[MyGiveawayView] = []
    for e in entries:
        g = giveaways.get(e.giveaway_id)
        if g is None:
            continue
        won = g.status == GiveawayStatus.drawn and g.winner_user_id == user.id
        out.append(MyGiveawayView(
            giveaway_id=str(g.id), title=g.title, prize_name=g.prize_name,
            status=g.status, starts_at=g.starts_at, ends_at=g.ends_at,
            entered_at=e.entered_at, won=won,
            code=won_codes.get(g.id) if won else None,
        ))
    out.sort(key=lambda x: (x.won, x.ends_at), reverse=True)
    return out


async def enter(user: SiteUser, giveaway_id: str) -> EnterResponse:
    gid = _oid(giveaway_id)
    g = await Giveaway.get(gid)
    if g is None:
        raise APIError(404, ErrorCode.not_found, "Giveaway not found")
    now = utcnow()
    if not (g.status == GiveawayStatus.open and g.starts_at <= now < g.ends_at):
        raise APIError(400, ErrorCode.bad_request, "This giveaway isn't open for entries.")
    entry = GiveawayEntry(
        giveaway_id=gid, site_user_id=user.id,
        username=user.display_name or user.username,
    )
    try:
        await entry.insert()
    except DuplicateKeyError:
        return EnterResponse(giveaway_id=giveaway_id, entered=True, entry_count=await _entry_count(gid))
    count = await _entry_count(gid)
    g.entry_count = count
    g.updated_at = now
    await g.save()
    return EnterResponse(giveaway_id=giveaway_id, entered=True, entry_count=count)


# ── Draw + winner email ─────────────────────────────────────────────────────

async def run_due() -> None:
    """Worker tick: open scheduled giveaways whose start passed, draw open ones
    whose end passed. Idempotent + resilient - one bad giveaway can't stall the
    rest."""
    now = utcnow()
    starting = await Giveaway.find(
        Giveaway.status == GiveawayStatus.scheduled, Giveaway.starts_at <= now,
    ).to_list()
    for g in starting:
        g.status = GiveawayStatus.open
        g.updated_at = now
        await g.save()

    ending = await Giveaway.find(
        Giveaway.status == GiveawayStatus.open, Giveaway.ends_at <= now,
    ).to_list()
    for g in ending:
        try:
            await _draw(g)
        except Exception:
            logger.warning("draw failed for giveaway %s", g.id, exc_info=True)


async def _draw(g: Giveaway) -> None:
    """Pick a uniformly-random entrant, award + email the reserved code, and
    mark the giveaway drawn. No entrants -> close it and free the code."""
    entries = await GiveawayEntry.find(GiveawayEntry.giveaway_id == g.id).to_list()
    now = utcnow()

    if not entries:
        g.status = GiveawayStatus.closed
        g.updated_at = now
        await g.save()
        await _release_code(g.prize_code_id)
        logger.info("Giveaway %s closed - no entrants", g.id)
        return

    winner = secrets.choice(entries)
    su = await SiteUser.get(winner.site_user_id)
    winner_email = su.email if su else None

    code = await PrizeCode.get(g.prize_code_id) if g.prize_code_id else None

    g.status = GiveawayStatus.drawn
    g.winner_user_id = winner.site_user_id
    g.winner_username = winner.username
    g.winner_email = winner_email
    g.drawn_at = now
    g.updated_at = now
    await g.save()

    if code and code.status == CodeStatus.reserved:
        code.status = CodeStatus.awarded
        code.awarded_to = winner.site_user_id
        code.awarded_to_email = winner_email
        code.awarded_at = now
        await code.save()

    if winner_email and code:
        try:
            await _email_winner(winner_email, g, code)
        except Exception:
            logger.warning("winner email failed for giveaway %s", g.id, exc_info=True)

    logger.info("Giveaway %s drawn - winner=%s", g.id, winner.username)


async def _email_winner(to: str, g: Giveaway, code: PrizeCode) -> None:
    subject = f"You won: {g.prize_name}!"
    text_lines = [
        f'Congratulations - you won "{g.title}" on Better Trove Tools!',
        "",
        f"Prize: {g.prize_name}",
        f"Your code: {code.code}",
    ]
    if g.description:
        text_lines += ["", g.description]
    text_lines += ["", "Thanks for playing.", "- Better Trove Tools"]
    text = "\n".join(text_lines)

    desc_html = f"<p style='color:#9aa4b2'>{g.description}</p>" if g.description else ""
    html = (
        "<div style=\"font-family:system-ui,sans-serif;max-width:480px;margin:0 auto;"
        "padding:24px;color:#e8ecf3;background:#0a0e14\">"
        f"<h1 style='font-size:1.4rem;margin:0 0 12px'>You won {g.prize_name}! 🎉</h1>"
        f"<p>Congratulations - you won <strong>{g.title}</strong> on Better Trove Tools.</p>"
        "<p style='margin:18px 0 6px'>Your code:</p>"
        f"<p style='font-family:monospace;font-size:1.1rem;background:#161b22;"
        f"border:1px solid #232a33;border-radius:8px;padding:12px 14px'>{code.code}</p>"
        f"{desc_html}"
        "<p style='margin-top:20px;color:#9aa4b2'>Thanks for playing.<br>- Better Trove Tools</p>"
        "</div>"
    )
    await queue_email(to, subject, text, html)
