from types import SimpleNamespace

from app.core.context import get_request_id, new_request_id, reset_request_id, set_request_id
from app.core.idempotency import _storage_key


def test_request_id_generation_unique():
    a, b = new_request_id(), new_request_id()
    assert a.startswith("req_") and b.startswith("req_")
    assert a != b


def test_request_id_context_roundtrip():
    assert get_request_id() == "-"          # default outside a request
    token = set_request_id("req_test123")
    try:
        assert get_request_id() == "req_test123"
    finally:
        reset_request_id(token)
    assert get_request_id() == "-"


def _req(auth: str, method: str, path: str):
    return SimpleNamespace(
        headers={"authorization": auth},
        method=method,
        url=SimpleNamespace(path=path),
    )


def test_idempotency_key_is_deterministic_and_namespaced():
    base = _storage_key(_req("Bearer a", "POST", "/v1/things"), "K1")
    same = _storage_key(_req("Bearer a", "POST", "/v1/things"), "K1")
    other_cred = _storage_key(_req("Bearer b", "POST", "/v1/things"), "K1")
    other_path = _storage_key(_req("Bearer a", "POST", "/v1/other"), "K1")
    other_key = _storage_key(_req("Bearer a", "POST", "/v1/things"), "K2")

    assert base == same                  # deterministic
    assert base.startswith("idem:")
    assert len({base, other_cred, other_path, other_key}) == 4  # all namespaced apart
