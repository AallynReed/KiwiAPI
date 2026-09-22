"""The custom art release worker: ``python -m app.custom_art.builder``.

Runs as its own container on the api image. It claims whatever the master has
queued and, for the whole batch:

1. brings its TroveUI checkout to the tip of the bare repo, and ``lib`` to KiwiZUI,
2. puts the game's own vanilla SWFs and English language files in from the updates
   archive, so the builds verify against what the live client ships,
3. places each picture with TroveUI's ``custom_art.py``, commits and pushes that
   together with the untouched file that was submitted, which ``custom_art.py`` keeps
   under ``originals/`` - the shipped picture is a lossy render and nothing can be
   re-derived from it, so the original is the only way a later quality change goes
   either way,
4. releases each affected mod silently to the hub, and to Trovesaurus unless the
   mod is beta, then commits and pushes the version bump,
5. marks each request released and emails the person who asked.

Steam is left to the master, so a released request stays ``steam_pending``. A
failure marks what did not ship ``failed`` with the log; the checkout is reset on
the next run, so a retry starts clean and skips mods already released.
"""
import asyncio
import json
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
    service.CLUBS: ("ui/clubs.swf", "clubs.vanilla.swf"),
}
ART = tuple(f"{mod}/art" for mod in service.MODS) + ("originals",)
_TIMEOUT = 1800
_LOG_MAX = 60_000

STEAMCMD = "/opt/steamcmd/steamcmd.sh"
STEAM_HOME = "/home/steam"
STEAM_APP = "304050"


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

    def __call__(self, args: list[str], cwd: Path, check: bool = True,
                 env: dict[str, str] | None = None) -> bool:
        self.say("$ " + " ".join(args))
        try:
            done = subprocess.run(args, cwd=cwd, env={**self.env, **(env or {})},
                                  capture_output=True, text=True, timeout=_TIMEOUT)
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
    wanted = [(path, tree / mod / name) for mod, (path, name) in VANILLA.items()]
    wanted.append(("Trove_x64.exe", _root() / "trove" / "Trove_x64.exe"))
    for path, dest in wanted:
        row = await UpdateState.find_one({"branch": branch, "path": path})
        if row is None:
            raise BuildFailed(f"{path} is not archived on {branch}")
        copy(row.content_sha256, dest)
    run.say(f"{len(rows)} language files, the client and {len(VANILLA)} vanilla SWFs from {branch}")


async def _place(tree: Path, requests: list[ArtRequest], run: Run) -> None:
    incoming = _root() / "incoming"
    shutil.rmtree(incoming, ignore_errors=True)
    incoming.mkdir(parents=True)
    for request in requests:
        for slot, picture in request.pictures.items():
            lane = service.LANES[(request.kind, slot)]
            data = await store.get_blob(picture.sha)
            if data is None:
                raise BuildFailed(f"the {service.LANE_LABELS[lane]} for {request.name!r} "
                                  f"is missing from the store")
            source = incoming / f"{request.id}-{slot}"
            source.write_bytes(data)
            await asyncio.to_thread(run, [sys.executable, "custom_art.py", lane,
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


def _steam(tree: Path, mod: str, note: str, run: Run) -> bool:
    """Push the .tmod that was just released to the Steam Workshop.

    Never fatal. The hub and Trovesaurus already have the build by the time this
    runs, so a Steam failure is a platform that is behind rather than a release
    that did not happen - it warns, leaves the request ``steam_pending`` and the
    master pushes that one by hand.

    The artifact is the mod's own ``dist`` copy, which is the file release.py just
    uploaded, so all three platforms carry the same bytes. The login is whatever
    is in the mounted volume; steamcmd is told where to find it through HOME
    rather than the worker running under it, because git would read that too."""
    user = settings.custom_art_steam_user
    if not user:
        run.say(f"WARNING no Steam account configured - {mod} not sent to Steam")
        return False
    ids = tree / "steam_ids.json"
    item = json.loads(ids.read_text(encoding="utf-8")).get(mod) if ids.is_file() else None
    if item is None:
        run.say(f"WARNING {mod} has no Steam id in {ids.name} - not sent to Steam")
        return False
    art = tree / mod / "dist" / f"{mod}.tmod"
    if not art.is_file():
        run.say(f"WARNING {mod} has no built .tmod - not sent to Steam")
        return False

    work = Path(settings.custom_art_workspace) / "steam"
    shutil.rmtree(work, ignore_errors=True)
    (work / "content").mkdir(parents=True)
    shutil.copyfile(art, work / "content" / art.name)
    vdf = work / "item.vdf"
    vdf.write_text('"workshopitem"\n{\n'
                   f'\t"appid"\t\t"{STEAM_APP}"\n'
                   f'\t"publishedfileid"\t"{item}"\n'
                   f'\t"contentfolder"\t"{(work / "content").as_posix()}"\n'
                   f'\t"changenote"\t"{_vdf_safe(note)}"\n'
                   "}\n", encoding="utf-8")

    ok = run([STEAMCMD, "+login", user, "+workshop_build_item", str(vdf), "+quit"],
             work, check=False, env={"HOME": STEAM_HOME})
    tail = run.lines[-1] if run.lines else ""
    if not ok or "Success." not in tail:
        run.say(f"WARNING Steam upload failed for {mod} - push it by hand")
        return False
    run.say(f"{mod} is on the Steam Workshop, item {item}")
    return True


def _vdf_safe(text: str) -> str:
    return text.replace("\\", " ").replace('"', "'").replace("\n", " ")


async def _publish(tree: Path, mod: str, requests: list[ArtRequest], run: Run,
                   steamed: dict[str, bool]) -> str:
    project = await ModProject.find_one(ModProject.title == mod)
    if project is None:
        raise BuildFailed(f"no hub project titled {mod!r}")
    note = service.changelog(requests, mod)
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
    steamed[mod] = await asyncio.to_thread(_steam, tree, mod, note, run)
    for request in requests:
        request.versions[mod] = version
        await request.save()
    return f"{settings.app_url.rstrip('/')}/mods/{project.owner_handle}/{project.slug}"


async def _process(requests: list[ArtRequest]) -> None:
    env = {**os.environ, "TROVE_DIR": str(_root() / "trove")}
    run = Run(env)
    pages: dict[str, str] = {}
    steamed: dict[str, bool] = {}
    try:
        tree = await asyncio.to_thread(_checkout, run)
        await _game_files(tree, run)
        await _place(tree, requests, run)
        await asyncio.to_thread(_commit, run, tree, list(ART), _art_message(requests))
        for mod in service.MODS:
            mine = [r for r in requests if mod in service.mods_of(r) and mod not in r.versions]
            if mine:
                pages[mod] = await _publish(tree, mod, mine, run, steamed)
    except BuildFailed as exc:
        run.say(f"FAILED {exc}")
    except Exception as exc:
        logger.exception("custom art batch crashed")
        run.say(f"FAILED {type(exc).__name__}: {exc}")

    log = run.text()
    for request in requests:
        request.log = log
        if all(mod in request.versions for mod in service.mods_of(request)):
            request.status = "released"
            request.steam_pending = not all(steamed.get(mod)
                                            for mod in service.mods_of(request))
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
