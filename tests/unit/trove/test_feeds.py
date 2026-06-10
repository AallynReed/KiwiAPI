from app.trove.feeds import _has_term, _normalize_twitch, _normalize_videos


def test_has_term_is_whole_word_not_substring():
    # Matches the word (and possessive boundary) but not a substring inside another word.
    assert _has_term("trove's summer event is live", "trove")
    assert not _has_term("introvert productivity tips", "trove")
    # Multi-word terms match as a phrase.
    assert _has_term("trove chaos chest farming guide", "chaos chest")
    assert not _has_term("opening a chest in another game", "chaos chest")


def test_relevance_gate_keeps_trove_drops_ambiguous():
    # Mirror of _fetch_youtube's keep rule on the curation vocab.
    require, signal, exclude = {"trove"}, {"geode", "gunslinger"}, {"trinket trove"}

    def keep(blob: str, category: str, gaming: str = "20") -> bool:
        blob = blob.lower()
        if any(_has_term(blob, t) for t in exclude):
            return False
        if not all(_has_term(blob, t) for t in require):
            return False
        in_gaming = bool(gaming) and category == gaming
        has_signal = any(_has_term(blob, t) for t in signal)
        return bool(signal) and (has_signal or in_gaming)

    assert keep("Trove Geode farming guide", "22")        # signal term
    assert keep("Trove Gunslinger build 2026", "20")      # signal + gaming
    assert not keep("Capital One Trove savings review", "25")   # trove, no signal, not gaming
    assert not keep("Treasure trove of vlog secrets", "22")     # idiom, no signal
    assert not keep("Trinket Trove DIY craft", "26")            # excluded
    assert not keep("My daily coffee vlog", "22")               # no 'trove'


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
