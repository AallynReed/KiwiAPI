"""End-to-end ingestion over fakes: proves the pipeline's add/modify/remove/dedup
logic offline, with a real CAS and an in-memory repo + CDN."""

import hashlib
import zlib

import pytest

from app.trove.troveio import calculate_hash, write_leb128
from app.trove.updates.cas import ContentStore
from app.trove.updates.ingest import sync_branch

pytestmark = pytest.mark.asyncio


def _sha1(b: bytes) -> str:
    return hashlib.sha1(b).hexdigest()


def _build_tfi_tfa(dir_files):
    """(name, archive_index, bytes) list -> (tfi_bytes, {arc: tfa_raw})."""
    contents: dict[int, bytearray] = {}
    tfi = bytearray()
    for name, arc, data in dir_files:
        buf = contents.setdefault(arc, bytearray())
        offset = len(buf)
        buf += data
        nb = name.encode("utf-8") + b"\x00"
        tfi += write_leb128(len(nb)) + nb
        tfi += write_leb128(arc) + write_leb128(offset)
        tfi += write_leb128(len(data)) + write_leb128(calculate_hash(data))
    return bytes(tfi), {arc: zlib.compress(bytes(buf)) for arc, buf in contents.items()}


def make_game(loose: dict[str, bytes], dirs: dict[str, list]):
    """Build a synthetic manifest + the file bytes the CDN would serve."""
    manifest, files = [], {}
    for path, data in loose.items():
        files[path] = data
        manifest.append({"path": path, "sha1": _sha1(data), "size": len(data)})
    for d, dfiles in dirs.items():
        tfi, tfas = _build_tfi_tfa(dfiles)
        tfi_path = f"{d}/index.tfi"
        files[tfi_path] = tfi
        manifest.append({"path": tfi_path, "sha1": _sha1(tfi), "size": len(tfi)})
        for arc, tfa in tfas.items():
            tfa_path = f"{d}/archive{arc}.tfa"
            files[tfa_path] = tfa
            manifest.append({"path": tfa_path, "sha1": _sha1(tfa), "size": len(tfa)})
    return manifest, files


class FakeCdn:
    def __init__(self, version, content_path, manifest, files):
        self.pointer = {"version": version, "content_path": content_path, "motd": ""}
        self.manifest = manifest
        self.files = files

    async def fetch_pointer(self, pointer_file):
        return self.pointer

    async def fetch_manifest(self, content_path, version):
        return self.pointer["version"], self.manifest

    async def download_file(self, content_path, path, sha1, expected_size=None):
        return self.files[path]


class FakeRepo:
    def __init__(self):
        self.manifest, self.state, self.changes, self.versions, self.branch = {}, {}, [], [], {}

    async def get_manifest_sidecar(self, branch):
        return {p: dict(v) for p, v in self.manifest.items()}

    async def begin_version(self, branch, version_tag, pointer):
        for v in self.versions:
            if v["status"] == "in_progress":
                return v["ordinal"], True
        ordinal = max((v["ordinal"] for v in self.versions), default=0) + 1
        self.versions.append({"ordinal": ordinal, "version_tag": version_tag, "status": "in_progress"})
        return ordinal, False

    async def state_get(self, branch, path):
        st = self.state.get(path)
        return dict(st) if st else None

    async def get_archive_state(self, branch, directory):
        return {p: {"fnv_hash": st["fnv_hash"], "content_sha256": st["content_sha256"]}
                for p, st in self.state.items() if st.get("archive") == directory}

    async def record_change(self, branch, ordinal, path, change_type, content_sha256, fnv_hash, size):
        self.changes.append({"ordinal": ordinal, "path": path, "type": change_type,
                             "content_sha256": content_sha256, "fnv_hash": fnv_hash, "size": size})

    async def upsert_state(self, branch, path, content_sha256, fnv_hash, size, archive, archive_index):
        self.state[path] = {"content_sha256": content_sha256, "fnv_hash": fnv_hash, "size": size,
                            "archive": archive, "archive_index": archive_index}

    async def remove_state(self, branch, path):
        self.state.pop(path, None)

    async def set_manifest_entry(self, branch, path, sha1, size):
        self.manifest[path] = {"sha1": sha1, "size": size}

    async def remove_manifest_entry(self, branch, path):
        self.manifest.pop(path, None)

    async def finish_version(self, branch, ordinal, version_tag, pointer, counts):
        for v in self.versions:
            if v["ordinal"] == ordinal:
                v["status"] = "complete"
                v.update(counts)
        self.branch = {"current_version": version_tag, "current_ordinal": ordinal}

    async def touch_probe(self, branch, content_path, version_tag):
        self.branch["current_version"] = version_tag


async def test_full_then_incremental_lifecycle(tmp_path):
    store = ContentStore(tmp_path)
    repo = FakeRepo()
    loose = {"Trove_x64.exe": b"exe-v1", "Trove.cfg": b"[cfg]"}

    # --- v1: full first sync - everything is added ---
    manifest, files = make_game(loose, {"ui": [("a.bin", 0, b"alpha-content"), ("b.bin", 0, b"bravo-content")]})
    s1 = await sync_branch("live-us", "kiwi-live-us.txt", FakeCdn("V1", "cp", manifest, files), store, repo)
    assert s1["changed"] and s1["ordinal"] == 1
    assert (s1["added"], s1["modified"], s1["removed"]) == (4, 0, 0)
    assert set(repo.state) == {"Trove_x64.exe", "Trove.cfg", "ui/a.bin", "ui/b.bin"}
    assert store.get(repo.state["ui/a.bin"]["content_sha256"]) == b"alpha-content"

    # --- v2: modify only ui/b.bin (archive0 + tfi change; a.bin must dedup) ---
    m2, f2 = make_game(loose, {"ui": [("a.bin", 0, b"alpha-content"), ("b.bin", 0, b"BRAVO-CHANGED!")]})
    s2 = await sync_branch("live-us", "kiwi-live-us.txt", FakeCdn("V2", "cp", m2, f2), store, repo)
    assert (s2["added"], s2["modified"], s2["removed"]) == (0, 1, 0) and s2["ordinal"] == 2
    assert repo.state["ui/b.bin"]["content_sha256"] == hashlib.sha256(b"BRAVO-CHANGED!").hexdigest()
    assert repo.state["ui/a.bin"]["content_sha256"] == hashlib.sha256(b"alpha-content").hexdigest()
    o2 = [c for c in repo.changes if c["ordinal"] == 2]
    assert len(o2) == 1 and o2[0]["path"] == "ui/b.bin" and o2[0]["type"] == "modified"
    assert s2["bytes_added"] == len(b"BRAVO-CHANGED!")  # only the new blob; a.bin deduped

    # --- v3: nothing changed -> no new version ---
    s3 = await sync_branch("live-us", "kiwi-live-us.txt", FakeCdn("V2", "cp", m2, f2), store, repo)
    assert s3["changed"] is False
    assert [v["ordinal"] for v in repo.versions] == [1, 2]  # no v3 created

    # --- v4: remove ui/a.bin ---
    m4, f4 = make_game(loose, {"ui": [("b.bin", 0, b"BRAVO-CHANGED!")]})
    s4 = await sync_branch("live-us", "kiwi-live-us.txt", FakeCdn("V3", "cp", m4, f4), store, repo)
    assert (s4["added"], s4["modified"], s4["removed"]) == (0, 0, 1) and s4["ordinal"] == 3
    assert "ui/a.bin" not in repo.state
    assert [c["path"] for c in repo.changes if c["ordinal"] == 3] == ["ui/a.bin"]


async def test_loose_file_modify_and_remove(tmp_path):
    store, repo = ContentStore(tmp_path), FakeRepo()
    m1, f1 = make_game({"keep.dll": b"k", "drop.dll": b"d"}, {})
    await sync_branch("pts", "kiwi-pts.txt", FakeCdn("V1", "cp", m1, f1), store, repo)
    assert set(repo.state) == {"keep.dll", "drop.dll"}

    # keep.dll modified, drop.dll removed.
    m2, f2 = make_game({"keep.dll": b"k2"}, {})
    s = await sync_branch("pts", "kiwi-pts.txt", FakeCdn("V2", "cp", m2, f2), store, repo)
    assert (s["added"], s["modified"], s["removed"]) == (0, 1, 1)
    assert repo.state["keep.dll"]["content_sha256"] == hashlib.sha256(b"k2").hexdigest()
    assert "drop.dll" not in repo.state
