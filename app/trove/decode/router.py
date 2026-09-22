"""Dev panel "Game data" tab: decoder status, and run one or all on demand."""
from __future__ import annotations

from fastapi import APIRouter, BackgroundTasks, Depends, Query

from app.core.dependencies import get_current_superuser
from app.core.errors import APIError, ErrorCode
from app.trove.decode import runner
from app.trove.decode.registry import DECODERS

router = APIRouter(prefix="/admin/gamedata", tags=["admin"],
                   dependencies=[Depends(get_current_superuser)])


@router.get("")
async def gamedata_status() -> dict:
    return await runner.status()


@router.post("/run")
async def gamedata_run(
    background_tasks: BackgroundTasks,
    name: str | None = Query(default=None, description="one decoder; all when omitted"),
    allow_shrink: bool = Query(default=False, description="accept an output smaller than the last one"),
) -> dict:
    if not runner.enabled():
        raise APIError(409, ErrorCode.conflict, "GAMEDATA_DIR isn't set on this server, so rebuilds are off.")
    if name is not None and name not in DECODERS:
        raise APIError(404, ErrorCode.not_found, f"No decoder called '{name}'.")
    if await runner.is_running():
        return {"started": False, "message": "A rebuild is already running."}
    names = [name] if name else list(DECODERS)
    background_tasks.add_task(runner.run, names, trigger="manual", allow_shrink=allow_shrink)
    return {"started": True, "message": f"Rebuilding {', '.join(names)}."}
