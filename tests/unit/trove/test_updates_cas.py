import hashlib

from app.trove.updates.cas import ContentStore


def test_put_get_has_and_dedup(tmp_path):
    store = ContentStore(tmp_path)
    sha, created = store.put(b"hello trove")
    assert created is True
    assert sha == hashlib.sha256(b"hello trove").hexdigest()
    assert store.has(sha) and store.get(sha) == b"hello trove"
    # Sharded into objects/<sha[:2]>/<sha>.
    assert store.path_for(sha).parent.name == sha[:2]
    # Same content -> same key, not re-created.
    sha2, created2 = store.put(b"hello trove")
    assert sha2 == sha and created2 is False


def test_get_missing_is_none(tmp_path):
    store = ContentStore(tmp_path)
    assert store.get("00" * 32) is None
    assert store.has("00" * 32) is False


def test_empty_blob(tmp_path):
    store = ContentStore(tmp_path)
    sha, created = store.put(b"")
    assert created is True and store.get(sha) == b""
