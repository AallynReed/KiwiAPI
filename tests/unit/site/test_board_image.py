"""Live "Trove Now" board image: the Pillow layout must always produce a valid PNG,
including with empty / unknown data, and the relative-time helper must read right."""
import time

from app.site import og_image

_PNG_MAGIC = b"\x89PNG\r\n\x1a\n"


def _data(**over):
    base = {
        "challenge": {"name": "Cursed Vale", "active": True,
                      "starts_at": 1781280000, "ends_at": 1781281200},
        "chaos": {"item": {"name": "Radiant Sovereign"}, "ends_at": 1781800000},
        "biomes": {"biomes": [{"final_name": "Sundered Uplands"},
                              {"final_name": "Cursed Vale"}], "ends_at": 1781290800},
        "corruxion": {"active": True, "starts_at": 1781200000, "ends_at": 1781460000},
        "fluxion": {"active": False, "starts_at": 1781500000, "ends_at": 1781760000, "state": "away"},
        "server": {"daily_reset_at": 1781298000, "weekly_reset_at": 1781730000},
        "status": {"overall": "online"},
    }
    base.update(over)
    return base


def test_board_renders_a_valid_png():
    png = og_image._draw_board(_data())
    assert png[:8] == _PNG_MAGIC and len(png) > 1000


def test_board_handles_empty_and_unknown_without_crashing():
    png = og_image._draw_board({
        "challenge": {}, "chaos": {}, "biomes": {},
        "corruxion": {}, "fluxion": {},
        "server": {"daily_reset_at": 1781298000, "weekly_reset_at": 1781730000},
        "status": {"overall": "unknown"},
    })
    assert png[:8] == _PNG_MAGIC


def test_rel_formats_future_durations():
    now = int(time.time())
    assert og_image._rel(now + 90) == "in 1m"
    assert og_image._rel(now + 3700).startswith("in 1h")
    assert og_image._rel(now + 90000).startswith("in 1d")
    assert og_image._rel(now - 5) == "now"
    assert og_image._rel(None) == "—"


# ── per-announcement card ────────────────────────────────────────────────────

def test_announcement_card_renders_png():
    png = og_image._draw_announcement("Corruxion Merchant", (155, 93, 229),
                                      ["Here now", "Leaves in 1d 15h"])
    assert png[:8] == _PNG_MAGIC and len(png) > 1000


def test_announcement_content_for_computed_kinds():
    import asyncio

    # kinds that don't need Mongo (rotations/merchants/server_time/status cache)
    for kind in ("corruxion", "fluxion", "longshade", "wild_mana", "stampy",
                 "daily_bonuses", "server_status"):
        title, accent, lines = asyncio.run(og_image._announcement_content(kind))
        assert title and isinstance(accent, tuple) and len(accent) == 3
        assert lines and all(isinstance(ln, str) for ln in lines)
