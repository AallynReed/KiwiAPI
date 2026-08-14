"""Mod Workshop - unpack and build ``.tmod`` files from whatever a browser hands over.

The engine behind the ``/mod-workshop`` page. Everything here is STATELESS: bytes
arrive with the request, the answer goes back with the response, nothing is written
down and nothing is kept.

It is the Mods Hub's own machinery pointed at loose files instead of a repo - the
same placement rules (``trove_layout``) that decide what a hub release compiles, and
the same reader/builder (``tmod``) that parses and produces the artifact - so a mod
built here is structurally identical to one released through the hub.

The placement pass is the point of the page. Trove only loads an override that sits
at the exact path the base game keeps it at, so nothing is packed before every file
is sorted:

  - **ready** - inside a Trove folder, nothing in the game contradicts it.
  - **moved** - the file's NAME matches a real game file but its path doesn't, so it
    would override nothing where it sits. We know where the game keeps it, so it goes
    there. This is what turns a folder of loose ``.blueprint`` files into a mod.
  - **misplaced** - the same, but the modder opted this one out (a custom file can
    legitimately share a name with a game file, and a wrong "fix" is worse than none).
  - **skipped** - outside every Trove folder, an ignored type, or beaten to its
    destination by another file.

The misplaced check needs the updates archive populated; without it the pure path
rules still run and the report says the game index was unavailable.
"""

from __future__ import annotations

import base64
import io
import logging
import re
import zipfile
from collections.abc import Iterable, Sequence

from app.trove import tmod
from app.trove.mods_hub import trove_layout

logger = logging.getLogger("kiwi.mods.workshop")

# What one request may unpack. The route's body cap bounds the upload; these bound
# what a small archive is allowed to *declare* (a zip bomb is 40 KB on the wire).
MAX_FILES = 4000
MAX_UNPACKED_BYTES = 192 * 1024 * 1024
# How deep a wrapper-folder chain to look through ("MyMod/v2/blueprints/…").
MAX_WRAPPER_DEPTH = 4
# Archive housekeeping that is never part of a mod and would otherwise show up as
# "skipped" noise (and break wrapper detection by adding a second top-level folder).
_JUNK_NAMES = frozenset({".ds_store", "thumbs.db", "desktop.ini", ".gitkeep"})
_JUNK_DIRS = ("__macosx/", ".git/", ".svn/")

_TITLE_ILLEGAL = re.compile(r'[<>:"/\\|?*\x00-\x1f]')
_CONTROL = re.compile(r"[\x00-\x08\x0b\x0c\x0e-\x1f]")

# What a mod's preview image may be. WebP is deliberately absent: Trove can't render
# one, so it must never be baked into a .tmod (the Mods Hub refuses it for the same
# reason). A preview is a thumbnail; anything bigger than this is the wrong file.
PREVIEW_EXTENSIONS: dict[str, str] = {
    ".png": "png", ".jpg": "jpg", ".jpeg": "jpg", ".gif": "gif",
}
MAX_PREVIEW_BYTES = 8 * 1024 * 1024


class WorkshopError(ValueError):
    """Bad input from the page (the router maps it to a 400)."""


# --- paths -----------------------------------------------------------------


def norm_path(path: str) -> str:
    """Posix, no leading slash, no ``.`` / ``..`` segments. A Windows file picker
    hands over backslashes and a zip's entry names are attacker-controlled text."""
    parts = [p for p in str(path or "").replace("\\", "/").split("/") if p and p != "."]
    return "/".join(p for p in parts if p != "..")


def _ext(path: str) -> str:
    name = path.rsplit("/", 1)[-1]
    return ("." + name.rsplit(".", 1)[1].lower()) if "." in name else ""


def is_junk(path: str) -> bool:
    """Archive housekeeping - not a mod file under any reading."""
    low = path.lower()
    return (low.rsplit("/", 1)[-1] in _JUNK_NAMES
            or any(low == d[:-1] or low.startswith(d) for d in _JUNK_DIRS))


def strip_wrapper(paths: Sequence[str]) -> tuple[str, list[str]]:
    """Drop the wrapper folder a downloaded mod usually carries
    (``MyMod/blueprints/…``), returning ``(prefix, rewritten paths)``.

    Only ever strips a top-level folder that ISN'T one of Trove's own: a mod whose
    root already *is* ``blueprints/`` must be left exactly where it is, and that is
    the case a naive common-prefix strip quietly destroys."""
    prefix = ""
    current = list(paths)
    for _ in range(MAX_WRAPPER_DEPTH):
        live = [p for p in current if p]
        if not live:
            break
        tops = {p.split("/", 1)[0] for p in live}
        if len(tops) != 1:
            break
        top = tops.pop()
        if top.lower() in trove_layout.TROVE_DIRECTORIES:
            break
        if not all("/" in p for p in live):   # a lone root file isn't a wrapper
            break
        prefix += top + "/"
        current = [p.split("/", 1)[1] if "/" in p else p for p in current]
    return prefix, current


# --- the placement plan ----------------------------------------------------


async def plan(paths: Sequence[str], *, fix: bool = True,
               keep: Iterable[str] = ()) -> dict:
    """Sort ``paths`` into what will and won't be packed, and where each one lands.

    ``fix`` moves misplaced files to the game's path for them; ``keep`` names the
    ones to leave alone anyway (by their original path). The result is the single
    source of truth for both the preview and the build - the build re-runs this over
    the same paths rather than trusting a mapping sent back by the page."""
    originals = [norm_path(p) for p in paths]
    prefix, rel = strip_wrapper(originals)
    keep_set = {norm_path(k) for k in keep}
    # The folder rules need no game data; only "is this the game's own path for it"
    # does. If that lookup is unavailable the page says so and still works, so a
    # database blip costs the placement hints, never the compiler.
    try:
        game_map = await trove_layout.game_file_map()
    except Exception:
        logger.warning("mod workshop: game file index unavailable", exc_info=True)
        game_map = {}
    misplaced = {m["path"]: m["expected"]
                 for m in trove_layout.find_misplaced(rel, game_map)}

    entries: list[dict] = []
    mapping: dict[int, str] = {}
    taken: dict[str, str] = {}          # final path -> the file that claimed it
    for i, (original, path) in enumerate(zip(originals, rel, strict=True)):
        entry: dict = {"index": i, "path": original, "name": path.rsplit("/", 1)[-1]}
        if not path or is_junk(path):
            entries.append({**entry, "status": "skipped",
                            "reason": "archive housekeeping, not a mod file"})
            continue
        expected = misplaced.get(path)
        if expected:
            entry["expected"] = expected
        move = bool(expected) and fix and original not in keep_set
        final = expected if move else path

        if not trove_layout.is_compilable(final):
            entries.append({**entry, "status": "skipped",
                            "reason": trove_layout.skip_reason(final)})
            continue
        clash = taken.get(final)
        if clash is not None:
            entries.append({**entry, "status": "skipped",
                            "reason": f"'{clash}' already lands at {final}"})
            continue

        taken[final] = original
        mapping[i] = final
        entries.append({**entry, "final": final,
                        "status": "moved" if move else
                                  ("misplaced" if expected else "ready")})

    counts = dict.fromkeys(("ready", "moved", "misplaced", "skipped"), 0)
    for e in entries:
        counts[e["status"]] += 1
    return {
        "wrapper": prefix,
        "total": len(entries),
        "entries": entries,
        "counts": counts,
        "packed": len(mapping),
        "mapping": mapping,
        "game_index_available": bool(game_map),
        "buildable": bool(mapping),
    }


async def preview(paths: Sequence[str]) -> dict:
    """The plan with automatic placement ON, plus - for every file placement would
    move - what becomes of it if it's left alone (as ``alt`` on that entry).

    Both answers go out together so the page's "put things where the game keeps
    them" switch, and the per-file opt-out beside each moved row, are instant. A
    ``.zip`` is uploaded once to ask the question, not once per toggle."""
    main = await plan(paths, fix=True)
    if not any(e.get("expected") for e in main["entries"]):
        return main
    other = {e["index"]: e for e in (await plan(paths, fix=False))["entries"]}
    for entry in main["entries"]:
        if not entry.get("expected"):
            continue
        alt = other.get(entry["index"], {})
        entry["alt"] = {k: alt[k] for k in ("status", "final", "reason") if k in alt}
    return main


# --- reading what was handed over ------------------------------------------


def read_zip(data: bytes) -> list[tuple[str, bytes]]:
    """Every real file in a ``.zip``, as ``(path, bytes)``. Directory entries and
    archive housekeeping are dropped here so they never reach the report."""
    out: list[tuple[str, bytes]] = []
    try:
        with zipfile.ZipFile(io.BytesIO(data)) as zf:
            infos = [i for i in zf.infolist()
                     if not i.is_dir() and not is_junk(norm_path(i.filename))]
            if len(infos) > MAX_FILES:
                raise WorkshopError(
                    f"That .zip holds more than {MAX_FILES} files - too many for one mod.")
            total = sum(i.file_size for i in infos)
            if total > MAX_UNPACKED_BYTES:
                raise WorkshopError(
                    f"That .zip unpacks to more than {MAX_UNPACKED_BYTES // (1024 * 1024)} MB.")
            for info in infos:
                path = norm_path(info.filename)
                if path:
                    out.append((path, zf.read(info)))
    except (zipfile.BadZipFile, RuntimeError, OSError) as e:
        raise WorkshopError(f"That .zip couldn't be opened: {e}") from e
    if not out:
        raise WorkshopError("That .zip has no files in it.")
    return out


def read_mod(data: bytes) -> tuple[dict[str, str], list[tuple[str, bytes]]]:
    """A ``.tmod``'s header properties plus its packed files."""
    try:
        parsed = tmod.read_tmod(data)
    except tmod.TmodError as e:
        raise WorkshopError(f"That isn't a readable .tmod file: {e}") from e
    props = {str(k): str(v) for k, v in (parsed.get("properties") or {}).items()}
    files = [(norm_path(f["path"]), base64.b64decode(f["content_base64"] or ""))
             for f in parsed.get("files", [])]
    return props, [(p, b) for p, b in files if p]


def describe(data: bytes) -> dict:
    """A ``.tmod`` taken apart: the header the game and the mod sites read off it,
    the category flags decoded back into labels, and every packed file with its size.

    The same breakdown a Mods Hub release shows under Contents (``inspect_release``),
    for a file that was never uploaded anywhere. Metadata-only - the file table lives
    in the header, so nothing is decompressed to answer this."""
    from app.trove.mods_hub.service import _declared_config_path, _preview_path

    try:
        parsed = tmod.read_tmod(data, metadata_only=True)
    except tmod.TmodError as e:
        raise WorkshopError(f"That isn't a readable .tmod file: {e}") from e
    props = {str(k): str(v) for k, v in (parsed.get("properties") or {}).items()}
    files = sorted(({"path": f["path"], "size": f["size"]} for f in parsed["files"]),
                   key=lambda f: str(f["path"]).lower())
    return {
        "version": parsed.get("version"),
        "properties": props,
        "categories": parsed.get("categories") or [],
        "flags": parsed.get("flags") or 0,
        "preview_path": _preview_path(props),
        "config_path": _declared_config_path(props),
        "files": files,
        "file_count": len(files),
        "size": len(data),
        "total_size": sum(int(f["size"] or 0) for f in files),
    }


def looks_like_zip(data: bytes, filename: str = "") -> bool:
    return data[:2] == b"PK" or (filename or "").lower().endswith(".zip")


def read_archive(data: bytes, filename: str = "",
                 ) -> tuple[str, dict[str, str], list[tuple[str, bytes]]]:
    """Unpack a ``.zip`` or an existing ``.tmod`` into ``(kind, header, files)``.

    Taking a ``.tmod`` here is what makes the page a repair shop as well as a
    compiler: a mod whose files sit at the wrong paths can be opened, re-placed and
    rebuilt without ever leaving the browser tab."""
    if not data:
        raise WorkshopError("That file is empty.")
    if looks_like_zip(data, filename):
        return "zip", {}, read_zip(data)
    props, files = read_mod(data)
    if not files:
        raise WorkshopError("That .tmod has no files packed in it.")
    return "tmod", props, files


# --- writing --------------------------------------------------------------


def to_zip(files: Sequence[tuple[str, bytes]]) -> bytes:
    """Pack ``(path, bytes)`` into a ``.zip`` - the extract half of the page."""
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as zf:
        for path, content in files:
            zf.writestr(path, content)
    return buf.getvalue()


def safe_title(value: str | None) -> str:
    """A mod title that can also be a filename. Trove matches a mod by the ``title``
    in its header and the file on disk must be named after it, so the two are
    sanitised the same way and only ever once - here (see the Mods Hub's
    ``release_download_filename``, which restores exactly this name)."""
    cleaned = _TITLE_ILLEGAL.sub("", str(value or "")).strip().strip(".")
    return cleaned[:120] or "My Mod"


def _clean(value: object, limit: int, *, multiline: bool = False) -> str:
    text = _CONTROL.sub("", str(value or ""))
    if not multiline:
        text = text.replace("\n", " ").replace("\r", " ")
    return text.strip()[:limit]


def clean_properties(raw: dict | None) -> dict[str, str]:
    """The header a hand-built mod carries. ``title`` is the one field that is never
    allowed to be empty - it is how Trove identifies the mod - so it falls back
    rather than failing. ``modLoader`` is stamped by the builder, not by us."""
    raw = raw or {}
    props: dict[str, str] = {"title": safe_title(raw.get("title"))}
    for key, source, limit in (
        ("author", "author", 200),
        ("modVersion", "modVersion", 40),
    ):
        value = _clean(raw.get(source), limit)
        if value:
            props[key] = value
    notes = _clean(raw.get("notes"), 4000, multiline=True)
    if notes:
        props["notes"] = notes
    raw_tags = raw.get("tags") or []
    # A .tmod opened for rebuild carries its tags as one comma-separated string; the
    # page sends a list. `flags` is deliberately NOT carried over - build_tmod derives
    # it from these tags, so an edited category set can't leave a stale bitmask behind.
    if isinstance(raw_tags, str):
        raw_tags = raw_tags.split(",")
    tags = ", ".join(t for t in (_clean(x, 40) for x in raw_tags) if t)[:400]
    if tags:
        props["tags"] = tags
    # Which packed file IS the preview / the config. Carried over from an opened
    # .tmod so a rebuild doesn't quietly lose them; build_mod drops either one whose
    # file didn't survive the build, and overwrites it when a new one is attached.
    for key in ("previewPath", "configPath"):
        declared = norm_path(_clean(raw.get(key), 260)).lower()
        if declared:
            props[key] = declared
    return props


def config_candidates(paths: Sequence[str]) -> list[str]:
    """The ``.cfg`` files among ``paths`` that could be the build's config.

    A ``.cfg`` is stripped by the ordinary placement rules (a repo is full of
    unrelated ones), so packing one is always a deliberate choice - and only a mod
    with a Flash UI has anything that would read it, which is why an answer at all
    depends on a ``.swf`` being in the same pile."""
    names = [norm_path(p) for p in paths]
    if not any(p.lower().endswith(".swf") for p in names):
        return []
    return [p for p in names if p.lower().endswith(".cfg")]


def preview_candidates(paths: Sequence[str]) -> list[str]:
    """The images among ``paths`` that could be the mod's preview.

    Any image will do wherever it sits - a ``preview.png`` at the root is the usual
    one, and the placement rules skip it - because the chosen file is re-packed at
    the path Trove reads a preview from rather than the one it arrived at."""
    return [p for p in (norm_path(x) for x in paths) if _ext(p) in PREVIEW_EXTENSIONS]


def _pick(files: Sequence[tuple[str, bytes]], path: str, what: str) -> bytes:
    """One named file out of the pile, by the path it arrived at."""
    data = next((d for p, d in files if p == path), None)
    if data is None:
        raise WorkshopError(f"No file '{path}' to pack as the {what}.")
    return data


async def build_mod(
    files: Sequence[tuple[str, bytes]], properties: dict | None, *,
    fix: bool = True, keep: Iterable[str] = (), config_path: str = "",
    preview_path: str = "", attached: Sequence[tuple[str, bytes]] = (),
) -> tuple[bytes, dict]:
    """Place, filter and pack ``files`` into a ``.tmod``. Returns ``(bytes, plan)``.

    The plan is recomputed here from the paths themselves rather than trusting one
    the page sends back, so what gets built is always what the preview described.

    ``config_path`` and ``preview_path`` name the mod's settings file and its preview
    image. Neither is placed by the ordinary rules - both are re-packed at the path
    Trove reads them from, named after the mod - so either one can be a file the
    build would otherwise skip, like a ``preview.png`` sitting at the root.

    ``attached`` holds files that came from outside the mod (picked off a computer
    rather than out of the folder). They take no part in placement at all: a loose
    file dropped in beside a mod that lives under one wrapper folder would otherwise
    look like a second root, and the wrapper would stop being one."""
    from app.trove.mods_hub.service import _inject_config, pack_preview

    result = await plan([p for p, _ in files], fix=fix, keep=keep)
    if not result["buildable"]:
        raise WorkshopError(
            "Nothing here would load in-game. Trove only reads files inside its own "
            "folders (blueprints/, ui/, prefabs/, textures/, …) - put the mod's files "
            "under one of those, or let the workshop place them for you.")
    packed = [(result["mapping"][i], data)
              for i, (_, data) in enumerate(files) if i in result["mapping"]]
    props = clean_properties(properties)

    # A mod opened for rebuild re-attaches what it already carried, unless something
    # else was picked. A .cfg is stripped by the ordinary placement rules, so without
    # this a mod would lose its settings file just by passing through here again -
    # and a renamed mod gets both files renamed with it.
    available = {p for p, _ in files}
    config_path = norm_path(config_path) or (
        props["configPath"] if props.get("configPath") in available else "")
    preview_path = norm_path(preview_path) or (
        props["previewPath"] if props.get("previewPath") in available else "")
    # A file picked off a computer wins a name it shares with one of the mod's own.
    pickable = list(attached) + list(files)

    if config_path:
        # Same rule and same packed path as a Mods Hub release, so a mod that moves
        # between the two carries its config identically.
        _inject_config(packed, props, _pick(pickable, config_path, "config"))

    if preview_path:
        ext = PREVIEW_EXTENSIONS.get(_ext(preview_path))
        if not ext:
            raise WorkshopError(
                "A preview image has to be a PNG, JPG or GIF - Trove can't show anything else.")
        image = _pick(pickable, preview_path, "preview")
        if not image:
            raise WorkshopError("That preview image is empty.")
        if len(image) > MAX_PREVIEW_BYTES:
            raise WorkshopError(
                f"A preview image can be at most {MAX_PREVIEW_BYTES // (1024 * 1024)} MB.")
        pack_preview(packed, props, image, ext, props["title"])

    # A declaration carried over from an opened .tmod outlives its file if placement
    # dropped it, and a header pointing at a file that isn't there reads as a broken
    # mod. Paths are lowercased on the way into the archive, so compare that way.
    final_paths = {p.lower() for p, _ in packed}
    for key in ("previewPath", "configPath"):
        if props.get(key) and props[key].lower() not in final_paths:
            props.pop(key)

    return tmod.build_tmod(1, props, packed), result
