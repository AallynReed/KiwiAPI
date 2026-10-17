"""API side: hands the configured country list to the website container."""
from fastapi import APIRouter, Header

from app.admin import runtime_config
from app.core.errors import APIError, ErrorCode
from app.core.internal_api import internal_key_ok

router = APIRouter(prefix="/internal", include_in_schema=False)


def parse_countries(raw: str) -> list[str]:
    return sorted({c.strip().upper() for c in raw.split(",") if c.strip()})


@router.get("/geo-maintenance")
async def geo_maintenance(x_internal_key: str = Header(default="")) -> dict:
    if not internal_key_ok(x_internal_key):
        raise APIError(status_code=404, code=ErrorCode.not_found, message="Not found.")
    return {"countries": parse_countries(await runtime_config.get_setting("geo_maintenance_countries"))}
