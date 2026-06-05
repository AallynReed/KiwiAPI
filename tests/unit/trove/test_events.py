from app.trove.events import parse_events


def test_parse_events_normalizes():
    raw = [
        {"id": "986", "name": "Playful Kite", "url": "https://trovesaurus.com/giveaway=397",
         "startdate": "1780066800", "enddate": "1780671600", "category": "Giveaway",
         "image": "", "icon": "https://t/icon.png", "lookup": "giveaway=397"},
        {"id": "", "name": "no id — skipped"},
        {"id": "5", "name": "bad dates — skipped", "startdate": "x", "enddate": "y"},
    ]
    out = parse_events(raw)
    assert len(out) == 1  # only the valid one survives
    e = out[0]
    assert e["event_id"] == "986"
    assert e["starts_at"] == 1780066800 and e["ends_at"] == 1780671600
    assert e["category"] == "Giveaway"
    assert e["image"] is None          # empty string normalized to None
    assert e["icon"] == "https://t/icon.png"
    assert e["lookup"] == "giveaway=397"


def test_parse_events_defaults():
    out = parse_events([{"id": "1", "startdate": "10", "enddate": "20"}])
    assert len(out) == 1
    assert out[0]["name"] == "Untitled" and out[0]["category"] == "Event" and out[0]["url"] == ""
    assert out[0]["image"] is None and out[0]["icon"] is None and out[0]["lookup"] is None


def test_parse_events_handles_non_string_id():
    # Trovesaurus sends string ids, but be defensive about ints too.
    out = parse_events([{"id": 42, "startdate": "10", "enddate": "20", "name": "X"}])
    assert out[0]["event_id"] == "42"
