from app.core.scopes import (
    ALL_SCOPES,
    SCOPE_BITS,
    catalog,
    decode,
    is_valid_mask,
    mask_grants,
)

# The 1.0 base ships with NO scopes — they arrive with the real endpoints.
# These tests pin the empty-registry behaviour so the mechanism stays correct.


def test_registry_is_empty():
    assert SCOPE_BITS == {}
    assert catalog() == []
    assert ALL_SCOPES == 0


def test_mask_zero_grants_everything():
    assert mask_grants(0, "anything:read")          # 0 = all, present + future
    assert not mask_grants(1, "anything:read")      # no scope registered for bit 1


def test_only_all_mask_is_valid_without_scopes():
    assert is_valid_mask(0)
    assert not is_valid_mask(1)                      # no known bits yet


def test_decode_is_empty():
    assert decode(0) == []
    assert decode(7) == []
