import pytest

from app.core.errors import APIError
from app.core.limits import endpoint_limit_for, register_endpoint_limit
from app.core.pagination import Page, decode_cursor


def test_endpoint_limit_lookup():
    # The base registers no per-endpoint limits; lookups return None.
    assert endpoint_limit_for(None) is None
    assert endpoint_limit_for("/nonexistent") is None


def test_register_endpoint_limit():
    register_endpoint_limit("/v1/test/heavy", 5, 30)
    assert endpoint_limit_for("/v1/test/heavy") == (5, 30)


def test_decode_cursor_none():
    assert decode_cursor(None) is None


def test_decode_cursor_valid():
    oid = "507f1f77bcf86cd799439011"
    assert str(decode_cursor(oid)) == oid


def test_decode_cursor_invalid_raises():
    with pytest.raises(APIError) as e:
        decode_cursor("not-an-objectid")
    assert e.value.status_code == 400


def test_page_defaults():
    p = Page[str](items=["a", "b"])
    assert p.next_cursor is None and p.has_more is False
