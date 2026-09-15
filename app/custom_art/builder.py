"""The custom art release worker: ``python -m app.custom_art.builder``.

Runs as its own container on the api image. It claims whatever the master has
queued and, for the whole batch:

1. brings its TroveUI checkout to the tip of the bare repo, and ``lib`` to KiwiZUI,
2. puts the game's own vanilla SWFs and English language files in from the updates
   archive, so the builds verify against what the live client ships,
3. places each picture with TroveUI's ``custom_art.py``, commits and pushes that,
4. releases each affected mod silently to the hub, and to Trovesaurus unless the
   mod is beta, then commits and pushes the version bump,
5. marks each request released and emails the person who asked.

Steam is left to the master, so a released request stays ``steam_pending``. A
failure marks what did not ship ``failed`` with the log; the checkout is reset on
the next run, so a retry starts clean and skips mods already released.
"""
import asyncio
import logging
import os
import re
import secrets
import shutil
import subprocess
import sys
from pathlib import Path

from app.core.config import settings
from app.core.database import init_db
from app.core.observability import configure_logging
from app.core.utils import utcnow
from app.custom_art import service
from app.custom_art.models import ArtRequest
from app.trove.mods_hub import store
from app.trove.mods_hub.models import ModProject
from app.trove.updates.cas import ContentStore
from app.trove.updates.models import UpdateState

logger = logging.getLogger("kiwi.custom_art.builder")

VANILLA = {
    service.CHAT: ("ui/chat.swf", "chat.vanilla.swf"),
    service.NAMEPLATE: ("ui/nameplate.swf", "nameplate.vanilla.swf"),
}
ART = (f"{service.CHAT}/art", f"{service.NAMEPLATE}/art")
_TIMEOUT = 1800
_LOG_MAX = 60_000


class BuildFailed(Exception):
    pass


class Run:
    """Runs one command at a time and keeps everything it printed for the log."""

    def __init__(self, env: dict[str, str]) -> None:
        self.env = env
        self.lines: list[str] = []

    def say(self, text: str) -> None:
        self.lines.append(text)
        logger.info(text)

    def __call__(self, args: list[str], cwd: Path, check: bool = True) -> bool:
        self.say("$ " + " ".join(args))
        try:
            done = subprocess.run(args, cwd=cwd, env=self.env, capture_output=True,
                                  text=True, timeout=_TIMEOUT)
        except subprocess.TimeoutExpired:
            raise BuildFailed(f"timed out: {' '.join(args)}") from None
        out = (done.stdout + done.stderr).rstrip()
        if out:
            self.lines.append(out)
        if done.returncode != 0 and check:
            raise BuildFailed(f"exit {done.returncode}: {' '.join(args)}")
        return done.returncode == 0

    def text(self) -> str:
        return "\n".join(self.lines)[-_LOG_MAX:]


def _root() -> Path:
    return Path(settings.custom_art_workspace)


def _checkout(run: Run) -> Path:
    root = _root()
    tree = root / "TroveUI"
    root.mkdir(parents=True, exist_ok=True)
    if not (tree / ".git").is_dir():
        run(["git", "clone", settings.custom_art_remote, str(tree)], root)
    run(["git", "fetch", "origin"], tree)
    run(["git", "reset", "--hard", "origin/master"], tree)
    run(["git", "clean", "-fd"], tree)
    lib = tree / "lib"
    if not (lib / ".git").is_dir():
        shutil.rmtree(lib, ignore_errors=True)
        run(["git", "clone", "--depth", "1", settings.custom_art_lib_remote, str(lib)], tree)
    else:
        run(["git", "fetch", "--depth", "1", "origin", "HEAD"], lib)
        run(["git", "reset", "--hard", "FETCH_HEAD"], lib)
    shutil.copyfile(settings.custom_art_env_file, tree / ".env")
    return tree


async def _game_files(tree: Path, run: Run) -> None:
    blobs = ContentStore(settings.trove_update_store_dir)
    branch = settings.custom_art_branch

    def copy(sha: str, dest: Path) -> None:
        src = blobs.path_for(sha)
        if not src.is_file():
            raise BuildFailed(f"archive blob {sha} is missing")
        shutil.copyfile(src, dest)

    lang = _root() / "trove" / "languages" / "en"
    shutil.rmtree(lang, ignore_errors=True)
    lang.mkdir(parents=True)
    rows = await UpdateState.find(
        {"branch": branch, "path": {"$regex": r"^languages/en/[^/]+\.binfab$"}}).to_list()
    if not rows:
        raise BuildFailed(f"no English language files archived on {branch}")
    for row in rows:
        copy(row.content_sha256, lang / Path(row.path).name)
    for mod, (path, name) in VANILLA.items():
        row = await UpdateState.find_one({"branch": branch, "path": path})
        if row is None:
            raise BuildFailed(f"{path} is not archived on {branch}")
        copy(row.content_sha256, tree / mod / name)
    run.say(f"{len(rows)} language files and {len(VANILLA)} vanilla SWFs from {branch}")


async def _place(tree: Path, requests: list[ArtRequest], run: Run) -> None:
    incoming = _root() / "incoming"
    shutil.rmtree(incoming, ignore_errors=True)
    incoming.mkdir(parents=True)
    for request in requests:
        data = await store.get_blob(request.image_sha)
        if data is None:
            raise BuildFailed(f"the picture for {request.name!r} is missing from the store")
        source = incoming / str(request.id)
        source.write_bytes(data)
        await asyncio.to_thread(run, [sys.executable, "custom_art.py", request.kind,
                                      request.name, str(source)], tree)


def _commit(run: Run, tree: Path, paths: list[str], message: str) -> None:
    status = subprocess.run(["git", "status", "--porcelain", "--", *paths], cwd=tree,
                            capture_output=True, text=True).stdout
    if not status.strip():
        run.say(f"nothing to commit for {message!r}")
        return
    run(["git", "add", "-A", "--", *paths], tree)
    run(["git", "commit", "-m", message], tree)
    run(["git", "push", "origin", "HEAD:master"], tree)


def _short(mod: str) -> str:
    return mod.removeprefix("Zakros UI - ")


def _art_message(requests: list[ArtRequest]) -> str:
    names = [r.name for r in requests]
    shown = ", ".join(names[:4]) + (f" and {len(names) - 4} more" if len(names) > 4 else "")
    return f"Custom art: {shown}"


def _version(tree: Path, mod: str) -> str:
    found = re.search(r'"modVersion":\s*"([^"]*)"',
                      (tree / mod / "build.py").read_text(encoding="utf-8"))
    if not found:
        raise BuildFailed(f"no modVersion in {mod}/build.py")
    return found.group(1)


async def _publish(tree: Path, mod: str, requests: list[ArtRequest], run: Run) -> str:
    project = await ModProject.find_one(ModProject.title == mod)
    if project is None:
        raise BuildFailed(f"no hub project titled {mod!r}")
    note = service.changelog(requests)
    await asyncio.to_thread(run, [sys.executable, "release.py", mod, "-m", note,
                                  "--silent", "--no-steam"], tree)
    version = _version(tree, mod)
    try:
        await asyncio.to_thread(_commit, run, tree, [f"{mod}/build.py"],
                                f"{_short(mod)}: record {version}")
    except BuildFailed as exc:
        run.say(f"WARNING the version bump did not reach origin: {exc}")
    if project.is_beta:
        run.say(f"{mod} is beta - hub only, Trovesaurus skipped")
    elif not await asyncio.to_thread(run, [sys.executable, "trovesaurus.py", mod, "-m", note,
                                           "--upload", "--silent"], tree, False):
        run.say(f"WARNING Trovesaurus upload failed for {mod} - send it by hand")
    for request in requests:
        request.versions[mod] = version
        await request.save()
    return f"{settings.app_url.rstrip('/')}/mods/{project.owner_handle}/{project.slug}"


async def _process(requests: list[ArtRequest]) -> None:
    env = {**os.environ, "TROVE_DIR": str(_root() / "trove")}
    run = Run(env)
    pages: dict[str, str] = {}
    try:
        tree = await asyncio.to_thread(_checkout, run)
        await _game_files(tree, run)
        await _place(tree, requests, run)
        await asyncio.to_thread(_commit, run, tree, list(ART), _art_message(requests))
        for mod in (service.CHAT, service.NAMEPLATE):
            mine = [r for r in requests if mod in service.MODS[r.kind] and mod not in r.versions]
            if mine:
                pages[mod] = await _publish(tree, mod, mine, run)
    except BuildFailed as exc:
        run.say(f"FAILED {exc}")
    except Exception as exc:
        logger.exception("custom art batch crashed")
        run.say(f"FAILED {type(exc).__name__}: {exc}")

    log = run.text()
    for request in requests:
        request.log = log
        if all(mod in request.versions for mod in service.MODS[request.kind]):
            request.status = "released"
            request.steam_pending = True
            request.released_at = utcnow()
            await request.save()
            await service.notify_released(request, pages)
        else:
            request.status = "failed"
            await request.save()


async def _claim() -> list[ArtRequest]:
    batch = secrets.token_hex(6)
    await ArtRequest.get_pymongo_collection().update_many(
        {"status": "queued"}, {"$set": {"status": "building", "batch": batch}})
    return await ArtRequest.find(ArtRequest.batch == batch,
                                 ArtRequest.status == "building").sort("+created_at").to_list()


async def main() -> None:
    configure_logging(logging.INFO)
    await init_db()
    await ArtRequest.get_pymongo_collection().update_many(
        {"status": "building"},
        {"$set": {"status": "failed", "log": "The worker restarted in the middle of this release."}})
    logger.info("custom art worker up, polling every %ss", settings.custom_art_poll_seconds)
    while True:
        try:
            batch = await _claim()
            if batch:
                logger.info("releasing %d custom art request(s)", len(batch))
                await _process(batch)
        except Exception:
            logger.exception("custom art worker iteration failed")
        await asyncio.sleep(settings.custom_art_poll_seconds)


if __name__ == "__main__":
    asyncio.run(main())
