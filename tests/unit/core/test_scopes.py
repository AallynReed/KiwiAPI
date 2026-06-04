from app.core.scopes import (
    ALL_SCOPES,
    SCOPE_BITS,
    catalog,
    decode,
    is_valid_mask,
    mask_grants,
)

# Scopes are `<resource>:<action>`; bits are permanent. The base now defines the
# first real scope, trove:read (bit 1), alongside the Trove data endpoints.


def test_trove_scope_registered():
    assert SCOPE_BITS == {"trove:read": 1}
    assert ALL_SCOPES == 0
    assert catalog()[0]["resource"] == "trove"
    assert all(":" in c["key"] for c in catalog())  # naming convention


def test_mask_grants():
    assert mask_grants(0, "trove:read")        # 0 = all (present + future)
    assert mask_grants(1, "trove:read")
    assert not mask_grants(2, "trove:read")    # bit 2 isn't trove:read
    assert not mask_grants(1, "unknown:scope")


def test_is_valid_mask():
    assert is_valid_mask(0) and is_valid_mask(1)
    assert not is_valid_mask(2)                # bit 2 not assigned


def test_decode():
    assert decode(0) == []
    assert decode(1) == ["trove:read"]
