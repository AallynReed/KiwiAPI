import re

from pymongo.errors import DuplicateKeyError

from app.btt_blocklist.models import BlockKind, BttModBlock
from app.core.errors import APIError, ErrorCode
from app.core.utils import iso, to_oid

_VALID = {"author": re.compile(r"\d+"), "mod": re.compile(r"\d+"), "hash": re.compile(r"[0-9a-f]{32}"),
          "workshop": re.compile(r"\d+"), "workshop_author": re.compile(r"7656\d{13}")}
_LABEL = {"author": "author id", "mod": "mod id", "hash": "md5 hash", "workshop": "Steam Workshop item id",
          "workshop_author": "SteamID64"}


def parse_values(raw: str) -> list[str]:
    return list(dict.fromkeys(p.lower() for p in re.split(r"[\s,]+", raw or "") if p))


def entry_dto(e: BttModBlock) -> dict:
    return {"id": str(e.id), "kind": e.kind, "value": e.value, "note": e.note,
            "created_at": iso(e.created_at)}


async def list_entries() -> list[dict]:
    return [entry_dto(e) for e in await BttModBlock.find().sort("-created_at").to_list()]


async def add(kind: BlockKind, raw: str, note: str) -> list[dict]:
    """Add every value in ``raw`` (commas or whitespace). Values already listed are
    skipped; one malformed value rejects the whole batch."""
    values = parse_values(raw)
    if not values:
        raise APIError(status_code=400, code=ErrorCode.bad_request, message="Enter at least one value.")
    bad = [v for v in values if not _VALID[kind].fullmatch(v)]
    if bad:
        raise APIError(status_code=400, code=ErrorCode.bad_request,
                       message=f"Not a valid {_LABEL[kind]}: {', '.join(bad[:5])}")
    added = []
    for value in values:
        try:
            entry = await BttModBlock(kind=kind, value=value, note=note.strip()).insert()
        except DuplicateKeyError:
            continue
        added.append(entry_dto(entry))
    return added


async def remove(entry_id: str) -> None:
    entry = await BttModBlock.get(oid) if (oid := to_oid(entry_id)) else None
    if entry is None:
        raise APIError(status_code=404, code=ErrorCode.not_found, message="No such entry.")
    await entry.delete()


async def served() -> dict[str, list[str]]:
    out: dict[str, list[str]] = {"author": [], "mod": [], "hash": [], "workshop": [], "workshop_author": []}
    for entry in await BttModBlock.find().sort("created_at").to_list():
        out[entry.kind].append(entry.value)
    return out
