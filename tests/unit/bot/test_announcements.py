"""Announcement-type registry integrity + the announcer's sync/async embed glue."""
import asyncio

from app.bot.announcements import ANNOUNCEMENT_TYPES, TYPES_BY_KEY, catalog


def test_registry_keys_are_unique():
    keys = [t.key for t in ANNOUNCEMENT_TYPES]
    assert len(keys) == len(set(keys))


def test_types_by_key_covers_every_type():
    assert set(TYPES_BY_KEY) == {t.key for t in ANNOUNCEMENT_TYPES}
    assert all(TYPES_BY_KEY[t.key] is t for t in ANNOUNCEMENT_TYPES)


def test_catalog_shape_matches_registry():
    cat = catalog()
    assert [c["key"] for c in cat] == [t.key for t in ANNOUNCEMENT_TYPES]
    for c in cat:
        assert {"key", "label", "description", "category"} == set(c)
        assert c["category"] in {"Rotations", "Feeds"}
        assert c["label"] and c["description"]


def test_hourly_challenge_is_registered_with_a_stable_key():
    # The legacy migration writes announcements["hourly_challenge"]; the registry
    # MUST keep that exact key or migrated configs would orphan.
    assert "hourly_challenge" in TYPES_BY_KEY


def test_every_type_has_callable_embed_and_anchor():
    for t in ANNOUNCEMENT_TYPES:
        assert callable(t.build_embed)
        assert callable(t.current_anchor)


def test_build_embed_accepts_sync_and_async_builders():
    from app.bot.announcer import _build_embed

    def sync_builder():
        return {"title": "sync"}

    async def async_builder():
        return {"title": "async"}

    assert asyncio.run(_build_embed(sync_builder)) == {"title": "sync"}
    assert asyncio.run(_build_embed(async_builder)) == {"title": "async"}
