import pytest
from fastapi import Depends, FastAPI
from fastapi.testclient import TestClient
from pydantic import BaseModel

from app.core.dependencies import get_current_user
from app.core.errors import APIError, ErrorCode, register_error_handlers


@pytest.fixture
def client():
    app = FastAPI()
    register_error_handlers(app)

    class Body(BaseModel):
        n: int

    @app.post("/boom")
    async def boom():
        raise APIError(409, ErrorCode.email_taken, "taken", details={"x": 1})

    @app.post("/val")
    async def val(b: Body):
        return b

    @app.get("/secure")
    async def secure(user=Depends(get_current_user)):
        return {}

    return TestClient(app, raise_server_exceptions=True)


def test_apierror_envelope(client):
    r = client.post("/boom")
    assert r.status_code == 409
    body = r.json()["error"]
    assert body["code"] == "email_taken"
    assert body["message"] == "taken"
    assert body["details"] == {"x": 1}
    assert "request_id" in body  # every error carries a correlation id


def test_validation_envelope(client):
    r = client.post("/val", json={"n": "nope"})
    assert r.status_code == 422
    assert r.json()["error"]["code"] == "validation_error"


def test_missing_auth_is_consistent_401(client):
    r = client.get("/secure")
    assert r.status_code == 401
    assert r.json()["error"]["code"] == "not_authenticated"
    assert r.headers.get("www-authenticate") == "Bearer"


def test_unknown_route_404_envelope(client):
    r = client.get("/nope")
    assert r.status_code == 404
    assert r.json()["error"]["code"] == "not_found"
