from fastapi import FastAPI
from fastapi.testclient import TestClient


def test_security_headers_and_body_limit(monkeypatch):
    from app.core.config import settings

    monkeypatch.setattr(settings, "max_request_body_bytes", 10)
    from app.core.middleware import add_security_middleware

    app = FastAPI()
    add_security_middleware(app)

    @app.post("/x")
    async def x():
        return {"ok": True}

    client = TestClient(app)

    r = client.post("/x", json={})
    assert r.status_code == 200
    assert r.headers["x-content-type-options"] == "nosniff"
    assert r.headers["x-frame-options"] == "DENY"
    assert "content-security-policy" in r.headers
    assert "strict-transport-security" in r.headers

    # Oversized body (50 bytes > 10) is rejected.
    r = client.post("/x", content=b"x" * 50)
    assert r.status_code == 413
    assert r.json()["error"]["code"] == "bad_request"
