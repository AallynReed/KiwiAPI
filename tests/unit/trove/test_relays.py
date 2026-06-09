from app.trove.relays import _normalize_twitch, _normalize_videos


def test_normalize_twitch_resolves_thumbnail_and_url():
    raw = [{
        "user_name": "CoolStreamer", "user_login": "coolstreamer", "title": "Trove time!",
        "viewer_count": 42, "game_name": "Trove", "started_at": "2024-01-01T00:00:00Z",
        "thumbnail_url": "https://x.tv/live_user_coolstreamer-{width}x{height}.jpg",
    }]
    out = _normalize_twitch(raw)
    assert len(out) == 1
    s = out[0]
    assert s["channel"] == "CoolStreamer" and s["login"] == "coolstreamer"
    assert s["url"] == "https://twitch.tv/coolstreamer"
    assert s["viewers"] == 42 and s["game"] == "Trove"
    assert s["thumbnail"] == "https://x.tv/live_user_coolstreamer-440x248.jpg"


def test_normalize_twitch_accepts_data_wrapper():
    out = _normalize_twitch({"data": [{"user_login": "a", "title": "t", "viewer_count": 1}]})
    assert out[0]["login"] == "a" and out[0]["viewers"] == 1


def test_normalize_videos_passthrough_and_skip():
    raw = [
        {"title": "V", "url": "https://yt/1", "channel": "Ch", "video_id": "1",
         "published_at": "2024-01-01T00:00:00Z", "thumbnail_url": "https://t/1.jpg"},
        {"title": "no url"},  # skipped - no url
    ]
    out = _normalize_videos(raw)
    assert len(out) == 1
    assert out[0]["url"] == "https://yt/1" and out[0]["video_id"] == "1"
