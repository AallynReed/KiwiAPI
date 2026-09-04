"""The uploader's surface: ``/site/drops/*``.

Tokenless and login-free by design - the whole point is that the person on the
other end has a link and a PIN and nothing else. It lives on the data plane (the
API container) like every other ``/site/*`` endpoint; the page itself is
rendered by the website container (``app/web/pages.py`` -> ``/drop/{slug}``).

Nothing here is in the OpenAPI schema: this is a private tool, not part of the
public API.
"""
from fastapi import APIRouter, File, Form, Request, Response, UploadFile

from app.admin import runtime_config
from app.core.ratelimit import check_rate_limit
from app.core.utils import client_ip
from app.drops import service
from app.drops.schemas import DropPublicView, DropUploadView

router = APIRouter(tags=["drops"], include_in_schema=False)


async def _throttle(request: Request, slug: str) -> None:
    """Two buckets, because the PIN is short by design.

    Per-IP-per-link stops one person guessing; per-link stops many people (or one
    person on many addresses) grinding the same PIN between them. Both are tuned
    by ``file_drop_rate_limit_*`` - the per-link budget is the per-IP one times
    four, so a handful of honest friends sharing a link never collide with it."""
    max_, window = await runtime_config.get_rate_limit("file_drop_rate_limit")
    await check_rate_limit(f"drop:{slug}:{client_ip(request) or 'unknown'}", max_, window)
    await check_rate_limit(f"drop:{slug}", max_ * 4, window)


@router.get("/site/drops/{slug}")
async def drop_meta(slug: str, request: Request, response: Response) -> DropPublicView:
    """What the page needs before the PIN is entered: what the file is for, how
    big it may be, how many uploads are left and when the link dies. Says nothing
    about who made it. An inactive link 404s.

    Never cached: "one upload left" and "still alive" are answers that go stale
    the moment somebody uses the link, and the edge sits in front of this."""
    await _throttle(request, slug)
    response.headers["Cache-Control"] = "no-store"
    return service.public_view(await service.by_slug(slug))


@router.post("/site/drops/{slug}/verify")
async def drop_verify(slug: str, request: Request, pin: str = Form(...)) -> DropPublicView:
    """Check the PIN before a file is sent.

    Purely so a wrong PIN costs a keystroke rather than a finished upload - the
    upload endpoint checks the PIN again itself, and this grants nothing."""
    await _throttle(request, slug)
    drop = await service.by_slug(slug)
    service.check_pin(drop, pin)
    return service.public_view(drop)


@router.post("/site/drops/{slug}/upload")
async def drop_upload(
    slug: str,
    request: Request,
    pin: str = Form(...),
    file: UploadFile = File(...),
    note: str | None = Form(default=None),
) -> DropUploadView:
    """Send the file. The PIN rides along with it (there is no session here), and
    the drop's upload budget is spent the moment this is accepted."""
    await _throttle(request, slug)
    drop = await service.by_slug(slug)
    service.check_pin(drop, pin)
    return await service.receive(drop, file, note)
