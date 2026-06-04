"""Integration test fixtures.

These spin up a real MongoDB + Redis via testcontainers, so they need Docker
available (CI, or a local machine with Docker). Marked `integration`.
"""
import pytest
import pytest_asyncio
from httpx import ASGITransport, AsyncClient
from pymongo import AsyncMongoClient
from testcontainers.mongodb import MongoDbContainer
from testcontainers.redis import RedisContainer


@pytest.fixture(scope="session")
def mongo_url():
    with MongoDbContainer("mongo:7") as mongo:
        yield mongo.get_connection_url()


@pytest.fixture(scope="session")
def redis_url():
    with RedisContainer("redis:7-alpine") as r:
        host = r.get_container_host_ip()
        port = r.get_exposed_port(6379)
        yield f"redis://{host}:{port}/0"


@pytest_asyncio.fixture
async def client(mongo_url, redis_url):
    from app.core.config import settings

    settings.mongo_uri = mongo_url
    settings.mongo_db = "kiwi_test"
    settings.redis_url = redis_url
    # Deterministic, no external calls.
    settings.captcha_secret = None
    settings.captcha_sitekey = None
    settings.require_verified_for_tokens = False
    settings.require_verified_for_login = False
    settings.password_breach_check = False  # don't hit HIBP
    settings.security_email_notifications = False

    # Clean slate each test (Mongo + Redis), so rate-limit/lockout counters reset.
    raw = AsyncMongoClient(mongo_url)
    await raw.drop_database("kiwi_test")
    await raw.close()

    from app.core.database import close_db, init_db
    from app.core.redis import close_redis, get_redis, init_redis

    await init_db()
    await init_redis()
    r = get_redis()
    if r is not None:
        await r.flushdb()

    from app.main import app

    transport = ASGITransport(app=app, client=("1.2.3.4", 12345))
    async with AsyncClient(transport=transport, base_url="http://test") as ac:
        yield ac

    await close_redis()
    await close_db()
