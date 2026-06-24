"""Per-project git repositories - the source of truth for a mod's files & history.

Backed by **dulwich** (pure-Python git, no native binary). Each project is a bare
repo at ``<mods_store_dir>/git/<project_id>.git``. Both the web studio ("Commit
files") and a remote ``git push`` land here; the hub's commit/branch/tree/release
views are projected from these repos. Compiled ``.tmod`` release artifacts and
images still live in the CAS (``store.py``) - only versioned file content is git.

dulwich is synchronous; the public functions here are ``async`` wrappers that run
the blocking work in a threadpool, mirroring ``store.py``.
"""

from __future__ import annotations

import asyncio
from io import BytesIO
from pathlib import Path

from dulwich.object_store import iter_tree_contents
from dulwich.objects import Blob, Commit
from dulwich.index import commit_tree
from dulwich.protocol import Protocol, ReceivableProtocol, pkt_line
from dulwich.repo import Repo
from dulwich.server import DictBackend, ReceivePackHandler, UploadPackHandler

from app.core.config import settings

_GIT_AUTHOR_DOMAIN = "users.noreply.kiwi"
_HANDLERS = {b"git-upload-pack": UploadPackHandler, b"git-receive-pack": ReceivePackHandler}


class GitStoreError(Exception):
    pass


class NothingToCommit(GitStoreError):
    pass


def _root() -> Path:
    return Path(settings.mods_store_dir) / "git"


def repo_dir(project_id: str) -> Path:
    return _root() / f"{project_id}.git"


def _ensure(project_id: str) -> Repo:
    d = repo_dir(project_id)
    if (d / "objects").is_dir() or (d / "HEAD").is_file():
        return Repo(str(d))
    d.mkdir(parents=True, exist_ok=True)
    repo = Repo.init_bare(str(d))
    # Default branch "main" (matches ModProject.default_branch). Point HEAD at it
    # up front so `git clone` checks out the right branch once it has commits.
    repo.refs.set_symbolic_ref(b"HEAD", b"refs/heads/main")
    return repo


def _open(project_id: str) -> Repo | None:
    d = repo_dir(project_id)
    if (d / "objects").is_dir() or (d / "HEAD").is_file():
        return Repo(str(d))
    return None


def author_ident(username: str) -> bytes:
    safe = "".join(c for c in (username or "modder") if c.isalnum() or c in "-_.") or "modder"
    return f"{username} <{safe}@{_GIT_AUTHOR_DOMAIN}>".encode("utf-8")


def _name(ident: bytes) -> str:
    s = ident.decode("utf-8", "replace")
    return s.split(" <", 1)[0] if " <" in s else s


def _branch_ref(name: str) -> bytes:
    return b"refs/heads/" + name.encode("utf-8")


def _commit_meta(repo: Repo, c: Commit) -> dict:
    file_count = sum(1 for _ in iter_tree_contents(repo.object_store, c.tree))
    return {
        "sha": c.id.decode(),
        "parents": [p.decode() for p in c.parents],
        "author": _name(c.author),
        "message": c.message.decode("utf-8", "replace").strip(),
        "time": int(c.commit_time),
        "file_count": file_count,
    }


# --- sync implementations --------------------------------------------------

def _resolve(repo: Repo, ref: str) -> bytes | None:
    """Resolve a ref (branch name or 40-hex commit sha) to a commit sha (bytes)."""
    refs = repo.get_refs()
    bref = _branch_ref(ref)
    if bref in refs:
        return refs[bref]
    cand = ref.lower().encode()
    if len(cand) == 40:
        try:
            if isinstance(repo.object_store[cand], Commit):
                return cand
        except KeyError:
            return None
    return None


def _write_commit(project_id, branch, adds, deletes, author, message, ts) -> str:
    repo = _ensure(project_id)
    store = repo.object_store
    ref = _branch_ref(branch)
    refs = repo.get_refs()
    parent = refs.get(ref)
    entries: dict[bytes, tuple[int, bytes]] = {}
    if parent is not None:
        for e in iter_tree_contents(store, store[parent].tree):
            entries[e.path] = (e.mode, e.sha)
    for d in deletes:
        entries.pop(d.encode("utf-8"), None)
    for path, content in adds:
        blob = Blob.from_string(content)
        store.add_object(blob)
        entries[path.encode("utf-8")] = (0o100644, blob.id)
    tree_id = commit_tree(store, [(p, sha, mode) for p, (mode, sha) in entries.items()])
    if parent is not None and store[parent].tree == tree_id:
        raise NothingToCommit()
    c = Commit()
    c.tree = tree_id
    c.author = c.committer = author
    c.author_time = c.commit_time = int(ts)
    c.author_timezone = c.commit_timezone = 0
    c.encoding = b"UTF-8"
    c.message = (message or "").encode("utf-8") or b"(no message)"
    if parent is not None:
        c.parents = [parent]
    store.add_object(c)
    repo.refs[ref] = c.id
    return c.id.decode()


def _read_tree(project_id, ref) -> tuple[dict, list[dict]] | None:
    repo = _open(project_id)
    if repo is None:
        return None
    sha = _resolve(repo, ref)
    if sha is None:
        return None
    c = repo.object_store[sha]
    entries = [
        {"path": e.path.decode("utf-8", "replace"), "blob_sha": e.sha.decode(),
         "size": len(repo.object_store[e.sha].data)}
        for e in iter_tree_contents(repo.object_store, c.tree)
    ]
    entries.sort(key=lambda x: x["path"])
    return _commit_meta(repo, c), entries


def _read_blob(project_id, ref, path) -> bytes | None:
    repo = _open(project_id)
    if repo is None:
        return None
    sha = _resolve(repo, ref)
    if sha is None:
        return None
    target = path.encode("utf-8")
    for e in iter_tree_contents(repo.object_store, repo.object_store[sha].tree):
        if e.path == target:
            return repo.object_store[e.sha].data
    return None


def _list_branches(project_id) -> list[dict]:
    repo = _open(project_id)
    if repo is None:
        return []
    out = []
    for name, sha in repo.get_refs().items():
        if name.startswith(b"refs/heads/"):
            out.append({"name": name[len("refs/heads/"):].decode(), "head": sha.decode()})
    out.sort(key=lambda b: b["name"])
    return out


def _list_commits(project_id, ref, limit) -> list[dict]:
    repo = _open(project_id)
    if repo is None:
        return []
    head = _resolve(repo, ref)
    if head is None:
        return []
    out = []
    for entry in repo.get_walker(include=[head], max_entries=limit):
        out.append(_commit_meta(repo, entry.commit))
    return out


def _count_commits(project_id, ref) -> int:
    repo = _open(project_id)
    if repo is None:
        return 0
    head = _resolve(repo, ref)
    if head is None:
        return 0
    return sum(1 for _ in repo.get_walker(include=[head]))


def _create_branch(project_id, name, start_ref) -> str | None:
    repo = _ensure(project_id)
    start = _resolve(repo, start_ref) if start_ref else None
    if start is None:
        # Fall back to whatever HEAD points at; an empty repo can't branch.
        for _, sha in repo.get_refs().items():
            start = sha
            break
    if start is None:
        raise GitStoreError("Commit something before creating a branch.")
    repo.refs[_branch_ref(name)] = start
    return start.decode()


def _delete_branch(project_id, name) -> None:
    repo = _open(project_id)
    if repo is None:
        return
    try:
        del repo.refs[_branch_ref(name)]
    except KeyError:
        pass


def _fork(src_id, dst_id, src_branch, branch, author, message, ts) -> str | None:
    dst = _ensure(dst_id)
    src = _open(src_id)
    if src is None:
        return None
    src_head = _resolve(src, src_branch)
    if src_head is None:
        return None
    src_commit = src.object_store[src_head]
    entries = []
    for e in iter_tree_contents(src.object_store, src_commit.tree):
        dst.object_store.add_object(src.object_store[e.sha])  # copy the blob over
        entries.append((e.path, e.sha, e.mode))
    tree_id = commit_tree(dst.object_store, entries)
    c = Commit()
    c.tree = tree_id
    c.author = c.committer = author
    c.author_time = c.commit_time = int(ts)
    c.author_timezone = c.commit_timezone = 0
    c.encoding = b"UTF-8"
    c.message = (message or "fork").encode("utf-8")
    dst.object_store.add_object(c)
    dst.refs[_branch_ref(branch)] = c.id
    return c.id.decode()


def _delete_repo(project_id) -> None:
    import shutil
    d = repo_dir(project_id)
    if d.exists():
        shutil.rmtree(d, ignore_errors=True)


# --- smart-HTTP (sync) -----------------------------------------------------

def _advertise(project_id, service: bytes) -> bytes:
    repo = _ensure(project_id)
    backend = DictBackend({b"/": repo})
    out = BytesIO()
    proto = Protocol(None, out.write)
    handler = _HANDLERS[service](backend, [b"/"], proto, stateless_rpc=True, advertise_refs=True)
    handler.handle()
    return pkt_line(b"# service=" + service + b"\n") + b"0000" + out.getvalue()


def _service(project_id, service: bytes, data: bytes) -> bytes:
    repo = _ensure(project_id)
    backend = DictBackend({b"/": repo})
    inp = BytesIO(data)
    out = BytesIO()
    proto = ReceivableProtocol(inp.read, out.write)
    handler = _HANDLERS[service](backend, [b"/"], proto, stateless_rpc=True)
    handler.handle()
    return out.getvalue()


# --- async public surface --------------------------------------------------

async def ensure_repo(project_id: str) -> None:
    await asyncio.to_thread(_ensure, project_id)


async def write_commit(project_id, *, branch, adds, deletes, author, message, ts) -> str:
    return await asyncio.to_thread(_write_commit, project_id, branch, adds, deletes, author, message, ts)


async def read_tree(project_id, ref) -> tuple[dict, list[dict]] | None:
    return await asyncio.to_thread(_read_tree, project_id, ref)


async def read_blob(project_id, ref, path) -> bytes | None:
    return await asyncio.to_thread(_read_blob, project_id, ref, path)


async def list_branches(project_id) -> list[dict]:
    return await asyncio.to_thread(_list_branches, project_id)


async def list_commits(project_id, ref, limit) -> list[dict]:
    return await asyncio.to_thread(_list_commits, project_id, ref, limit)


async def count_commits(project_id, ref) -> int:
    return await asyncio.to_thread(_count_commits, project_id, ref)


async def resolve(project_id, ref) -> str | None:
    def _r():
        repo = _open(project_id)
        if repo is None:
            return None
        sha = _resolve(repo, ref)
        return sha.decode() if sha else None
    return await asyncio.to_thread(_r)


async def create_branch(project_id, name, start_ref) -> str | None:
    return await asyncio.to_thread(_create_branch, project_id, name, start_ref)


async def delete_branch(project_id, name) -> None:
    await asyncio.to_thread(_delete_branch, project_id, name)


async def fork(src_id, dst_id, *, src_branch, branch, author, message, ts) -> str | None:
    return await asyncio.to_thread(_fork, src_id, dst_id, src_branch, branch, author, message, ts)


async def delete_repo(project_id) -> None:
    await asyncio.to_thread(_delete_repo, project_id)


async def advertise_refs(project_id, service: bytes) -> bytes:
    return await asyncio.to_thread(_advertise, project_id, service)


async def run_service(project_id, service: bytes, data: bytes) -> bytes:
    return await asyncio.to_thread(_service, project_id, service, data)
