"""Standalone test of the VFX dependency resolver (no DB needed).

Builds a .tmod containing one real .pkfx and NO textures, then verifies the
ref extractor + recursive dep resolver find its assets in a local game tree.
Run: python scripts/vfx_resolver_test.py [VFX_DIR] [PKFX_NAME]
"""
import asyncio
import os
import sys

from app.trove import tmod
from app.trove.mods_hub import vfx, service

VFX = sys.argv[1] if len(sys.argv) > 1 else r"S:\Downloads\particles\VFX"
PKFX = sys.argv[2] if len(sys.argv) > 2 else "weapon_aura_torch_fire_01.pkfx"


def build_local_index(root):
    idx = {}
    for r, _d, names in os.walk(root):
        for n in names:
            idx.setdefault(n.lower(), os.path.join(r, n))
    return idx


async def main():
    local = build_local_index(VFX)
    print(f"local game tree: {len(local)} files under {VFX}")

    pkfx_bytes = open(os.path.join(VFX, "Particles", PKFX), "rb").read()
    # bundle ONLY the .pkfx (simulate a mod with no textures)
    tmod_bytes = tmod.build_tmod(1, {"title": "test"},
                                 [("particles/" + PKFX.lower(), pkfx_bytes)])

    items, index = service._tmod_pkfx_and_index(tmod_bytes)
    print(f"\n.tmod: {len(items)} pkfx, {len(index)} files bundled")
    for it in items:
        print("  pkfx:", it["path"], it["size"], "bytes")

    # lookup/read over the local game tree (stands in for the updates archive)
    def lookup(bn):
        return local.get(bn)

    async def read(path):
        return open(path, "rb").read()

    # direct-ref classification (the manifest)
    refs = vfx.extract_refs(pkfx_bytes.decode("utf-8", "replace"))
    print(f"\ndirect refs: {len(refs)}")
    counts = {"mod": 0, "game": 0, "missing": 0}
    for ref in refs:
        bn = vfx.basename(ref).lower()
        src = "mod" if bn in index else ("game" if lookup(bn) else "missing")
        counts[src] += 1
        print(f"  [{src:7}] {ref}")
    print("  ->", counts)

    # recursive dep set (authorizes /asset)
    depset = await service._build_vfx_depset(index, lookup, read)
    resolvable = sum(1 for bn in depset if (bn in index or lookup(bn)))
    print(f"\nrecursive depset: {len(depset)} basenames, {resolvable} resolvable")

    # asset resolution for one texture
    tex = next((vfx.basename(r) for r in refs if r.lower().endswith(".dds")), None)
    if tex:
        bn = tex.lower()
        raw = open(lookup(bn), "rb").read() if lookup(bn) else None
        print(f"\nasset '{tex}': {'resolved ' + str(len(raw)) + ' bytes' if raw else 'MISSING'}"
              f"  media={vfx.media_type_for(tex)}")
    print("\nOK" if counts["missing"] == 0 else "\n(some refs missing - check the VFX dir)")


asyncio.run(main())
