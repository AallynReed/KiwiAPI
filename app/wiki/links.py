"""How wiki pages link to each other.

``link(ref)`` turns a game reference into the wiki URL that shows it: a page of its
own, or an anchor on the group page that lists it. ``related(refs)`` reads the other
way: every decoded relation that points at a page, grouped for its Related section.

A ref is a prefab path under ``prefabs/`` without ``.binfab`` (case-insensitive), or
a tagged id: ``bp:<blueprint>``, ``class:<tech_name>``, ``adv:<adventure id>``,
``page:<wiki path>``. Only relations the game files state are indexed; nothing is
joined by name.
"""
from __future__ import annotations

from typing import Any

from app.trove import stats as trove_stats
from app.trove.decode import store as gamedata
from app.wiki import entities, upgrade_pages

FILES = tuple(dict.fromkeys([*(s["file"] for s in entities.KINDS.values()), "worlds.json", "recipes.json",
                             "badges.json", "quests.json", "npcs.json", "gem_upgrades.json", "leaderboards.json", "titles.json", "upgrade_trees.json", "mastery.json", "class_rewards.json",
                             "items.json"]))

# Relation -> (heading on the page it points at, icon). Listed in display order.
RELATIONS: dict[str, tuple[str, str]] = {
    "made_from": ("Crafted from", "fa-hammer"),
    "crafted_at": ("Crafted at", "fa-industry"),
    "sold_by": ("Sold by", "fa-store"),
    "taught_by": ("Recipe learned from", "fa-scroll"),
    "unlocked_by": ("Unlocked by", "fa-key"),
    "reward_badge": ("Badge reward from", "fa-medal"),
    "reward_adventure": ("Reward from", "fa-gift"),
    "reward_mastery": ("Mastery reward at", "fa-crown"),
    "class_level": ("Earned by levelling", "fa-arrow-up"),
    "yielded_by": ("Loot Collecting gives it from", "fa-recycle"),
    "contained_in": ("Can come from", "fa-dice"),
    "trophy_of": ("Trophy of", "fa-trophy"),
    "used_in": ("Used to craft", "fa-screwdriver-wrench"),
    "teaches": ("Teaches", "fa-scroll"),
    "levels": ("Used to level", "fa-arrow-up-right-dots"),
    "opens": ("Opens", "fa-lock-open"),
    "paragon": ("Buys Paragon powers for", "fa-crown"),
    "upgrades": ("Used for", "fa-arrow-up"),
    "wanted_by": ("Adventures that ask for it", "fa-scroll"),
    "gives_adventure": ("Gives adventures", "fa-comment"),
    "given_by": ("Given by", "fa-comment"),
    "counts_for": ("Counts toward", "fa-medal"),
    "sells": ("Sells", "fa-store"),
    "trophies": ("Trophies", "fa-trophy"),
    "costumes": ("Costumes", "fa-shirt"),
    "rings": ("Class rings", "fa-ring"),
    "boards": ("Leaderboards", "fa-ranking-star"),
    "for_class": ("Class", "fa-user"),
}
SHOW = 24  # links a group shows before "Show all"


def norm(ref: str | None) -> str:
    r = str(ref or "").replace("\\", "/").strip()
    if ":" in r.split("/", 1)[0]:
        tag, _, rest = r.partition(":")
        return f"{tag}:{rest.lower()}" if tag in ("bp", "class") else r
    return r.lower().removeprefix("prefabs/").removesuffix(".binfab")


def blueprint_ref(bp: str | None) -> str:
    name = (bp or "").rsplit("/", 1)[-1].lower().removesuffix(".blueprint")
    return f"bp:{name}" if name else ""


def anchor(prefix: str, slug: str) -> str:
    return f"{prefix}-" + "".join(ch if ch.isalnum() or ch in "-_" else "-" for ch in slug.lower())


def class_slug(name: str) -> str:
    return "-".join("".join(ch if ch.isalnum() else " " for ch in name.lower()).split())


def class_url(tech: str) -> str:
    c = trove_stats.class_by_tech_name((tech or "").lower())
    return f"/class/{class_slug(c['name'])}" if c else ""


def _load(name: str, key: str | None = None) -> Any:
    data = gamedata.load(name, {}) or {}
    return (data.get(key) if isinstance(data, dict) else None) if key else data


# ── ref -> page ────────────────────────────────────────────────────────────

@gamedata.cached(*FILES)
def targets() -> dict[str, dict[str, str]]:
    """``ref -> {"url", "name"}``; a page of its own wins over an anchor."""
    out: dict[str, dict[str, str]] = {}

    def put(ref: str | None, url: str, name: str) -> None:
        key = norm(ref)
        if key and url and name and key not in out:
            out[key] = {"url": url, "name": name}

    group_kinds = ("style", "npc", "placeable", "adventure")
    for kind in entities.KINDS:
        if kind in group_kinds:
            continue
        for e in entities.entries(kind):
            put(e.get("prefab"), entities.url(kind, e), e.get("title") or e["name"])
            if kind == "item" and e["prefab"].startswith("equipment/"):
                put(blueprint_ref(e.get("blueprint")), entities.url(kind, e), e["name"])
    for b in entities.entries("badge"):
        for r in b.get("ranks") or []:
            put(r.get("prefab"), entities.url("badge", b), b["name"])
    for c in (trove_stats.all_classes() or {}).get("items") or []:
        put(f"class:{c['tech_name']}", f"/class/{class_slug(c['name'])}", c["name"])
    for n in entities._npc_file().get("npcs") or []:
        top = entities._folder(n.get("group", ""))[0] or "general"
        u = entities.url("boss", n) if n.get("boss") else f"/npc/{entities.url_slug(top)}#{anchor('n', n['slug'])}"
        name = entities._npc_card(n)["name"]
        put(n.get("prefab"), u, name)
        for p in n.get("placed_by") or []:
            put(p.get("prefab"), u, name)
    for g in entities.entries("placeable"):
        for x in g.get("entries") or []:
            put(x.get("prefab"), f"{entities.url('placeable', g)}#{anchor('p', x['slug'])}", x["name"])
    for g in entities.entries("style"):
        for x in g.get("styles") or []:
            put(blueprint_ref(x.get("blueprint")), f"{entities.url('style', g)}#{anchor('s', blueprint_ref(x.get('blueprint'))[3:])}",
                x["name"])
    for g in entities.entries("adventure"):
        for x in g.get("entries") or []:
            put(f"adv:{x['slug']}", f"{entities.url('adventure', g)}#{anchor('a', x['slug'])}", x.get("name") or g["name"])
    worlds = _load("worlds.json")
    for w in (worlds.get("worlds") or []) if isinstance(worlds, dict) else []:
        put(w.get("prefab"), f"/worlds#{anchor('w', w['slug'])}", w.get("item_name") or w["name"])
    for w in (worlds.get("delve_gateways") or []) if isinstance(worlds, dict) else []:
        put(w.get("prefab"), f"/delve-gateways#{anchor('g', w['slug'])}", w["name"])
    for t in _load("titles.json") or []:
        if t.get("id"):
            put(f"title:{t['id']}", f"/titles#{anchor('t', t['id'])}", t.get("name") or t["id"])
    for b in _load("leaderboards.json", "boards") or []:
        put(f"lb:{b['id']}", f"/leaderboards#{anchor('b', str(b['id']))}", b.get("name") or str(b["id"]))
    put("page:/gems", "/gems", "Gem upgrades")
    for page, sid in upgrade_pages.SYSTEMS.items():
        put(f"page:/{page}", f"/{page}", (upgrade_pages.system(sid) or {}).get("name") or page)
    put("page:/geode-tools", "/geode-tools", "Geode tool upgrades")
    for lad in (_load("mastery.json", "ladders") or []):
        for lv in lad.get("levels") or []:
            if lv.get("rewards"):
                put(f"mastery:{lad['slug']}:{lv['level']}", f"/mastery#{anchor('l-' + lad['slug'], str(lv['level']))}",
                    f"{lad['name']} {lv['level']}")
    return out


def link(ref: str | None) -> str:
    t = targets().get(norm(ref))
    return t["url"] if t else ""


def name(ref: str | None) -> str:
    t = targets().get(norm(ref))
    return t["name"] if t else ""


def thing(text: str, ref: str | None = None, count: int | float | None = None) -> dict:
    """A name to show, linked when its ref has a page: ``{"text", "url"}``."""
    from app.wiki.ability_view import num
    return {"text": f"{num(count)} × {text}" if count and count != 1 else text, "url": link(ref)}


# ── relations ──────────────────────────────────────────────────────────────

def _result_ref(x: dict) -> str:
    return x.get("path") or (x.get("collectable") or {}).get("ref") or ""


@gamedata.cached(*FILES)
def _edges() -> dict[str, list[tuple[str, str, str]]]:
    """``target ref -> [(relation, source ref, fallback name)]``."""
    out: dict[str, list[tuple[str, str, str]]] = {}

    def edge(target: str | None, rel: str, source: str | None, label: str = "") -> None:
        t, s = norm(target), norm(source)
        if t and s and t != s:
            row = (rel, s, label)
            rows = out.setdefault(t, [])
            if row not in rows:
                rows.append(row)

    recipes = entities._recipes()
    stations = {st["slug"]: st.get("prefab") for st in entities._recipe_file().get("stations") or []}
    vendors: dict[str, list[str]] = {}
    for n in entities._npc_file().get("npcs") or []:
        for tab in (n.get("crafting") or {}).get("tabs") or []:
            for rid in tab.get("recipes") or []:
                vendors.setdefault(rid, []).append(n["prefab"])
    for rid, r in recipes.items():
        makes = [(_result_ref(x), x.get("name") or "") for x in r.get("results") or [] if _result_ref(x)]
        needs = [(x.get("path") or "", x.get("name") or "") for x in r.get("ingredients") or [] if x.get("path")]
        for ref, label in makes:
            for sid in r.get("stations") or []:
                edge(ref, "crafted_at", stations.get(sid))
            for npc in vendors.get(rid) or []:
                edge(ref, "sold_by", npc)
                edge(npc, "sells", ref, label)
            for i, ilabel in needs:
                edge(ref, "made_from", i, ilabel)
                edge(i, "used_in", ref, label)
            for u in r.get("unlocked_by") or []:
                edge(ref, "taught_by", u)
                edge(u, "teaches", ref, label)
    for b in _load("badges.json") or []:
        for r in b.get("ranks") or []:
            for rw in r.get("rewards") or []:
                for gr in rw.get("grants") or []:
                    edge(gr.get("prefab") or blueprint_ref(gr.get("blueprint")), "reward_badge", r.get("prefab"))
            for soul in (r.get("requirement") or {}).get("soul_items") or []:
                edge(soul, "counts_for", r.get("prefab"))
    for g in _load("quests.json", "groups") or []:
        for x in g.get("entries") or []:
            src = f"adv:{x['slug']}"
            for rw in x.get("rewards") or []:
                edge(rw.get("prefab"), "reward_adventure", src)
            obj = x.get("objective") or {}
            for o in [obj, *(obj.get("objectives") or [])]:
                for t in (o.get("items") or []) + (o.get("npcs") or []) + (o.get("targets") or []):
                    edge(t.get("prefab"), "wanted_by", src)
    for n in entities._npc_file().get("npcs") or []:
        for a in n.get("offers_adventures") or []:
            edge(n["prefab"], "gives_adventure", f"adv:{a['id']}", a.get("name") or "")
            edge(f"adv:{a['id']}", "given_by", n["prefab"])
        for a in n.get("defeat_adventures") or []:
            edge(n["prefab"], "wanted_by", f"adv:{a['id']}")
    for c in _load("companions.json") or []:
        for lv in c.get("levels") or []:
            for cost in lv.get("cost") or []:
                edge(cost.get("item"), "levels", c.get("prefab"))
    gems = _load("gem_upgrades.json")
    if isinstance(gems, dict):
        mats = [m for pair in (gems.get("materials") or {}).values() for m in pair.values()]
        mats += [b["item"] for b in (gems.get("boosters") or []) + (gems.get("focuses") or []) if b.get("item")]
        mats += [c["item"] for g in gems.get("gems") or [] for c in g.get("reselect_cost") or [] if c.get("item")]
        for m in mats:
            edge(m, "upgrades", "page:/gems")
    for f in _load("fish.json", "fish") or []:
        for z in f.get("sizes") or []:
            for y in z.get("deconstruct") or []:
                edge(y.get("prefab"), "yielded_by", f.get("prefab"))
            trophy = (z.get("trophy") or {}).get("prefab")
            edge(trophy, "trophy_of", f.get("prefab"))
            edge(f.get("prefab"), "trophies", trophy, (z.get("trophy") or {}).get("name") or "")
    for m in _load("mementos.json", "mementos") or []:
        for y in m.get("deconstruct") or []:
            edge(y.get("item"), "yielded_by", m.get("prefab"))
    for c in entities.entries("costume"):
        edge(f"class:{c.get('class', '')}", "costumes", c.get("prefab"))
        edge(c.get("prefab"), "for_class", f"class:{c.get('class', '')}")
    for lad in (_load("mastery.json", "ladders") or []):
        rewards = {r["id"]: r for r in lad.get("rewards") or []}
        for lv in lad.get("levels") or []:
            for rid in lv.get("rewards") or []:
                for g in (rewards.get(rid) or {}).get("grants") or []:
                    edge(g.get("ref"), "reward_mastery", f"mastery:{lad['slug']}:{lv['level']}")
    for tech, c in (_load("class_rewards.json", "classes") or {}).items():
        for lv in c.get("levels") or []:
            for ref in ([lv["costume"]] if lv.get("costume") else []) + (lv.get("styles") or []):
                edge(ref, "class_level", f"class:{tech}")
    trees = _load("upgrade_trees.json")
    if isinstance(trees, dict):
        pages = {sid: page for page, sid in upgrade_pages.SYSTEMS.items()}
        for s in trees.get("systems") or []:
            for n in s.get("nodes") or []:
                for c in (n.get("cost") or []) + (n.get("opens_with") or []):
                    edge(c.get("item"), "upgrades", f"page:/{pages.get(s['slug'], '')}")
        for m in trees.get("geode_tools") or []:
            for lv in m.get("levels") or []:
                for c in lv.get("cost") or []:
                    edge(c.get("item"), "upgrades", "page:/geode-tools")
        for p in trees.get("paragon") or []:
            for power in p.get("powers") or []:
                for c in power.get("cost") or []:
                    edge(c.get("item"), "paragon", f"class:{p['class']}")
    for b in _load("leaderboards.json", "boards") or []:
        if b.get("class"):
            edge(f"class:{b['class']}", "boards", f"lb:{b['id']}")
    for it in _load("items.json", "items") or []:
        src = it.get("prefab")
        for u in it.get("unlocks") or []:
            edge(u, "unlocked_by", src)
        for t in it.get("titles") or []:
            edge(f"title:{t}", "unlocked_by", src)
        for m in (it.get("random_unlock") or {}).get("members") or []:
            edge(m, "contained_in", src)
        for dec in it.get("deconstruct") or []:
            for y in dec.get("yields") or []:
                edge(y.get("item"), "yielded_by", src)
        for opt in it.get("opens") or []:
            for c in opt.get("cost") or []:
                edge(c.get("item"), "opens", src)
    return out


def related(*refs: str | None, skip: set[str] | frozenset[str] = frozenset()) -> list[dict]:
    """The Related groups for a page whose own refs are ``refs``: only sources that
    have a page are listed, and a relation the page already shows is in ``skip``."""
    own = {norm(r) for r in refs if r}
    groups: dict[str, list[dict]] = {}
    for ref in own:
        for rel, src, _label in _edges().get(ref) or []:
            if rel in skip or src in own:
                continue
            t = targets().get(src)
            if not t:
                continue
            rows = groups.setdefault(rel, [])
            if all(r["url"] != t["url"] for r in rows):
                rows.append({"text": t["name"], "url": t["url"]})
    return [{"label": RELATIONS[rel][0], "icon": RELATIONS[rel][1],
             "links": sorted(groups[rel], key=lambda r: r["text"].lower()), "show": SHOW}
            for rel in RELATIONS if rel in groups]
