from datetime import datetime, timezone

from app.trove import rotations

UTC = timezone.utc
NOW = datetime(2025, 6, 1, 12, 0, tzinfo=UTC)


def _check_feed(feed, biome_count):
    assert feed["current"] is not None
    cur = feed["current"]
    assert cur["ends_at"] > cur["starts_at"]
    assert len(cur["biomes"]) == biome_count
    for b in cur["biomes"]:
        assert set(b) == {"name", "final_name", "icon"}
    assert isinstance(feed["upcoming"], list) and feed["upcoming"]


def test_biome_rotation_d15():
    feed = rotations.biome_rotation(NOW, count=4)
    _check_feed(feed, 3)
    # d15 windows are 3 hours apart.
    assert feed["upcoming"][0]["starts_at"] - feed["current"]["starts_at"] == 3 * 3600
    assert len(feed["upcoming"]) == 4


def test_wild_mana():
    feed = rotations.wild_mana(NOW, count=5)
    _check_feed(feed, 3)
    assert feed["upcoming"][0]["starts_at"] - feed["current"]["starts_at"] == 7 * 86400


def test_stampy():
    feed = rotations.stampy(NOW, count=5)
    _check_feed(feed, 1)
    cur = feed["current"]
    assert cur["ends_at"] - cur["starts_at"] == 48 * 3600  # 48-hour window
