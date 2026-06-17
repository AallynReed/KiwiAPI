from app.core.scopes import (
    ALL_SCOPES,
    SCOPE_BITS,
    catalog,
    decode,
    is_valid_mask,
    mask_grants,
)

# Scopes by function: rotations(1) feeds(2) stats(4) gems(8) misc(16) mods(32)
# updates(64) codexes(128) btt(256) leaderboards(512) market(1024) activity(2048)
# giveaways(4096) events(8192) ocr(16384).


def test_scopes_registered():
    assert SCOPE_BITS == {
        "rotations:read": 1, "feeds:read": 2, "stats:read": 4,
        "gems:read": 8, "misc:read": 16, "mods:read": 32, "updates:read": 64,
        "codexes:read": 128, "btt:read": 256, "leaderboards:read": 512,
        "market:read": 1024, "activity:read": 2048, "giveaways:read": 4096,
        "events:read": 8192, "ocr:read": 16384,
    }
    assert ALL_SCOPES == 0
    assert {c["resource"] for c in catalog()} == {
        "rotations", "feeds", "stats", "gems", "misc", "mods", "updates", "codexes", "btt",
        "leaderboards", "market", "activity", "giveaways", "events", "ocr",
    }
    assert all(":" in c["key"] for c in catalog())  # naming convention


def test_mask_grants():
    assert mask_grants(0, "rotations:read") and mask_grants(0, "updates:read")   # 0 = all
    assert mask_grants(1, "rotations:read") and not mask_grants(1, "feeds:read")
    assert mask_grants(32, "mods:read") and not mask_grants(32, "misc:read")
    assert mask_grants(64, "updates:read") and not mask_grants(64, "mods:read")
    assert mask_grants(128, "codexes:read") and not mask_grants(128, "updates:read")
    assert mask_grants(256, "btt:read") and not mask_grants(256, "codexes:read")
    assert mask_grants(512, "leaderboards:read") and not mask_grants(512, "btt:read")
    assert mask_grants(1024, "market:read") and not mask_grants(1024, "leaderboards:read")
    assert mask_grants(2048, "activity:read") and not mask_grants(2048, "market:read")
    assert mask_grants(4096, "giveaways:read") and not mask_grants(4096, "activity:read")
    assert mask_grants(8192, "events:read") and not mask_grants(8192, "giveaways:read")
    assert mask_grants(16384, "ocr:read") and not mask_grants(16384, "events:read")
    assert mask_grants(32767, "rotations:read") and mask_grants(32767, "ocr:read")  # 1|…|16384


def test_is_valid_mask():
    assert all(is_valid_mask(m) for m in (0, 1, 2, 4, 8, 16, 32, 63, 64, 127, 128, 255, 256, 511, 512, 1023, 1024, 2047, 2048, 4095, 4096, 8191, 8192, 16383, 16384, 32767))
    assert not is_valid_mask(32768)  # bit 15 unassigned


def test_decode():
    assert decode(0) == []
    assert decode(1) == ["rotations:read"]
    assert decode(64) == ["updates:read"]
    assert decode(128) == ["codexes:read"]
    assert decode(256) == ["btt:read"]
    assert decode(512) == ["leaderboards:read"]
    assert decode(1024) == ["market:read"]
    assert sorted(decode(2047)) == [
        "btt:read", "codexes:read", "feeds:read", "gems:read", "leaderboards:read",
        "market:read", "misc:read", "mods:read", "rotations:read", "stats:read", "updates:read",
    ]
