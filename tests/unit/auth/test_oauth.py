from app.auth.oauth import _pick_email


def test_pick_email_prefers_verified_primary():
    emails = [
        {"email": "secondary@x.com", "primary": False, "verified": True},
        {"email": "primary@x.com", "primary": True, "verified": True},
    ]
    assert _pick_email(emails) == "primary@x.com"


def test_pick_email_falls_back_to_any_verified():
    emails = [
        {"email": "primary@x.com", "primary": True, "verified": False},
        {"email": "other@x.com", "primary": False, "verified": True},
    ]
    assert _pick_email(emails) == "other@x.com"


def test_pick_email_rejects_all_unverified():
    emails = [
        {"email": "primary@x.com", "primary": True, "verified": False},
        {"email": "other@x.com", "primary": False, "verified": False},
    ]
    assert _pick_email(emails) is None


def test_pick_email_handles_garbage():
    assert _pick_email(None) is None
    assert _pick_email([]) is None
    assert _pick_email("not-a-list") is None
