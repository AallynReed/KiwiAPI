"""The request form's data plane: ``/site/custom-art``.

Tokenless and login-free - anyone can ask. The page itself is rendered by the
website container (``app/web/pages.py`` -> ``/custom-art``).
"""
from typing import Literal

from fastapi import APIRouter, File, Form, Request, Response, UploadFile

from app.admin import runtime_config
from app.core.config import settings
from app.core.ratelimit import check_rate_limit
from app.core.utils import client_ip
from app.custom_art import service

router = APIRouter(tags=["custom-art"], include_in_schema=False)


@router.get("/site/custom-art/config")
async def form_config(response: Response) -> dict:
    response.headers["Cache-Control"] = "no-store"
    return {
        "captcha_provider": settings.captcha_provider,
        "captcha_sitekey": settings.captcha_sitekey,
        "max_bytes": settings.mods_image_max_bytes,
    }


@router.post("/site/custom-art", status_code=202)
async def submit_request(
    request: Request,
    kind: Literal["pfp", "club", "banner"] = Form(...),
    name: str = Form(..., max_length=200),
    email: str = Form(..., max_length=254),
    note: str | None = Form(default=None, max_length=500),
    captcha_token: str | None = Form(default=None, max_length=4096),
    file: UploadFile = File(...),
) -> dict:
    ip = client_ip(request)
    max_, window = await runtime_config.get_rate_limit("art_request_rate_limit")
    await check_rate_limit(f"custom-art:{ip or 'unknown'}", max_, window)
    data = await file.read(settings.mods_image_max_bytes + 1)
    await service.submit(kind=kind, name=name, email=email, note=note, data=data,
                         captcha_token=captcha_token, ip=ip)
    return {"status": "received"}
