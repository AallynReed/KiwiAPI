"""VFX (PopcornFX ``.pkfx``) dependency resolution for the Mods Hub preview.

A mod release rarely bundles the textures/meshes a ``.pkfx`` effect needs - those
live in the base game. The web viewer renders the effect client-side, so the
server's job is to hand it the ``.pkfx`` text plus every asset it references,
resolving each one:

  1. **bundled in the mod** (matched by basename inside the release's ``.tmod``), else
  2. **the live game tree** (the updates archive - ``game_file_map`` -> the CAS blob), else
  3. **missing** (surfaced so the UI can mark the preview partial).

PopcornFX v1.x ``.pkfx`` is a text format; references are quoted paths with a known
asset extension (e.g. ``Diffuse = "Textures/Foo.dds";``). ``$LOCAL$/...`` strings are
internal object refs, not files, and are excluded.
"""

from __future__ import annotations

import re

# Render-relevant asset types a .pkfx can reference. (.pkfx itself = a nested child
# effect, whose own dependencies we resolve recursively.)
ASSET_EXTS: tuple[str, ...] = (
    "dds", "png", "tga", "pkat", "pkmm", "fbx", "pkfx", "pkma", "pkml", "pkcf",
)

_REF_RE = re.compile(
    r'"([^"\n]+\.(?:' + "|".join(ASSET_EXTS) + r'))"', re.IGNORECASE,
)
# An object header sits at column 0: ``ClassName<ws>$LOCAL$/id``. Property lines are
# indented, and script bodies never contain ``$LOCAL$``, so this won't false-match them.
_HEADER_RE = re.compile(r'^(C[A-Za-z0-9_]+)[ \t]+\$LOCAL\$')


def extract_refs(pkfx_text: str) -> list[str]:
    """Asset paths referenced by a ``.pkfx`` - quoted strings ending in a known asset
    extension - excluding ``$LOCAL$`` object refs. Order-preserving and deduped
    (case-insensitively).

    References inside **editor-only** objects (``CNEdEditor*`` backdrops: the preview
    room, scale-reference models, lights, grids) are skipped - they are editor
    scaffolding, not effect render dependencies, and don't ship in the game assets
    (so counting them produced false "asset missing" warnings)."""
    seen: dict[str, str] = {}
    current: str | None = None
    for line in pkfx_text.splitlines():
        h = _HEADER_RE.match(line)
        if h:
            current = h.group(1)
            continue
        if current and current.startswith("CNEdEditor"):
            continue
        for m in _REF_RE.finditer(line):
            ref = m.group(1)
            if ref.startswith("$LOCAL$"):
                continue
            key = ref.lower()
            if key not in seen:
                seen[key] = ref
    return list(seen.values())


def basename(path: str) -> str:
    return path.replace("\\", "/").rsplit("/", 1)[-1]


_MEDIA = {
    "dds": "image/vnd.ms-dds",
    "png": "image/png",
    "tga": "image/x-tga",
    "pkat": "text/plain; charset=utf-8",
    "pkfx": "text/plain; charset=utf-8",
}


def media_type_for(path: str) -> str:
    ext = basename(path).rsplit(".", 1)[-1].lower() if "." in path else ""
    return _MEDIA.get(ext, "application/octet-stream")
