"""Supporters feature - seed list, schemas, and route wiring."""


def test_default_supporters_are_the_seven():
    from app.supporters.service import DEFAULT_SUPPORTERS
    assert DEFAULT_SUPPORTERS == [
        "IINikstarII", "Nao373", "Wahoo", "boryzje", "nz", "Grainus", "Tues",
    ]
    assert len(DEFAULT_SUPPORTERS) == len(set(DEFAULT_SUPPORTERS))   # no dupes


def test_supporter_schemas_roundtrip():
    from app.supporters.schemas import SupporterAdminList, SupporterAdminView, SupporterList
    from datetime import datetime, timezone

    pub = SupporterList(supporters=["A", "B"], count=2)
    assert pub.count == 2 and pub.supporters == ["A", "B"]

    row = SupporterAdminView(name="A", added_by=None, created_at=datetime(2026, 1, 1, tzinfo=timezone.utc))
    adm = SupporterAdminList(items=[row], count=1)
    assert adm.items[0].name == "A" and adm.count == 1


def test_routes_registered_public_and_admin():
    from app.main import app

    # FastAPI registers one route object per (path, method); aggregate methods.
    methods: dict[str, set] = {}
    in_schema: dict[str, bool] = {}
    for r in app.routes:
        path = getattr(r, "path", "")
        methods.setdefault(path, set()).update(getattr(r, "methods", set()) or set())
        in_schema[path] = in_schema.get(path, False) or getattr(r, "include_in_schema", False)

    # Public endpoint: tokenless misc:read, GET, in the schema.
    assert "GET" in methods.get("/v1/misc/supporters", set())
    assert in_schema.get("/v1/misc/supporters")
    # Admin CRUD present (master-only via the router-level dep).
    assert {"GET", "POST", "PUT"} <= methods.get("/admin/supporters", set())
    assert "DELETE" in methods.get("/admin/supporters/{name}", set())
