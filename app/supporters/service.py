"""Supporters business logic - the single source of truth for the routers."""
from beanie import PydanticObjectId
from pymongo.errors import DuplicateKeyError

from app.supporters.models import Supporter

# Seeded on first boot (empty collection) so the page/API are populated without
# manual setup; after that the master manages the list via the admin panel.
DEFAULT_SUPPORTERS = [
    "IINikstarII", "Nao373", "Wahoo", "boryzje", "nz", "Grainus", "Tues",
]


async def list_public() -> list[str]:
    """Supporter names in display (insertion) order - for the page + public API."""
    docs = await Supporter.find().sort("+created_at").to_list()
    return [d.name for d in docs]


async def admin_list() -> list[Supporter]:
    return await Supporter.find().sort("+created_at").to_list()


async def add(name: str, *, added_by: PydanticObjectId | None) -> Supporter:
    name = name.strip()
    if not name:
        raise ValueError("Supporter name cannot be empty")
    if await Supporter.find_one(Supporter.name == name) is not None:
        raise ValueError(f"Supporter '{name}' already exists")
    doc = Supporter(name=name, added_by=added_by)
    try:
        await doc.insert()
    except DuplicateKeyError:
        raise ValueError(f"Supporter '{name}' already exists")
    return doc


async def remove(name: str) -> bool:
    doc = await Supporter.find_one(Supporter.name == name.strip())
    if doc is None:
        return False
    await doc.delete()
    return True


async def replace(names: list[str], *, added_by: PydanticObjectId | None) -> dict:
    """Drop everything, then insert the de-duped, trimmed list. Refuses empty."""
    cleaned: list[str] = []
    seen: set[str] = set()
    for raw in names:
        n = (raw or "").strip()
        if n and n.lower() not in seen:
            seen.add(n.lower())
            cleaned.append(n)
    if not cleaned:
        raise ValueError("Provide at least one name (use DELETE to remove individually)")
    result = await Supporter.find_all().delete()
    removed = result.deleted_count if result else 0
    for n in cleaned:
        await Supporter(name=n, added_by=added_by).insert()
    return {"removed": removed, "added": len(cleaned)}


async def seed_supporters_if_empty() -> int:
    """Insert ``DEFAULT_SUPPORTERS`` on first boot only (empty collection)."""
    if await Supporter.find_one() is not None:
        return 0
    for name in DEFAULT_SUPPORTERS:
        await Supporter(name=name, added_by=None).insert()
    return len(DEFAULT_SUPPORTERS)
