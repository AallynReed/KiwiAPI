"""Source resolution for the embeddable viewers.

The Mods Hub's viewers only ever previewed one thing: a release in our own hub.
The embed widens that to three **sources**, all rendered by exactly the same code:

  ``release=<id>``   a published Mods Hub release (public/unlisted only - a draft
                     is never previewable through an embed, which is anonymous)
  ``tmod=<token>``   a .tmod the partner uploaded to /v1/embed/tmod (app/embed/uploads.py)
  ``game=<path>``    a file in the live game tree (the updates archive), so a partner
                     can preview native game content they don't host at all

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
import re
from dataclasses import dataclass

from app.core.errors import APIError, ErrorCode
from app.embed import uploads
from app.trove import tmod as tmod_mod
from app.trove.mods_hub import service as hub
from app.trove.mods_hub import vfx
from app.trove.mods_hub.trove_layout import LIVE_BRANCH, game_file_map

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

    kind: str                      # "release" | "tmod" | "game"
    ident: str                     # release id / upload token / game path
    title: str                     # what the viewer shows in its header
    tmod: bytes | None = None
    game_path: str | None = None

    @property
    def cache_key(self) -> str:
        return f"{self.kind}:{self.ident}"


# ── source resolution ──────────────────────────────────────────────────────

async def resolve(
    *, release: str | None = None, tmod: str | None = None, game: str | None = None,
) -> Source:
    """Resolve exactly one of the three source params to a ``Source``."""
    given = [p for p in (release, tmod, game) if p]
    if len(given) != 1:
        raise _bad("Pass exactly one source: release, tmod or game.")

    if release:
        return await _release_source(release)
    if tmod:
        return await _upload_source(tmod)
    return await _game_source(game or "")


async def _release_source(release_id: str) -> Source:
    # viewer=None -> anonymous visibility: published public/unlisted releases only.
    rel, project = await hub.release_with_project(release_id, None)
    if rel.release_format != "tmod":
        raise _missing("That release has nothing to preview.")
    data = await hub.store.get_blob(rel.tmod_sha)
    if data is None:
        raise _missing("Release artifact not found.")
    return Source(kind="release", ident=release_id,
                  title=f"{project.title} {rel.tag}".strip(), tmod=data)


async def _upload_source(token: str) -> Source:
    data = await uploads.load(token)
    if data is None:
        # Read by a VISITOR on someone else's page, who can't "re-upload" anything.
        # Reloading the host page IS the fix: it re-renders, which re-posts the mod
        # and mints a fresh token.
        raise _missing("This preview has expired. Reload the page to start it again.")
    return Source(kind="tmod", ident=token,
                  title=_tmod_title(data) or "Mod preview", tmod=data)


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
    from app.core.config import settings
    from app.trove.updates import read as updates_read
    from app.trove.updates.cas import ContentStore

    meta = await updates_read.get_file_meta(LIVE_BRANCH, path)
    if not meta:
        return None
    store = ContentStore(settings.trove_update_store_dir)
    return await asyncio.to_thread(store.get, meta["content_sha256"])


# ── manifest (what this source can show) ───────────────────────────────────

async def manifest(src: Source) -> dict:
    """Everything the viewer needs to paint its picker: the previewable blueprints
    (with rig/assembly info) and .pkfx effects in this source."""
    if src.kind == "game":
        return await _game_manifest(src)
    return await _tmod_manifest(src)


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
    skeleton, _attach = await rig_index.resolve([stem])
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

async def blueprint(src: Source, path: str) -> dict:
    """Decoded voxel payload for one .blueprint (the 3D viewer's model format)."""
    if src.kind == "game":
        return await _game_blueprint(src, path)
    assert src.tmod is not None
    return await asyncio.to_thread(hub._decode_blueprint_payload, src.tmod, path)


async def _game_blueprint(src: Source, path: str) -> dict:
    from app.trove.render.voxel import (
        BlueprintEmpty,
        BlueprintError,
        BlueprintTooLarge,
        pack_blueprint,
    )
    # A game source is pinned to its own file - the path param is only ever the
    # one the manifest advertised, so ignore anything else rather than turning
    # this into a general file reader.
    target = src.game_path or ""
    if path and path.strip().lower() not in (target.lower(), vfx.basename(target).lower()):
        raise _missing("That blueprint isn't part of this preview.")
    raw = await _read_game_file(target)
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


async def assembled(src: Source) -> dict | None:
    """The source's blueprint parts assembled onto their creature rig, or None."""
    if src.kind == "game":
        return await _game_assembled(src)
    assert src.tmod is not None
    from app.trove.mods_hub import assembly, rig_index

    def _read(b: bytes):
        files = tmod_mod.read_tmod(b)["files"]
        names = [f["path"].split("/")[-1][: -len(".blueprint")].lower()
                 for f in files if f["path"].lower().endswith(".blueprint")]
        return files, names

    files, names = await asyncio.to_thread(_read, src.tmod)
    skeleton, attach = await rig_index.resolve(names)
    return await asyncio.to_thread(
        lambda: assembly.assemble(files, rig_name=skeleton, ap_overrides=attach))


async def _game_assembled(src: Source) -> dict | None:
    """Assemble a NATIVE creature from one blueprint path: resolve the part to its
    skeleton, then pull that skeleton's every bound part out of the game tree.

    This is the piece a partner can't do themselves - they have a file path, and we
    have the binfab map that says which other files belong to the same creature."""
    path = src.game_path or ""
    if not path.lower().endswith(".blueprint"):
        return None
    from app.trove.mods_hub import assembly, rig_index

    stem = vfx.basename(path).lower()[: -len(".blueprint")]
    skeleton, _attach = await rig_index.resolve([stem])
    if not skeleton or not assembly.has_baked_rig(skeleton):
        return None                      # unknown rig -> no guess, no render
    parts = await rig_index.parts_for(skeleton)
    if not parts:
        return None

    fmap = await game_file_map(LIVE_BRANCH)
    files: list[dict] = []
    for basename, _ap in parts.items():
        gp = fmap.get(f"{basename}.blueprint")
        raw = await _read_game_file(gp) if gp else None
        if raw is None:
            continue                     # a part we don't have -> the rest still assembles
        files.append({"path": f"{basename}.blueprint",
                      "content_base64": base64.b64encode(raw).decode()})
    if not files:
        return None
    return await asyncio.to_thread(
        lambda: assembly.assemble(files, rig_name=skeleton, ap_overrides=parts))


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


async def allowed_origins() -> list[str]:
    """The origins the admin has allowed to iframe the viewer. Empty = nobody.

    Accepts space- or comma-separated entries; malformed ones are ignored (see
    ``_ORIGIN_RE``) so a bad character can never break or widen the CSP."""
    from app.admin import runtime_config

    raw = str(await runtime_config.get_setting("embed.allowed_origins") or "")
    seen: list[str] = []
    for part in raw.replace(",", " ").split():
        origin = part.strip().rstrip("/")
        if _ORIGIN_RE.match(origin) and origin not in seen:
            seen.append(origin)
    return seen
