"""Golden fixtures for the codexes port (native/kiwi-server/testdata/codexes).

Copies a few small game files out of an extracted tree, runs the Python
decoders, route serializers and the crafting calculator on them, and writes
what Python produced to golden.json. Run with the production-identical venv:

    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe \
        native/port/parity/gen_codexes_fixtures.py E:/Trove [Trove_x64.exe]
"""

from __future__ import annotations

import base64
import json
import os
import shutil
import sys
from datetime import datetime, timezone

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))
sys.path.insert(0, ROOT)

from starlette.responses import JSONResponse  # noqa: E402

from app.trove.codexes import (  # noqa: E402
    abilities, badges, binfab, bonuses, crafting, extract, geode, links, localize, loot,
    mastery, pg_store, powerrank, providers, recipe, unlocks, upgrades,
)
from app.trove.codexes.schemas import CodexEntryOut, CodexLinkOut  # noqa: E402
from app.trove.render.voxel import decode_render_map, is_empty_blueprint  # noqa: E402

OUT = os.path.join(ROOT, "native", "kiwi-server", "testdata", "codexes")

PREFABS = [
    "prefabs/collections/boat/clipper_tortoise_lunarswan.binfab",
    "prefabs/collections/pet/trex_manabeast_fire.binfab",
    "prefabs/collections/pet/dragonfly.binfab",
    "prefabs/collections/flask/effect_crit.binfab",
    "prefabs/collections/mount/dragon_amperium.binfab",
    "prefabs/item/companion/sample.binfab",
    "prefabs/recipes/recipe_equipment_events_2024_december_hat_1.binfab",
    "prefabs/recipes/recipe_item_wings_lunacy.binfab",
    "prefabs/recipes/recipe_item_crafting_deep_core_daily_fae.binfab",
    "prefabs/placeable/block/script/arrow_purple_down_interactive.binfab",
    "prefabs/placeable/crafting/workbench_prismatic_blue_interactive.binfab",
]
TABLES = {
    "powerrank": "prefabs/meta/powerrank.binfab",
    "geode_table": "prefabs/collections/collection_geodecompanion.binfab",
    "geode_multipliers": "prefabs/meta/geode_multipliers.binfab",
    "unlocks": "prefabs/collections/unlocks.binfab",
    "badges": "prefabs/meta/badges.binfab",
    "tree_barrier": "prefabs/upgrade/trees/barrier.binfab",
    "fx_barrier": "prefabs/upgrade/upgrades/barrier.binfab",
    "tree_prestige": "prefabs/upgrade/trees/prestige_bard.binfab",
    "fx_prestige": "prefabs/upgrade/upgrades/prestige_bard.binfab",
    "fx_gleemur": "prefabs/upgrade/upgrades/gleemur_common.binfab",
    "pvpbanner": "prefabs/loot/pvpbanner.binfab",
    "locale": "languages/en/prefabs_placeable_npc_hub.binfab",
}
# Truncated: the parsers must stop where Python stops.
CUTS = {"hat": ("prefabs/loot/hat.binfab", 9000)}


def flat(rel: str) -> str:
    return rel.replace("/", "__")


def b64(b: bytes) -> str:
    return base64.b64encode(b).decode()


def stat_view(s: dict) -> dict:
    return {k: s[k] for k in ("stat", "stat_id", "operation", "amount", "value", "is_percent",
                              "label", "slot", "component", "level", "offset")}


def main() -> None:
    tree = sys.argv[1]
    exe = sys.argv[2] if len(sys.argv) > 2 else None
    shutil.rmtree(OUT, ignore_errors=True)
    os.makedirs(OUT)

    def copy(rel: str, data: bytes | None = None) -> bytes:
        if data is None:
            with open(os.path.join(tree, rel), "rb") as f:
                data = f.read()
        with open(os.path.join(OUT, flat(rel) if not rel.endswith(".cut") else rel), "wb") as f:
            f.write(data)
        return data

    g: dict = {"prefabs": {}, "tables": {}}
    pr_table = None
    for key, rel in TABLES.items():
        g["tables"][key] = flat(rel)
        copy(rel)
    for key, (rel, n) in CUTS.items():
        with open(os.path.join(tree, rel), "rb") as f:
            data = f.read()[:n]
        copy(f"{key}.cut", data)
        g["tables"][key] = f"{key}.cut"

    def table(key: str) -> bytes:
        with open(os.path.join(OUT, g["tables"][key]), "rb") as f:
            return f.read()

    pr_table = powerrank.parse_power_rank_table(table("powerrank"))
    g["powerrank_table"] = sorted([k, v] for k, v in pr_table.items())

    names = badges.parse_metric_names(open(exe, "rb").read()) if exe else []
    g["metric_names"] = names
    synthetic = b"\x00\x00\x80?" + b"".join(
        n.encode() + b"\x00" * (8 - len(n) % 8 or 8) for n in names[1:40]) + b"KPlayerMetric\x00tail"
    g["metric_exe_b64"] = b64(synthetic)
    g["metric_exe_names"] = badges.parse_metric_names(synthetic)

    # Every blueprint a fixture prefab names, with its model size, so blueprint_ref
    # is checked against the same candidates and sizes.
    bp_root = os.path.join(tree, "blueprints")
    all_bps = []
    for d, _, fs in os.walk(bp_root):
        for f in fs:
            all_bps.append(os.path.relpath(os.path.join(d, f), bp_root).replace("\\", "/").lower())
    for rel in PREFABS:
        data = copy(rel)
        low = data.lower()
        valid = sorted(b for b in all_bps if b.removesuffix(".blueprint").encode() in low)
        sizes = {}
        for b in valid:
            with open(os.path.join(bp_root, b), "rb") as f:
                raw = f.read()
            sizes[b] = 0 if is_empty_blueprint(raw) else len(decode_render_map(raw))
        ident = binfab.decode_identity(data)
        rig = binfab.extract_rig_refs(data)
        parsed = recipe.parse_recipe(data, prefab_exists=lambda p: False)
        g["prefabs"][rel] = {
            "file": flat(rel),
            "identity": None if ident is None else {k: ident[k] for k in ("name_key", "category", "desc_key", "tradable")},
            "stats": [stat_view(s) for s in bonuses.extract_stat_bonuses(data)],
            "abilities": bonuses.extract_abilities(data),
            "power_rank": powerrank.decode_power_rank(data, pr_table),
            "recipe": parsed,
            "recipe_mastery": recipe.recipe_mastery_from_prefab(data),
            "rig": rig,
            "valid": valid,
            "sizes": sizes,
            "blueprint": extract.blueprint_ref(data, set(valid), rel, model_size=lambda n: sizes.get(n.lower().removeprefix("blueprints/"), 0)),
            "blueprint_named": extract.blueprint_ref(data, set(valid), rel),
            "upgrade_tree_ref": geode.find_upgrade_tree_ref(data),
            "ability_keys": abilities.extract_keys(data),
        }
        # The model sizes themselves, for the voxel counter.
        for b, n in sizes.items():
            if n and len(g.setdefault("voxel_counts", {})) < 6:
                with open(os.path.join(bp_root, b), "rb") as f:
                    raw = f.read()
                if len(raw) < 20000:
                    copy("bp__" + b.replace("/", "__"), raw)
                    g["voxel_counts"]["bp__" + b.replace("/", "__")] = n

    g["geode_members"] = geode.geode_companion_members(table("geode_table"))
    g["collection_map"] = binfab.collection_category_map(table("geode_table"))
    g["geode_multipliers"] = mastery.parse_geode_multipliers(table("geode_multipliers"))
    g["mastery_for"] = {p: [mastery.mastery_for(p, {}), mastery.geode_mastery_for(p, g["geode_multipliers"])]
                        for p in ("collections/wings/x", "skins/knight_x", "item/companion/sample",
                                  "equipment/hat_x", "recipe_y", "boat_x", "collections/fishing/rod")}
    pairs = unlocks.parse_unlocks(table("unlocks"))
    g["unlocks"] = pairs
    g["unlock_links"] = links.unlock_links(pairs, "live-us")
    b_rows = badges.parse_badges(table("badges"), names)
    g["badges"] = {"rows": [{k: r[k] for k in ("collection", "badge_id", "rank", "rank_name", "completion_kind",
                                                "requirement_key", "label", "amount", "difficulty", "context",
                                                "status", "blocker", "reason")} for r in b_rows["rows"]],
                   "errors": b_rows["errors"]}
    g["badges_unresolved"] = badges.parse_badges(table("badges"), [])["errors"]
    from app.trove.codexes.models import requirement_rows
    g["requirement_rows"] = requirement_rows(b_rows["rows"], "live-us")
    for key in ("barrier", "prestige"):
        t = upgrades.parse_upgrade_costs(table(f"tree_{key}"), TABLES[f"tree_{key}"])
        keys = [n["node_key"] for n in t["nodes"]]
        g[f"tree_{key}"] = t
        g[f"fx_{key}"] = upgrades.parse_upgrade_effects(table(f"fx_{key}"), keys)
        g[f"links_{key}"] = links.upgrade_links(t, "live-us", TABLES[f"tree_{key}"])
    g["gleemur_levels"] = geode.parse_upgrade_tree(table("fx_gleemur"))
    g["pvpbanner"] = loot.parse_style_catalogue(table("pvpbanner"), TABLES["pvpbanner"])[:40]
    g["hat_rows"] = loot.parse_style_catalogue(table("hat"), "prefabs/loot/hat.binfab")
    loc = binfab.extract_localization_map(table("locale"))
    g["locale"] = loc
    g["stat_names"] = {k: localize.resolve_stat_name(loc, k) for k in (
        "$Stat_MaxHealth", "$Stat_CriticalHitDamageBonus", "$Stat_HealthRegenBonus_controller",
        "$Stat_Unknown_FooBar", "$Stat_XBonus")}
    g["slot_names"] = {k: localize.resolve_slot_name(loc, k) for k in (
        "$EquipmentSlot_Cart", "$EquipmentSlot_Boat", "$EquipmentSlot_BigThing")}

    bench = PREFABS[-1]
    with open(os.path.join(OUT, flat(bench)), "rb") as f:
        bench_data = f.read()
    recipes_dir = os.path.join(tree, "prefabs", "recipes")
    full = {os.path.relpath(os.path.join(d, f), recipes_dir).replace("\\", "/").removesuffix(".binfab").lower()
            for d, _, fs in os.walk(recipes_dir) for f in fs if f.endswith(".binfab")}
    refs = providers.extract_provider_refs(bench_data, full)
    g["provider_known"] = sorted(refs)
    g["provider_map"] = providers.build_provider_map([("prefabs/" + bench.removeprefix("prefabs/"), bench_data)], set(refs))
    g["provider_categories"] = providers.extract_categories(bench_data, set(refs))

    g["entry_paths"] = {p: links.entry_path(p) for p in (
        "item/crafting/crystal", "prefabs/item/x.binfab", " Item\\Foo\\Bar ", "/abs/x", "", "x.BINFAB")}
    g["classify"] = {p: __import__("app.trove.codexes.types", fromlist=["classify"]).classify(p) for p in (
        "prefabs/collections/pet/dog.binfab", "prefabs/collections/pet/dog_npc.binfab",
        "prefabs/item/fish/x.binfab", "prefabs/item/x.binfab", "prefabs/equipment/hat.binfab",
        "prefabs/skins/k.binfab", "prefabs/collections/fishing/rod.binfab", "prefabs/other/x.binfab",
        "prefabs/recipes/r.txt", "item/x.binfab")}

    g["filters"] = []
    for args in ({}, {"codex_type": "mount", "search": "50%_off\\x", "category": "Dragon", "tradable": False,
                      "stat": "$Stat_MagicFind", "ability": "abilities/x"}, {"search": "", "tradable": True}):
        where, a = pg_store.build_filter("pts", **args)
        g["filters"].append({"args": args, "where": where, "params": a})

    ts = datetime(2026, 10, 20, 12, 34, 56, 123450, tzinfo=timezone.utc)
    ts0 = datetime(2026, 10, 20, 12, 34, 56, tzinfo=timezone.utc)
    rows = [
        {"codex_type": "mount", "path": "prefabs/collections/mount/x.binfab", "name": "Ünïcode “Mount”",
         "category": "Dragon", "description": "", "tradable": True, "mastery": 50, "mastery_geode": None,
         "power_rank": 75, "blueprint": "mount_x.blueprint",
         "data": {"stats": [{"amount": 1e-05, "value": 1e16, "x": 0.1, "n": 3, "f": 3.0, "neg": -2.5e-7}]},
         "indexed_at": ts},
        {"codex_type": "item", "path": "p", "name": "n", "category": "", "description": "d\n\"q\"",
         "tradable": None, "mastery": None, "mastery_geode": 2, "power_rank": None, "blueprint": None,
         "data": {}, "indexed_at": ts0, "stat_value": 0.30000001192092896, "stat_percent": False},
    ]
    from app.trove.router import _codex_out  # noqa: E402
    from app.site.router import _codex_row  # noqa: E402
    g["serialize"] = [{
        "row": {k: (v.isoformat() if isinstance(v, datetime) else v) for k, v in r.items()},
        "v1": _codex_out(r).model_dump_json(),
        "site": JSONResponse(_codex_row(r)).body.decode(),
    } for r in rows]
    link_rows = [{"rel": "crafts", "path": "prefabs/x.binfab", "codex_type": None, "name": None,
                  "category": None, "blueprint": None, "qty": 2.0, "data": {}},
                 {"rel": "craftable_at", "path": "prefabs/y.binfab", "codex_type": "item", "name": "Y",
                  "category": "C", "blueprint": "b", "qty": None, "data": {"lane": "bench", "category_order": None}}]
    g["links_v1"] = [CodexLinkOut(rel=r["rel"], path=r["path"], type=r.get("codex_type"),
                                  name=r.get("name") or "", category=r.get("category") or "",
                                  blueprint=r.get("blueprint"), qty=r.get("qty"),
                                  data=r.get("data") or {}).model_dump_json() for r in link_rows]
    g["links_rows"] = link_rows

    recipes_rows = [
        {"path": "prefabs/recipes/a.binfab", "name": "A", "category": "Mount", "data": {"recipe": {
            "output": {"path": "item/a", "name": "", "amount": 2},
            "ingredients": [{"path": "item/b", "name": "B", "amount": 3}, {"path": "item/c", "name": "", "amount": 0},
                            {"path": "", "amount": 9}]}}},
        {"path": "prefabs/recipes/b.binfab", "name": "B", "category": "", "data": {"recipe": {
            "output": {"path": "item/b", "name": "B", "amount": 1},
            "ingredients": [{"path": "item/d", "name": "D", "amount": 5}, {"path": "item/a", "name": "A", "amount": 1}]}}},
        {"path": "prefabs/recipes/c.binfab", "name": "C", "category": "", "data": {"recipe": {"output": None}}},
    ]
    prices = {"B": {"median_each": 12.345, "count": 3}, "D": {"median_each": 0.1, "count": 1},
              "C": {"median_each": 7.0, "count": 9}}
    crafting._MAP_CACHE.clear()
    rmap = {}
    for row in recipes_rows:
        rec = (row.get("data") or {}).get("recipe") or {}
        out = rec.get("output")
        if not out or not out.get("path"):
            continue
        rmap[out["path"]] = {"source_path": row["path"], "name": row["name"], "category": row.get("category") or "",
                             "output": {"path": out["path"], "name": out.get("name") or row["name"],
                                        "amount": int(out.get("amount") or 1)},
                             "ingredients": [{"path": i["path"], "name": i.get("name") or i["path"],
                                              "amount": int(i.get("amount") or 1)}
                                             for i in (rec.get("ingredients") or []) if i.get("path")]}
    counter, names_seen = [0], set()
    raw = crafting._raw("item/a", "A", 2, rmap, 0, frozenset(), counter, names_seen)
    tree = crafting._annotate(raw, prices)
    g["crafting"] = {"recipes": recipes_rows, "prices": prices, "tree": json.dumps(tree, separators=(",", ":")),
                     "node_count": counter[0], "names": sorted(names_seen)}

    with open(os.path.join(OUT, "golden.json"), "w", encoding="utf-8") as f:
        json.dump(g, f, ensure_ascii=False, indent=0, default=list)
    total = sum(os.path.getsize(os.path.join(OUT, x)) for x in os.listdir(OUT))
    print(f"wrote {len(os.listdir(OUT))} files, {total} bytes")


if __name__ == "__main__":
    main()
