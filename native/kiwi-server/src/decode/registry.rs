//! The decoders in Python's order, their file formats, and the version of each
//! one's code.

use std::path::{Path, PathBuf};

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
    /// Recorded on every run; a build reruns when it changes. Bump it by hand
    /// when the decoder's output changes.
    pub code_version: &'static str,
    pub title: &'static str,
    pub output: &'static str,
    pub prefixes: &'static [&'static str],
    pub indent: Option<usize>,
    pub final_newline: bool,
    pub build: fn(&Ctx) -> Built,
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

    pub fn run(&self, tree: &Tree, baseline: &Path) -> Built {
        (self.build)(&Ctx { tree: tree.only(self.prefixes), baseline: baseline.to_path_buf() })
    }
}

pub fn count_len(data: &J) -> Option<usize> {
    data.as_list().map(Vec::len)
}

pub fn count_key(key: &'static str) -> impl Fn(&J) -> Option<usize> {
    move |data: &J| data.get(key).map(J::len)
}

macro_rules! dec {
    ($name:literal, $version:literal, $title:literal, $output:literal, [$($p:literal),*], $indent:expr, $nl:expr, $build:expr, $count:expr) => {
        Decoder {
            name: $name,
            code_version: $version,
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
        "62b68e79fbc9",
        "Class levels",
        "classes.json",
        ["prefabs/class/"],
        Some(4),
        false,
        d::class_levels::build,
        count_len
    ),
    dec!(
        "class_abilities",
        "59196671d7f8",
        "Class abilities",
        "class_abilities.json",
        ["prefabs/class/", "prefabs/abilities/", "prefabs/sfx/", "languages/en/"],
        Some(4),
        false,
        d::class_abilities::build,
        count_len
    ),
    dec!(
        "ally_abilities",
        "26efbf0cfd4b",
        "Ally abilities",
        "ally_abilities.json",
        ["prefabs/collections/", "prefabs/abilities/", "prefabs/sfx/", "languages/en/"],
        Some(4),
        false,
        d::ally_abilities::build,
        count_len
    ),
    dec!(
        "gem_abilities",
        "5f20faa90b7a",
        "Gem abilities",
        "gem_abilities.json",
        ["prefabs/item/gem/large/", "prefabs/abilities/", "prefabs/class/", "prefabs/sfx/", "languages/en/"],
        Some(4),
        false,
        d::gem_abilities::build,
        count_len
    ),
    dec!(
        "ring_abilities",
        "955f7e5ae34a",
        "Ring abilities",
        "ring_abilities.json",
        ["prefabs/abilities/", "prefabs/class/", "prefabs/sfx/", "languages/en/"],
        Some(4),
        false,
        d::ring_abilities::build,
        count_len
    ),
    dec!(
        "mount_abilities",
        "b49eb4ed6e3d",
        "Mount abilities",
        "mount_abilities.json",
        ["prefabs/collections/", "prefabs/abilities/", "prefabs/sfx/", "languages/en/"],
        Some(4),
        false,
        d::mount_abilities::build,
        count_len
    ),
    dec!(
        "pvp_stat_ranges",
        "5119a17aeff7",
        "PvP stat curves",
        "pvp.json",
        ["prefabs/pvp/data/stats/"],
        Some(1),
        true,
        d::pvp_stat_ranges::build,
        d::pvp_stat_ranges::count
    ),
    dec!(
        "delve_modifiers",
        "7e4146b97192",
        "Delve modifiers",
        "delve_modifiers.json",
        ["prefabs/placeable/deco/delve/", "prefabs/abilities/delve/", "languages/en/"],
        Some(1),
        true,
        d::delve_modifiers::build,
        |j| j.get("modifiers").map(J::len)
    ),
    dec!(
        "companions",
        "7c81fd9f7764",
        "Companions",
        "companions.json",
        ["prefabs/collections/", "prefabs/item/", "prefabs/upgrade/", "prefabs/abilities/", "languages/en/"],
        Some(4),
        false,
        d::companions::build,
        count_len
    ),
    dec!(
        "badges",
        "e0915cb030a8",
        "Badges",
        "badges.json",
        ["prefabs/", "languages/en/", "Trove_x64.exe"],
        Some(4),
        false,
        d::badges::build,
        count_len
    ),
    dec!(
        "fish",
        "7a9bc082b9cb",
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
        d::fish::build,
        |j| j.get("fish").map(J::len)
    ),
    dec!(
        "mementos",
        "88105931bf21",
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
        d::mementos::build,
        |j| j.get("mementos").map(J::len)
    ),
    dec!(
        "recipes",
        "2f1ba6f31d72",
        "Recipes",
        "recipes.json",
        ["prefabs/", "languages/en/"],
        None,
        true,
        d::recipes::build,
        |j| Some(j.get("recipes").map_or(0, J::len))
    ),
    dec!(
        "collectibles",
        "db45f070aafc",
        "Wings, boats, sails and auras",
        "collectibles.json",
        ["prefabs/collections/", "prefabs/abilities/", "prefabs/sfx/", "languages/en/", "particles/VFX/Particles/"],
        Some(4),
        false,
        d::collectibles::build,
        |j| j.as_dict().map(|m| m.values().map(J::len).sum())
    ),
    dec!(
        "cosmetics",
        "e0afe740003d",
        "Costumes and styles",
        "cosmetics.json",
        ["prefabs/skins/", "prefabs/loot/", "prefabs/collections/", "prefabs/equipment/", "languages/en/"],
        Some(4),
        false,
        d::cosmetics::build,
        d::cosmetics::count
    ),
    dec!(
        "titles",
        "bebad39bf1af",
        "Titles",
        "titles.json",
        ["languages/en/"],
        Some(4),
        false,
        d::titles::build,
        count_len
    ),
    dec!(
        "npcs",
        "a04f7d5830bf",
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
        d::npcs::build,
        |j| j.get("npcs").map(J::len)
    ),
    dec!(
        "placeables",
        "3ef46efa98d1",
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
        d::placeables::build,
        count_placeables
    ),
    dec!(
        "quests",
        "bdcd852e2ef4",
        "Quests and Adventures",
        "quests.json",
        ["prefabs/", "languages/en/", "Trove_x64.exe"],
        Some(4),
        false,
        d::quests::build,
        count_groups
    ),
    dec!(
        "worlds",
        "ea604a032d1a",
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
        d::worlds::build,
        count_worlds
    ),
    dec!(
        "subclass_abilities",
        "ff535a44fe0e",
        "Subclass abilities",
        "subclass_abilities.json",
        ["prefabs/class/", "prefabs/abilities/subclass/", "languages/en/"],
        Some(4),
        false,
        d::subclass_abilities::build,
        count_len
    ),
    dec!(
        "daily_bonuses",
        "415aae8c0a79",
        "Daily bonuses",
        "daily_bonuses.json",
        ["prefabs/loot/", "ui/", "languages/en/"],
        Some(1),
        true,
        d::daily_bonuses::build,
        |j| j.get("days").map(J::len)
    ),
    dec!(
        "leaderboards",
        "f800e09eebfd",
        "Leaderboards",
        "leaderboards.json",
        ["prefabs/leaderboard/", "languages/en/", "Trove_x64.exe"],
        Some(1),
        true,
        d::leaderboards::build,
        |j| j.get("boards").map(J::len)
    ),
    dec!(
        "gem_upgrades",
        "42996d587383",
        "Gem upgrades",
        "gem_upgrades.json",
        ["prefabs/gems/meta/", "prefabs/item/gem/", "prefabs/item/crafting/", "languages/en/"],
        Some(1),
        true,
        d::gem_upgrades::build,
        |j| j.get("gems").map(J::len)
    ),
    dec!(
        "upgrade_trees",
        "f972574c2bd3",
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
        d::upgrade_trees::build,
        count_upgrade_trees
    ),
    dec!(
        "items",
        "fa7f2bc3493e",
        "Items",
        "items.json",
        ["prefabs/", "languages/en/"],
        None,
        true,
        d::items::build,
        |j| j.get("items").map(J::len)
    ),
    dec!(
        "mastery",
        "5bcabbd0121e",
        "Mastery",
        "mastery.json",
        ["prefabs/meta/", "prefabs/claim/", "prefabs/collections/", "prefabs/item/", "languages/en/"],
        None,
        true,
        d::mastery::build,
        count_mastery
    ),
    dec!(
        "class_rewards",
        "ddae06d7e506",
        "Class rewards",
        "class_rewards.json",
        ["prefabs/class/", "prefabs/claim/", "prefabs/abilities/", "prefabs/item/", "languages/en/"],
        Some(1),
        true,
        d::class_rewards::build,
        count_class_rewards
    ),
    dec!(
        "lootbox_odds",
        "bc3365605457",
        "Lootbox odds",
        "lootbox_odds.json",
        ["languages/en/"],
        Some(1),
        true,
        d::lootbox_odds::build,
        |j| j.get("boxes").map(J::len)
    ),
    dec!(
        "pvp_powerups",
        "5e98944a4145",
        "PvP power-ups",
        "pvp_powerups.json",
        ["prefabs/pvp/", "prefabs/abilities/", "prefabs/effects/", "prefabs/sfx/", "languages/en/"],
        Some(1),
        true,
        d::pvp_powerups::build,
        |j| j.get("powerups").map(J::len)
    ),
    dec!(
        "chat_commands",
        "73e6a1a67571",
        "Chat commands",
        "chat_commands.json",
        ["prefabs/"],
        Some(1),
        true,
        d::chat_commands::build,
        |j| j.get("commands").map(J::len)
    ),
    dec!(
        "insignia",
        "a288ae0567d3",
        "Sigils",
        "insignia.json",
        ["prefabs/meta/insignia.binfab", "ui/nameplate.swf"],
        Some(1),
        true,
        d::insignia::build,
        d::insignia::count
    ),
    dec!(
        "gear",
        "1150e62f42c0",
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
        d::gear::build,
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
