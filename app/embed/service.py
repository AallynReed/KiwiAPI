"""Source resolution for the embeddable viewers.

The Mods Hub's viewers only ever previewed one thing: a release in our own hub.
The embed widens that to three **sources**, all rendered by exactly the same code:

  ``release=<id>``   a published Mods Hub release (public/unlisted only - a draft
                     is never previewable through an embed, which is anonymous)
  ``tmod=<token>``   a .tmod the partner uploaded to /v1/embed/tmod (app/embed/uploads.py)
  ``game=<path>``    a file in the live game tree (the updates archive), so a partner
                     can preview native game content they don't host at all
  ``dress=<outfit>`` a dressed character - ``class:costume:hat:face:weapon``, the
                     dressing room's own colon-joined selection of game prefab stems

The first two hand back ``.tmod`` bytes, and every hub helper here already takes
``tmod_bytes`` - so they're reused verbatim (``_list_blueprints_sync``,
``_tmod_pkfx_and_index``, ``_decode_blueprint_payload``, the VFX resolver/dep-set).
The game source is the only one that needs new plumbing: it reads one file out of
the CAS and, for a creature blueprint, pulls its SIBLING parts out of the game tree
so the whole creature assembles from a single path.

Security notes:
  - ``game`` is not an open game-file proxy. Only ``.blueprint`` and ``.pkfx`` are
    addressable; every other byte a viewer fetches (textures, atlases, nested
    effects) must appear in the effect's own recursively-resolved dependency set,
    exactly like the hub's ``/vfx/asset``.
  - No source can reveal anything a signed-out visitor couldn't already read from
    the hub or the updates archive.
"""

from __future__ import annotations

import asyncio
import base64
import logging
import re
import time
from collections.abc import Awaitable
from dataclasses import dataclass

from app.core.errors import APIError, ErrorCode
from app.embed import uploads
from app.trove import tmod as tmod_mod
from app.trove.mods_hub import service as hub
from app.trove.mods_hub import vfx
from app.trove.mods_hub.trove_layout import (
    LIVE_BRANCH,
    game_file_map,
    game_file_paths,
    nearest_path,
)
from app.trove.render import bp_cache

logger = logging.getLogger("kiwi.embed")

# Files a ``game=`` source may address directly. (Everything else in the tree is
# reachable only as a resolved dependency of a .pkfx, never by request.)
_GAME_EXTS = (".blueprint", ".pkfx")

_MAX_GAME_PATH = 400


def _bad(msg: str) -> APIError:
    return APIError(400, ErrorCode.bad_request, msg)


def _missing(msg: str) -> APIError:
    return APIError(404, ErrorCode.not_found, msg)


@dataclass(frozen=True)
class Source:
    """A resolved embed target. ``tmod`` is set for release/upload sources;
    ``game_path`` for a game-tree file. ``cache_key`` namespaces the shared VFX
    dependency-set cache so two sources can't authorise each other's assets."""

    kind: str                      # "release" | "tmod" | "game" | "dress"
    ident: str                     # release id / upload token / game path / outfit
    title: str                     # what the viewer shows in its header
    tmod: bytes | None = None
    game_path: str | None = None
    outfit: object | None = None   # a dressing_service.Outfit, for kind == "dress"
    # SHA-256 of `tmod` (an upload's token IS its hash). Keys the decoded-payload
    # cache for a hub release; an upload is deliberately never cached, since we
    # hold that file for its TTL and nothing longer.
    tmod_sha: str | None = None

    @property
    def cache_key(self) -> str:
        return f"{self.kind}:{self.ident}"


# ── source resolution ──────────────────────────────────────────────────────

async def resolve(
    *, release: str | None = None, tmod: str | None = None, game: str | None = None,
    dress: str | None = None,
) -> Source:
    """Resolve exactly one of the source params to a ``Source``."""
    given = [p for p in (release, tmod, game, dress) if p]
    if len(given) != 1:
        raise _bad("Pass exactly one source: release, tmod, game or dress.")

    if release:
        return await _release_source(release)
    if tmod:
        return await _upload_source(tmod)
    if dress:
        return await _dress_source(dress)
    return await _game_source(game or "")


_MAX_DRESS = 200


async def _dress_source(token: str) -> Source:
    """``class:costume:hat:face:weapon`` - each field a game prefab stem, trailing
    fields optional. Stems are Trove's own identifiers rather than row ids of ours, so
    a partner's link keeps meaning the same thing across game updates and rebuilds, and
    nothing has to be stored for it to resolve."""
    from app.trove.dressing import service as dressing

    token = (token or "").strip()
    if not token or len(token) > _MAX_DRESS:
        raise _bad("Missing or over-long outfit.")
    fields = (token.lower().split(":") + ["", "", "", ""])[:5]
    outfit = await dressing.resolve(
        fields[0], fields[1], {"hat": fields[2], "face": fields[3], "weapon": fields[4]})
    if outfit is None:
        raise _missing("No such Trove class to dress.")
    return Source(kind="dress", ident=outfit.ident,
                  title=f"{outfit.cls.name} - {outfit.costume.name}", outfit=outfit)


async def _release_source(release_id: str) -> Source:
    # viewer=None -> anonymous visibility: published public/unlisted releases only.
    rel, project = await hub.release_with_project(release_id, None)
    if rel.release_format != "tmod":
        raise _missing("That release has nothing to preview.")
    data = await hub.store.get_blob(rel.tmod_sha)
    if data is None:
        raise _missing("Release artifact not found.")
    return Source(kind="release", ident=release_id,
                  title=f"{project.title} {rel.tag}".strip(), tmod=data, tmod_sha=rel.tmod_sha)


async def _upload_source(token: str) -> Source:
    data = await uploads.load(token)
    if data is None:
        # Read by a VISITOR on someone else's page, who can't "re-upload" anything.
        # Reloading the host page IS the fix: it re-renders, which re-posts the mod
        # and mints a fresh token.
        raise _missing("This preview has expired. Reload the page to start it again.")
    return Source(kind="tmod", ident=token,
                  title=_tmod_title(data) or "Mod preview", tmod=data, tmod_sha=token)


def _tmod_title(data: bytes) -> str:
    """The mod's own header title - what the author named it, read straight out of
    the bytes. Nothing else about an upload is known to us: we hold the file and
    nothing beside it, so this is the only name there is."""
    try:
        props = tmod_mod.read_tmod(data, metadata_only=True).get("properties") or {}
    except tmod_mod.TmodError:
        return ""
    return str(props.get("title") or "").strip()


async def _game_source(path: str) -> Source:
    path = (path or "").strip().replace("\\", "/").lstrip("/")
    if not path or len(path) > _MAX_GAME_PATH:
        raise _bad("Missing or over-long game file path.")
    if not path.lower().endswith(_GAME_EXTS):
        raise _bad("Only .blueprint and .pkfx game files can be previewed.")
    resolved = await _resolve_game_path(path)
    if resolved is None:
        raise _missing(f"No '{path}' in the current game files.")
    return Source(kind="game", ident=resolved,
                  title=resolved.rsplit("/", 1)[-1], game_path=resolved)


async def _resolve_game_path(path: str) -> str | None:
    """Canonical archive path for a game file. Accepts a full path OR a bare
    filename - partners generally know 'dragon_head.blueprint', not the folder it
    lives in, and the archive already indexes the tree by basename."""
    from app.trove.updates import read as updates_read

    if await updates_read.get_file_meta(LIVE_BRANCH, path) is not None:
        return path
    fmap = await game_file_map(LIVE_BRANCH)
    return fmap.get(vfx.basename(path).lower())


async def _read_game_file(path: str) -> bytes | None:
    from app.trove.updates import read as updates_read

    meta = await updates_read.get_file_meta(LIVE_BRANCH, path)
    if not meta:
        return None
    return await _read_game_blob(meta["content_sha256"])


async def _read_game_blob(sha: str) -> bytes | None:
    """The archive blob for a known content hash. Split out from ``_read_game_file``
    so a caller that already resolved the hash (to key the payload cache) doesn't
    look the file up twice."""
    from app.core.config import settings
    from app.trove.updates.cas import ContentStore

    store = ContentStore(settings.trove_update_store_dir)
    return await asyncio.to_thread(store.get, sha)


# ── manifest (what this source can show) ───────────────────────────────────

async def manifest(src: Source) -> dict:
    """Everything the viewer needs to paint its picker: the previewable blueprints
    (with rig/assembly info) and .pkfx effects in this source."""
    if src.kind == "game":
        return await _game_manifest(src)
    if src.kind == "dress":
        return _dress_manifest(src)
    return await _tmod_manifest(src)


def _dress_manifest(src: Source) -> dict:
    """A dressed character is only ever the assembled model - there is no per-part
    picker to offer, and no bundled VFX."""
    from app.trove.mods_hub import assembly

    outfit = src.outfit
    rig = outfit.cls.skeleton                            # type: ignore[union-attr]
    return {
        "source": "dress", "title": src.title,
        "blueprints": {
            "items": [{"path": src.ident, "size": None, "assembled": True}],
            "rig": rig,
            "animations": assembly.animations_for(rig),
        },
        "vfx": {"items": []},
    }


async def _tmod_manifest(src: Source) -> dict:
    assert src.tmod is not None
    try:
        base = await asyncio.to_thread(hub._list_blueprints_sync, src.tmod)
        pkfx_items, _index = await asyncio.to_thread(hub._tmod_pkfx_and_index, src.tmod)
    except tmod_mod.TmodError:
        raise _bad("That file isn't a readable .tmod.") from None
    rig, anims, components = await hub._resolve_rig(base["fns"])
    for item, fn in zip(base["items"], base["fns"], strict=False):
        item["assembled"] = fn in components
    pkfx_items.sort(key=lambda f: f["path"].lower())
    return {
        "source": src.kind, "title": src.title,
        "blueprints": {"items": base["items"], "rig": rig, "animations": anims},
        "vfx": {"items": pkfx_items},
    }


async def _game_manifest(src: Source) -> dict:
    """A game source addresses exactly ONE file, so the manifest has a single item.
    For a blueprint that the binfab map binds to a creature we also advertise the
    rig, so the viewer can offer the assembled creature instead of a lone body part."""
    path = src.game_path or ""
    if path.lower().endswith(".pkfx"):
        return {"source": "game", "title": src.title,
                "blueprints": {"items": [], "rig": None, "animations": []},
                "vfx": {"items": [{"path": path, "size": None}]}}

    from app.trove.mods_hub import assembly, rig_index
    stem = vfx.basename(path).lower()[: -len(".blueprint")]
    # Same lookup ``_game_assembled`` will make, so the manifest can't advertise a
    # creature the assemble step then declines to build.
    skeleton, _prefab, _parts = await rig_index.creature_for(stem)
    rig = skeleton if skeleton and assembly.has_baked_rig(skeleton) else None
    return {
        "source": "game", "title": src.title,
        "blueprints": {
            "items": [{"path": path, "size": None, "assembled": bool(rig)}],
            "rig": rig,
            "animations": assembly.animations_for(rig) if rig else [],
        },
        "vfx": {"items": []},
    }


# ── blueprint / assembled model ────────────────────────────────────────────

async def blueprint(src: Source, path: str, fmt: str = "json") -> bp_cache.Cached:
    """Decoded voxel payload for one .blueprint (the 3D viewer's model format).

    Content we host - a hub release, a game file - is content-addressed, so the
    decode is cached and the first view of a model is the only one that pays for
    it, however many partner pages embed it afterwards. An **upload** is not: the
    file is held in Redis for its TTL and never written to disk, and a decoded
    copy of a mod is still the mod, so it's rebuilt on every view."""
    if src.kind == "game":
        return await _game_blueprint(src, path, fmt)
    if src.kind == "dress":
        raise _missing("A dressed character has no separate blueprint to show.")
    assert src.tmod is not None
    data = src.tmod
    def build() -> Awaitable[dict]:
        return asyncio.to_thread(hub._decode_blueprint_payload, data, path)

    if src.kind == "tmod" or not src.tmod_sha:
        return await bp_cache.build_uncached(build, fmt)
    return await bp_cache.get_or_build(bp_cache.key_for_tmod(src.tmod_sha, path), build, fmt)


async def _game_blueprint(src: Source, path: str, fmt: str = "json") -> bp_cache.Cached:
    from app.trove.updates import read as updates_read

    # A game source is pinned to its own file - the path param is only ever the
    # one the manifest advertised, so ignore anything else rather than turning
    # this into a general file reader.
    target = src.game_path or ""
    if path and path.strip().lower() not in (target.lower(), vfx.basename(target).lower()):
        raise _missing("That blueprint isn't part of this preview.")
    meta = await updates_read.get_file_meta(LIVE_BRANCH, target)
    if not meta:
        raise _missing("Game file not found.")
    sha = meta["content_sha256"]
    return await bp_cache.get_or_build(
        bp_cache.key_for_file(sha, target), lambda: _pack_game_blueprint(sha, target), fmt,
    )


async def _pack_game_blueprint(sha: str, target: str) -> dict:
    from app.trove.render.voxel import (
        BlueprintEmpty,
        BlueprintError,
        BlueprintTooLarge,
        pack_blueprint,
    )

    raw = await _read_game_blob(sha)
    if raw is None:
        raise _missing("Game file not found.")
    try:
        return await asyncio.to_thread(pack_blueprint, raw, target)
    except BlueprintTooLarge as e:
        raise APIError(413, ErrorCode.bad_request, str(e)) from None
    except BlueprintEmpty as e:
        raise APIError(422, ErrorCode.bad_request, str(e)) from None
    except BlueprintError as e:
        raise APIError(422, ErrorCode.bad_request, f"Couldn't read that blueprint: {e}") from None


async def assembled(src: Source, fmt: str = "json") -> bp_cache.Cached | None:
    """The source's blueprint parts assembled onto their creature rig, or None.

    Cached like a single blueprint - a hub release keys on the same artifact hash
    the hub itself uses, so both viewers share one assembly. An upload is rebuilt
    every view (we don't keep partner files, decoded or not)."""
    if src.kind == "game":
        return await _game_assembled(src, fmt)
    if src.kind == "dress":
        from app.trove.dressing import service as dressing

        return await dressing.model(src.outfit, fmt)     # type: ignore[arg-type]
    assert src.tmod is not None
    data = src.tmod
    from app.trove.mods_hub import assembly, rig_index

    async def build() -> dict:
        def _read(b: bytes):
            files = tmod_mod.read_tmod(b)["files"]
            names = [f["path"].split("/")[-1][: -len(".blueprint")].lower()
                     for f in files if f["path"].lower().endswith(".blueprint")]
            return files, names

        files, names = await asyncio.to_thread(_read, data)
        skeleton, attach = await rig_index.resolve(names)
        model = await asyncio.to_thread(
            lambda: assembly.assemble(files, rig_name=skeleton, ap_overrides=attach))
        if model is None:
            raise bp_cache.NoPayload
        return model

    sig = None if src.kind == "tmod" else await rig_index.index_signature()
    try:
        if sig is None or not src.tmod_sha:
            return await bp_cache.build_uncached(build, fmt)
        return await bp_cache.get_or_build(
            bp_cache.key_for_assembly(sig, f"tmod:{src.tmod_sha}"), build, fmt)
    except bp_cache.NoPayload:
        return None


async def _game_assembled(src: Source, fmt: str = "json") -> bp_cache.Cached | None:
    """Assemble a NATIVE creature from one blueprint path: resolve the part to the
    CREATURE it belongs to, then pull that creature's other parts out of the game tree.

    This is the piece a partner can't do themselves - they have a file path, and we
    have the binfab map that says which other files belong to the same creature.

    The creature comes from ``rig_index.creature_for``, i.e. from the one prefab that
    binds this part. It deliberately does NOT come from the skeleton: a skeleton is
    shared by every variant that uses it (``mount_raptor`` covers every raptor mount in
    the game), so taking its parts assembled all of them at once - each attach point
    claimed by whichever variant happened to sort first, which rendered a chimera and
    dropped the requested file from its own preview."""
    path = src.game_path or ""
    if not path.lower().endswith(".blueprint"):
        return None
    from app.trove.mods_hub import assembly, rig_index

    stem = vfx.basename(path).lower()[: -len(".blueprint")]
    skeleton, prefab, parts = await rig_index.creature_for(stem)
    if not skeleton or not prefab or not parts:
        return None                      # part not bound to a creature -> no guess, no render
    if not assembly.has_baked_rig(skeleton):
        return None                      # unknown rig -> no guess, no render

    async def build() -> dict:
        # Resolve each part against the CREATURE's own prefab. Trove reuses filenames
        # across skins and NPC sets, so picking by archive order once put a merchant-hub
        # NPC on a costume's head attach point.
        paths = await game_file_paths(LIVE_BRANCH)
        files: list[dict] = []
        for basename in parts:
            gp = nearest_path(paths.get(f"{basename}.blueprint", []), prefab)
            if not gp:
                continue
            raw = await _read_game_file(gp)
            if raw is None:
                continue                 # a part we don't have -> the rest still assembles
            files.append({"path": f"{basename}.blueprint",
                          "content_base64": base64.b64encode(raw).decode()})
        if not files:
            raise bp_cache.NoPayload
        model = await asyncio.to_thread(
            lambda: assembly.assemble(files, rig_name=skeleton, ap_overrides=parts))
        if model is None:
            raise bp_cache.NoPayload
        return model

    # Reading and placing every part of a creature is the most expensive thing the
    # embed does, and the answer only moves when the game files or the rig map do -
    # both of which the signature tracks. The PREFAB is the creature's identity, so
    # every partner page pointing at any of its parts shares one assembly - and two
    # creatures on one skeleton no longer share (and overwrite) each other's.
    sig = await rig_index.index_signature()
    try:
        if sig is None:
            return await bp_cache.build_uncached(build, fmt)
        return await bp_cache.get_or_build(
            bp_cache.key_for_assembly(sig, f"game:{prefab}"), build, fmt)
    except bp_cache.NoPayload:
        return None


# ── VFX ────────────────────────────────────────────────────────────────────

async def vfx_manifest(src: Source, path: str) -> dict:
    """One effect's .pkfx text + its dependencies, each classified mod/game/missing.
    Also primes the dependency-set cache that authorises ``vfx_asset``."""
    index, pkfx_text = await _vfx_index_and_text(src, path)
    lookup, read, available = await hub._game_vfx_resolver()

    deps: list[dict] = []
    for ref in vfx.extract_refs(pkfx_text):
        bn = vfx.basename(ref).lower()
        source = "mod" if bn in index else ("game" if lookup(bn) else "missing")
        deps.append({"ref": ref, "basename": vfx.basename(ref), "source": source})

    hub._vfx_remember_depset(
        src.cache_key, await hub._build_vfx_depset(index, lookup, read))
    return {
        "path": path, "pkfx": pkfx_text, "deps": deps,
        "missing": [d["basename"] for d in deps if d["source"] == "missing"],
        "game_available": available,
    }


async def vfx_asset(src: Source, ref: str) -> tuple[bytes, str]:
    """Bytes of one asset the source's VFX references - bundled first, else the game
    tree. Authorised against the recursively-resolved dependency set, so this is
    never an open proxy into the game files."""
    index, _text = await _vfx_index_and_text(src, None)
    lookup, read, _available = await hub._game_vfx_resolver()
    depset = hub._VFX_DEPSET_CACHE.get(src.cache_key)
    if depset is None:
        depset = await hub._build_vfx_depset(index, lookup, read)
        hub._vfx_remember_depset(src.cache_key, depset)

    bn = vfx.basename(ref).lower()
    if bn not in depset and bn not in index:
        raise _missing("Asset not referenced by this effect.")
    raw = index.get(bn)
    if raw is None:
        gp = lookup(bn)
        raw = await read(gp) if gp else None
    if raw is None:
        raise _missing("Asset not found (not bundled and not in the game files).")
    return raw, vfx.media_type_for(ref)


async def _vfx_index_and_text(src: Source, path: str | None) -> tuple[dict[str, bytes], str]:
    """``(basename -> bytes bundled with the source, .pkfx text)``.

    A game source bundles nothing, so its index holds only the effect itself and
    every dependency resolves from the game tree."""
    if src.kind == "game":
        target = src.game_path or ""
        raw = await _read_game_file(target)
        if raw is None:
            raise _missing("Effect not found in the game files.")
        index = {vfx.basename(target).lower(): raw}
        return index, raw.decode("utf-8", "replace")

    assert src.tmod is not None
    try:
        _items, index = await asyncio.to_thread(hub._tmod_pkfx_and_index, src.tmod)
    except tmod_mod.TmodError:
        raise _bad("That file isn't a readable .tmod.") from None
    if path is None:
        return index, ""
    pkfx_raw = index.get(vfx.basename(path).lower())
    if pkfx_raw is None:
        raise _missing("Effect not found in this mod.")
    return index, pkfx_raw.decode("utf-8", "replace")


# ── embedding origins ──────────────────────────────────────────────────────

# A CSP source expression we're willing to emit: scheme + host (+ optional port),
# with at most one leading '*.' wildcard label. Anything else - a path, a bare host,
# a stray quote, a control character - is dropped rather than concatenated into a
# response header we'd then be trusting an admin to have typed perfectly.
_ORIGIN_RE = re.compile(
    r"^https?://(\*\.)?[a-z0-9-]+(\.[a-z0-9-]+)*(:\d{1,5})?$", re.IGNORECASE,
)


_ORIGINS_TTL = 30.0
_origins_cache: dict = {"at": -1e9, "value": None}


async def allowed_origins() -> list[str]:
    """The origins the admin has allowed to iframe the viewer. Empty = nobody.

    Runs from the security middleware on every ``/embed/viewer`` response, so it's
    memoised ~30s and **never raises**.

    Two containers, one answer. The API reads the setting in-process. The website
    container - which is where the framable page is actually served - has no database
    at all (app/web/main.py), so it asks the API, exactly as it does for feature
    flags. An unguarded read there used to 500 every request before routing.

    Unlike the feature flags, this fails **CLOSED**: it's the whole access control
    for framing, and "we couldn't check" must never read as "yes". Worst case a
    partner's embed goes blank for 30s; the alternative is letting anyone frame it
    during a blip.
    """
    now = time.monotonic()
    cached = _origins_cache["value"]
    if cached is not None and now - float(_origins_cache["at"]) <= _ORIGINS_TTL:
        return cached

    parsed = _parse_origins(await _read_allowlist())
    _origins_cache["at"], _origins_cache["value"] = now, parsed
    return parsed


async def _read_allowlist() -> str:
    """The raw setting - locally if this process has a database, else from the API."""
    from app.admin import runtime_config

    try:
        return str(await runtime_config.get_setting("embed.allowed_origins") or "")
    except Exception:                       # no DB here (website container), or a blip
        pass
    from app.core.internal_api import internal_get

    data = await internal_get("/site/embed/allowed-origins")   # never raises
    if isinstance(data, dict) and isinstance(data.get("origins"), list):
        return " ".join(str(o) for o in data["origins"])
    logger.warning("embed: allowlist unreadable - framing denied until it returns")
    return ""


def _parse_origins(raw: str) -> list[str]:
    """Space- or comma-separated entries; malformed ones dropped (see ``_ORIGIN_RE``)
    so a bad character can never break or widen the CSP."""
    seen: list[str] = []
    for part in raw.replace(",", " ").split():
        origin = part.strip().rstrip("/")
        if _ORIGIN_RE.match(origin) and origin not in seen:
            seen.append(origin)
    return seen
