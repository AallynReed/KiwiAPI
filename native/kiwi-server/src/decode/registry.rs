//! `app/trove/decode/registry.py`: the decoders in Python's order, their file
//! formats, and what identifies a version of each one's code.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use regex::Regex;
use sha1::{Digest, Sha1};

use super::{
    decoders as d,
    json::{self, J},
    tree::Tree,
};

/// Python's `type(exc).__name__` and `str(exc)` of a failed build.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodeError {
    pub kind: &'static str,
    pub msg: String,
}

impl DecodeError {
    pub fn value(msg: impl Into<String>) -> Self {
        DecodeError { kind: "ValueError", msg: msg.into() }
    }

    pub fn key(key: &str) -> Self {
        DecodeError { kind: "KeyError", msg: format!("'{key}'") }
    }
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind, self.msg)
    }
}

pub type Built = Result<J, DecodeError>;

/// What a build sees: its own prefixes of the tree, and the repo baseline.
pub struct Ctx {
    pub tree: Tree,
    pub baseline: PathBuf,
}

impl Ctx {
    /// `store.baseline(name)`: the repo copy, where hand-curated fields live.
    pub fn baseline(&self, name: &str) -> Built {
        let bytes = std::fs::read(self.baseline.join(name))
            .map_err(|e| DecodeError { kind: "FileNotFoundError", msg: e.to_string() })?;
        let v = crate::pyjson::loads(&bytes).map_err(|_| DecodeError { kind: "JSONDecodeError", msg: name.into() })?;
        Ok(json::from_serde(&v))
    }
}

pub struct Decoder {
    pub name: &'static str,
    pub title: &'static str,
    pub output: &'static str,
    pub prefixes: &'static [&'static str],
    pub indent: Option<usize>,
    pub final_newline: bool,
    /// `None`: not ported yet, Python still builds it.
    pub build: Option<fn(&Ctx) -> Built>,
    /// `module.count(data)`; `None` where Python raises.
    pub count: fn(&J) -> Option<usize>,
}

impl Decoder {
    /// The file text, in the format the repo copy already uses.
    pub fn dump(&self, data: &J) -> String {
        let mut text = json::dumps(data, self.indent);
        if self.final_newline {
            text.push('\n');
        }
        text
    }

    pub fn run(&self, tree: &Tree, baseline: &Path) -> Option<Built> {
        let build = self.build?;
        Some(build(&Ctx { tree: tree.only(self.prefixes), baseline: baseline.to_path_buf() }))
    }
}

pub fn count_len(data: &J) -> Option<usize> {
    data.as_list().map(Vec::len)
}

pub fn count_key(key: &'static str) -> impl Fn(&J) -> Option<usize> {
    move |data: &J| data.get(key).map(J::len)
}

macro_rules! dec {
    ($name:literal, $title:literal, $output:literal, [$($p:literal),*], $indent:expr, $nl:expr, $build:expr, $count:expr) => {
        Decoder {
            name: $name,
            title: $title,
            output: $output,
            prefixes: &[$($p),*],
            indent: $indent,
            final_newline: $nl,
            build: $build,
            count: $count,
        }
    };
}

/// `DECODERS`, in Python's order.
pub static DECODERS: &[Decoder] = &[
    dec!(
        "class_levels",
        "Class levels",
        "classes.json",
        ["prefabs/class/"],
        Some(4),
        false,
        Some(d::class_levels::build),
        count_len
    ),
    dec!(
        "class_abilities",
        "Class abilities",
        "class_abilities.json",
        ["prefabs/class/", "prefabs/abilities/", "prefabs/sfx/", "languages/en/"],
        Some(4),
        false,
        Some(d::class_abilities::build),
        count_len
    ),
    dec!(
        "ally_abilities",
        "Ally abilities",
        "ally_abilities.json",
        ["prefabs/collections/", "prefabs/abilities/", "prefabs/sfx/", "languages/en/"],
        Some(4),
        false,
        Some(d::ally_abilities::build),
        count_len
    ),
    dec!(
        "gem_abilities",
        "Gem abilities",
        "gem_abilities.json",
        ["prefabs/item/gem/large/", "prefabs/abilities/", "prefabs/class/", "prefabs/sfx/", "languages/en/"],
        Some(4),
        false,
        Some(d::gem_abilities::build),
        count_len
    ),
    dec!(
        "ring_abilities",
        "Ring abilities",
        "ring_abilities.json",
        ["prefabs/abilities/", "prefabs/class/", "prefabs/sfx/", "languages/en/"],
        Some(4),
        false,
        Some(d::ring_abilities::build),
        count_len
    ),
    dec!(
        "mount_abilities",
        "Mount abilities",
        "mount_abilities.json",
        ["prefabs/collections/", "prefabs/abilities/", "prefabs/sfx/", "languages/en/"],
        Some(4),
        false,
        Some(d::mount_abilities::build),
        count_len
    ),
    dec!(
        "pvp_stat_ranges",
        "PvP stat curves",
        "pvp.json",
        ["prefabs/pvp/data/stats/"],
        Some(1),
        true,
        Some(d::pvp_stat_ranges::build),
        d::pvp_stat_ranges::count
    ),
    dec!(
        "delve_modifiers",
        "Delve modifiers",
        "delve_modifiers.json",
        ["prefabs/placeable/deco/delve/", "prefabs/abilities/delve/", "languages/en/"],
        Some(1),
        true,
        Some(d::delve_modifiers::build),
        |j| j.get("modifiers").map(J::len)
    ),
    dec!(
        "companions",
        "Companions",
        "companions.json",
        ["prefabs/collections/", "prefabs/item/", "prefabs/upgrade/", "prefabs/abilities/", "languages/en/"],
        Some(4),
        false,
        Some(d::companions::build),
        count_len
    ),
    dec!(
        "badges",
        "Badges",
        "badges.json",
        ["prefabs/", "languages/en/", "Trove_x64.exe"],
        Some(4),
        false,
        Some(d::badges::build),
        count_len
    ),
    dec!(
        "fish",
        "Fish",
        "fish.json",
        [
            "prefabs/item/",
            "prefabs/fish/",
            "prefabs/meta/fishing/",
            "prefabs/placeable/",
            "prefabs/collections/",
            "prefabs/crafting/",
            "prefabs/effects/fishingbait/",
            "prefabs/upgrade/upgrades/",
            "languages/en/"
        ],
        Some(4),
        false,
        Some(d::fish::build),
        |j| j.get("fish").map(J::len)
    ),
    dec!(
        "mementos",
        "Mementos",
        "mementos.json",
        [
            "prefabs/item/",
            "prefabs/collections/",
            "prefabs/crafting/",
            "prefabs/meta/",
            "prefabs/recipes/",
            "prefabs/equipment/",
            "prefabs/placeable/",
            "languages/en/"
        ],
        Some(4),
        false,
        Some(d::mementos::build),
        |j| j.get("mementos").map(J::len)
    ),
    dec!(
        "recipes",
        "Recipes",
        "recipes.json",
        ["prefabs/", "languages/en/"],
        None,
        true,
        Some(d::recipes::build),
        |j| Some(j.get("recipes").map_or(0, J::len))
    ),
    dec!(
        "collectibles",
        "Wings, boats, sails and auras",
        "collectibles.json",
        ["prefabs/collections/", "prefabs/abilities/", "prefabs/sfx/", "languages/en/", "particles/VFX/Particles/"],
        Some(4),
        false,
        Some(d::collectibles::build),
        |j| j.as_dict().map(|m| m.values().map(J::len).sum())
    ),
    dec!(
        "cosmetics",
        "Costumes and styles",
        "cosmetics.json",
        ["prefabs/skins/", "prefabs/loot/", "prefabs/collections/", "prefabs/equipment/", "languages/en/"],
        Some(4),
        false,
        Some(d::cosmetics::build),
        d::cosmetics::count
    ),
    dec!("titles", "Titles", "titles.json", ["languages/en/"], Some(4), false, Some(d::titles::build), count_len),
    dec!(
        "npcs",
        "NPCs and bosses",
        "npcs.json",
        [
            "prefabs/npc/",
            "prefabs/placeable/npc/",
            "prefabs/abilities/",
            "prefabs/meta/activities/",
            "prefabs/sfx/",
            "languages/en/"
        ],
        None,
        true,
        Some(d::npcs::build),
        |j| j.get("npcs").map(J::len)
    ),
    dec!(
        "placeables",
        "Blocks and placeables",
        "placeables.json",
        [
            "prefabs/placeable/",
            "prefabs/blocks/",
            "prefabs/recipes/",
            "prefabs/collections/",
            "prefabs/crafting/",
            "prefabs/claim/",
            "prefabs/fish/",
            "prefabs/meta/",
            "prefabs/professions/",
            "prefabs/npc/",
            "prefabs/item/",
            "blueprints/",
            "languages/en/"
        ],
        None,
        true,
        Some(d::placeables::build),
        count_placeables
    ),
    dec!(
        "quests",
        "Quests and Adventures",
        "quests.json",
        ["prefabs/", "languages/en/", "Trove_x64.exe"],
        Some(4),
        false,
        Some(d::quests::build),
        count_groups
    ),
    dec!(
        "worlds",
        "Worlds, biomes and dungeons",
        "worlds.json",
        [
            "prefabs/placeable/portal/",
            "prefabs/placeable/matchmaker/",
            "prefabs/tower/",
            "prefabs/challenge/",
            "prefabs/meta/",
            "blueprints/",
            "languages/en/"
        ],
        None,
        true,
        Some(d::worlds::build),
        count_worlds
    ),
    dec!(
        "subclass_abilities",
        "Subclass abilities",
        "subclass_abilities.json",
        ["prefabs/class/", "prefabs/abilities/subclass/", "languages/en/"],
        Some(4),
        false,
        Some(d::subclass_abilities::build),
        count_len
    ),
    dec!(
        "daily_bonuses",
        "Daily bonuses",
        "daily_bonuses.json",
        ["prefabs/loot/", "ui/", "languages/en/"],
        Some(1),
        true,
        Some(d::daily_bonuses::build),
        |j| j.get("days").map(J::len)
    ),
    dec!(
        "leaderboards",
        "Leaderboards",
        "leaderboards.json",
        ["prefabs/leaderboard/", "languages/en/", "Trove_x64.exe"],
        Some(1),
        true,
        Some(d::leaderboards::build),
        |j| j.get("boards").map(J::len)
    ),
    dec!(
        "gem_upgrades",
        "Gem upgrades",
        "gem_upgrades.json",
        ["prefabs/gems/meta/", "prefabs/item/gem/", "prefabs/item/crafting/", "languages/en/"],
        Some(1),
        true,
        Some(d::gem_upgrades::build),
        |j| j.get("gems").map(J::len)
    ),
    dec!(
        "upgrade_trees",
        "Upgrade trees",
        "upgrade_trees.json",
        [
            "prefabs/progression/",
            "prefabs/upgrade/",
            "prefabs/item/",
            "prefabs/abilities/",
            "prefabs/class/",
            "languages/en/"
        ],
        Some(1),
        true,
        Some(d::upgrade_trees::build),
        count_upgrade_trees
    ),
    dec!("items", "Items", "items.json", ["prefabs/", "languages/en/"], None, true, Some(d::items::build), |j| j
        .get("items")
        .map(J::len)),
    dec!(
        "mastery",
        "Mastery",
        "mastery.json",
        ["prefabs/meta/", "prefabs/claim/", "prefabs/collections/", "prefabs/item/", "languages/en/"],
        None,
        true,
        Some(d::mastery::build),
        count_mastery
    ),
    dec!(
        "class_rewards",
        "Class rewards",
        "class_rewards.json",
        ["prefabs/class/", "prefabs/claim/", "prefabs/abilities/", "prefabs/item/", "languages/en/"],
        Some(1),
        true,
        Some(d::class_rewards::build),
        count_class_rewards
    ),
    dec!(
        "lootbox_odds",
        "Lootbox odds",
        "lootbox_odds.json",
        ["languages/en/"],
        Some(1),
        true,
        Some(d::lootbox_odds::build),
        |j| j.get("boxes").map(J::len)
    ),
    dec!(
        "pvp_powerups",
        "PvP power-ups",
        "pvp_powerups.json",
        ["prefabs/pvp/", "prefabs/abilities/", "prefabs/effects/", "prefabs/sfx/", "languages/en/"],
        Some(1),
        true,
        Some(d::pvp_powerups::build),
        |j| j.get("powerups").map(J::len)
    ),
    dec!(
        "chat_commands",
        "Chat commands",
        "chat_commands.json",
        ["prefabs/"],
        Some(1),
        true,
        Some(d::chat_commands::build),
        |j| j.get("commands").map(J::len)
    ),
    dec!(
        "insignia",
        "Sigils",
        "insignia.json",
        ["prefabs/meta/insignia.binfab", "ui/nameplate.swf"],
        Some(1),
        true,
        None,
        |j| Some(j.get("power_rank").map_or(0, J::len) + j.get("mastery").map_or(0, J::len))
    ),
    dec!(
        "gear",
        "Gear",
        "gear.json",
        [
            "prefabs/equipment/",
            "prefabs/item/banner/",
            "prefabs/item/crafting/",
            "prefabs/crafting/",
            "prefabs/meta/",
            "prefabs/placeable/crafting/",
            "languages/en/",
            "blueprints/"
        ],
        Some(1),
        true,
        Some(d::gear::build),
        |j| Some(j.get("rarities").map_or(0, J::len) + j.get("banners").map_or(0, J::len))
    ),
];

fn sum_of(list: Option<&J>, key: &str) -> Option<usize> {
    list?.as_list()?.iter().map(|x| x.get(key).map(J::len)).sum()
}

fn count_placeables(j: &J) -> Option<usize> {
    Some(sum_of(j.get("groups").filter(|g| g.truthy()), "entries").unwrap_or(0))
}

fn count_groups(j: &J) -> Option<usize> {
    sum_of(j.get("groups"), "entries")
}

fn count_worlds(j: &J) -> Option<usize> {
    Some(
        j.get("worlds")?.len() + j.get("biomes")?.len() + j.get("delve_gateways")?.len() + j.get("shadow_tower")?.len(),
    )
}

fn count_upgrade_trees(j: &J) -> Option<usize> {
    Some(
        sum_of(j.get("systems"), "nodes")?
            + sum_of(j.get("geode_tools"), "levels")?
            + sum_of(j.get("paragon"), "powers")?,
    )
}

fn count_mastery(j: &J) -> Option<usize> {
    Some(sum_of(j.get("ladders"), "levels")? + j.get("collectibles")?.len())
}

fn count_class_rewards(j: &J) -> Option<usize> {
    j.get("classes")?.as_dict()?.values().map(|c| c.get("levels").map(J::len)).sum()
}

pub fn get(name: &str) -> Option<&'static Decoder> {
    DECODERS.iter().find(|d| d.name == name)
}

/// Where the Python decoder sources live, next to the repo baseline.
pub fn source_dir(baseline: &Path) -> PathBuf {
    baseline.parent().map_or_else(|| PathBuf::from("app/trove/decode"), |p| p.join("decode"))
}

/// The package modules `code_version` reads but `_uses` would not find.
const ALWAYS: [&str; 4] = ["common", "wire", "fields", "ability"];
/// `app.trove.decode` modules loaded in the API process (`_uses` only follows those).
fn loaded(name: &str) -> bool {
    !matches!(name, "view" | "__main__" | "__init__")
}

fn import_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?m)^[ \t]*from[ \t]+app\.trove\.decode(\.[A-Za-z_][A-Za-z0-9_]*)?[ \t]+import[ \t]+(\([^)]*\)|[^\n]*)",
        )
        .expect("regex")
    })
}

/// The `app.trove.decode` modules a source file imports (`ast.ImportFrom` nodes).
fn imports(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for cap in import_re().captures_iter(source) {
        match cap.get(1) {
            Some(m) => out.push(m.as_str()[1..].to_owned()),
            None => {
                let names = cap.get(2).map_or("", |m| m.as_str());
                for part in names.trim_matches(|c| c == '(' || c == ')').split(',') {
                    let part = part.split('#').next().unwrap_or("").trim();
                    if let Some(n) = part.split_whitespace().next() {
                        out.push(n.to_owned());
                    }
                }
            }
        }
    }
    out
}

/// `registry.code_version(module)`: sha1 of the decoder's source plus everything
/// it uses from the package, first 12 hex digits. CRLF reads as LF, so a
/// Windows checkout hashes like the server's.
pub fn code_version(name: &str, src: &Path) -> Option<String> {
    let read = |m: &str| {
        std::fs::read(src.join(format!("{m}.py"))).ok().map(|b| {
            let mut out = Vec::with_capacity(b.len());
            for (i, &c) in b.iter().enumerate() {
                if !(c == b'\r' && b.get(i + 1) == Some(&b'\n')) {
                    out.push(c);
                }
            }
            out
        })
    };
    let mut found: BTreeMap<String, ()> = BTreeMap::new();
    let mut stack = vec![name.to_owned()];
    while let Some(m) = stack.pop() {
        let text = String::from_utf8_lossy(&read(&m)?).into_owned();
        for dep in imports(&text) {
            if loaded(&dep) && src.join(format!("{dep}.py")).is_file() && !found.contains_key(&dep) {
                found.insert(dep.clone(), ());
                stack.push(dep);
            }
        }
    }
    let mut order: Vec<String> = Vec::new();
    for m in std::iter::once(name.to_owned()).chain(ALWAYS.iter().map(|s| (*s).to_owned())).chain(found.into_keys()) {
        if !order.contains(&m) {
            order.push(m);
        }
    }
    let mut h = Sha1::new();
    for m in &order {
        h.update(read(m)?);
    }
    Some(hex::encode(h.finalize())[..12].to_owned())
}

/// The version recorded on a run: Python's hash while its sources ship, else
/// this build's.
pub fn version_of(name: &str, src: &Path) -> String {
    code_version(name, src).unwrap_or_else(|| {
        let mut h = Sha1::new();
        h.update(format!("kiwi-server:{name}:{}", env!("CARGO_PKG_VERSION")).as_bytes());
        hex::encode(h.finalize())[..12].to_owned()
    })
}
