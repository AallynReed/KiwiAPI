//! `delve_modifiers.py`: every delve modifier, named from its in-game sign.

use std::sync::OnceLock;

use indexmap::IndexMap;
use regex::{Regex, bytes::Regex as BRegex};

use crate::{
    decode::{
        common::{extract_localization_map, operation, stat_keys, unzig},
        json::{J, Map},
        registry::{Built, Ctx, DecodeError},
    },
    obj,
};

const SIGN_DIR: &str = "prefabs/placeable/deco/delve/";

fn sign_re() -> &'static BRegex {
    static R: OnceLock<BRegex> = OnceLock::new();
    R.get_or_init(|| {
        BRegex::new(r"(?-u)\$prefabs_placeable_deco_delve_([A-Za-z0-9_]+?)_interactable_sign_title").expect("regex")
    })
}

fn mod_re() -> &'static BRegex {
    static R: OnceLock<BRegex> = OnceLock::new();
    R.get_or_init(|| BRegex::new(r"(?s-u)(.)\x10(.)\x24(.{4})\x38(.)").expect("regex"))
}

fn boss_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^deltalith_boss(IncreaseDamage|ReduceIncDmg)_(\d+)$").expect("regex"))
}

type Effects = Vec<(&'static str, String)>;
type Signs = IndexMap<String, (&'static str, Effects)>;

/// sign id -> (group, [(who, effect prefab under abilities/delve/)]).
fn signs() -> &'static Signs {
    static S: OnceLock<Signs> = OnceLock::new();
    S.get_or_init(|| {
        let mut m: Signs = IndexMap::new();
        let mut add = |id: &str, group: &'static str, effects: Vec<(&'static str, String)>| {
            m.insert(id.to_owned(), (group, effects));
        };
        let none = Vec::new;
        let one = |who: &'static str, rel: &str| vec![(who, rel.to_owned())];
        for id in ["agile", "antisolo2", "antumbral", "arena"] {
            add(id, "creature", none());
        }
        add("berserker", "creature", one("Per ally slain", "mutators/berserker_onallykilled_stats"));
        add("berserkerstacking", "creature", one("Per ally slain", "mutators/berserkerstacking_onallykilled_stats"));
        add("buffondeath", "creature", none());
        add("debuffonranged", "creature", one("Ranged attackers", "mutators/debuffonrangeddamage_shield_debuff"));
        for id in [
            "deltalith_miniBoss",
            "fragile",
            "icyground_onaggro",
            "physicalimmune",
            "physicalresistant",
            "rangedimmune",
        ] {
            add(id, "creature", none());
        }
        add("slowplayersonhit", "creature", one("Players it hits", "mutators/daze_onoutgoingdamage_movementspeed"));
        add("spawnclone", "creature", one("Each clone", "mutators/spawnclone_ondamage_stats"));
        add("spawnmushrooms", "creature", one("Each Sporeling", "mutators/spawnmushroom_periodic_stats"));
        add("spawnshadowchickens", "creature", one("Each chicken", "mutators/spawnchicken_onaggro_stats"));
        for id in ["spawnturret", "spellimmune", "spellresistant", "tenebrous", "tenebrous2", "tenebrous3", "weak"] {
            add(id, "creature", none());
        }
        add("berserkerboss", "lair", one("Per threshold", "boss/mutators/berserker_onhealthpercent_stats"));
        for id in ["deltalith_bossAntiGrav", "deltalith_bossFloorIce"] {
            add(id, "lair", none());
        }
        add("deltalith_bossFloorLavaLong", "lair", one("Players caught", "mutators/shared_disable_healthregen_15s"));
        add("deltalith_bossFloorLavaShort", "lair", one("Players caught", "mutators/shared_disable_healthregen_10s"));
        for id in [
            "deltalith_bossIncreaseDamage_15",
            "deltalith_bossIncreaseDamage_30",
            "deltalith_bossIncreaseDamage_45",
            "deltalith_bossReduceIncDmg_15",
            "deltalith_bossReduceIncDmg_30",
            "deltalith_bossReduceIncDmg_45",
            "deltalith_bossReduceIncDmg_60",
            "deltalith_bossReduceIncDmg_75",
            "deltalith_bossTimeBomb_180",
            "deltalith_graveyard_creaturAoeRoot",
            "deltalith_stopDpsStun",
        ] {
            add(id, "lair", none());
        }
        for id in ["flaskrefiller", "pathExplosions", "pathNoChevrons", "speedblocks"] {
            add(id, "path", none());
        }
        for n in 1..14 {
            add(
                &format!("increaseplayerdamage_{n:02}"),
                "player",
                one("", &format!("mutators/tier_player_set_incomingdamagemod_t{n:02}")),
            );
        }
        add("flasks_0", "player", one("", "mutators/tier_player_set_flasks_0"));
        for n in [1, 2, 3] {
            add(&format!("flasks_n{n:02}"), "player", one("", &format!("mutators/tier_player_sub_flasks_{n:02}")));
        }
        for j in ["0", "2", "n1", "n3", "n5"] {
            add(&format!("jump_{j}"), "player", one("", &format!("mutators/jump_{j}")));
        }
        add("deltalith_movementSpeed_40", "player", one("", "mutators/tier_player_set_movementspeed_40"));
        add("reducehealing", "player", one("", "mutators/tier_player_reducehealing_noregen"));
        for id in [
            "vampirism",
            "deltalith_death_boon",
            "deltalith_death_curse",
            "deltalith_group_death",
            "disabledeathsaveeffect",
            "respawnSlower",
        ] {
            add(id, "player", none());
        }
        for id in ["lootlevel", "fauxlootlevel"] {
            add(id, "tier", none());
        }
        m
    })
}

fn legacy() -> &'static IndexMap<String, String> {
    static L: OnceLock<IndexMap<String, String>> = OnceLock::new();
    L.get_or_init(|| {
        let mut m: IndexMap<String, String> = [
            ("DelveCreatureMod_Agile", "agile"),
            ("DelveCreatureMod_MiniBoss", "deltalith_miniBoss"),
            ("DelveCreatureMod_Antumbral", "antumbral"),
            ("DelveCreatureMod_Arena", "arena"),
            ("DelveCreatureMod_Fragile", "fragile"),
            ("DelveCreatureMod_icyGround", "icyground_onaggro"),
            ("DelveCreatureMod_berserker", "berserker"),
            ("DelveCreatureMod_berserkerHard", "berserkerstacking"),
            ("DelveCreatureMod_antiSolo2", "antisolo2"),
            ("DelveCreatureMod_slowPlayersOnHit", "slowplayersonhit"),
            ("DelveCreatureMod_buffOnDeath", "buffondeath"),
            ("DelveCreatureMod_RangedProtection", "rangedimmune"),
            ("DelveCreatureMod_debuffOnRangedDamage", "debuffonranged"),
            ("DelveCreatureMod_ShadowChickens", "spawnshadowchickens"),
            ("DelveCreatureMod_SpawnClone", "spawnclone"),
            ("DelveCreatureMod_SpawnMushrooms", "spawnmushrooms"),
            ("DelveCreatureMod_SummonTurret", "spawnturret"),
            ("DelveCreatureMod_Tenebrous", "tenebrous"),
            ("DelveCreatureMod_Weak", "weak"),
            ("DelveCreatureMod_SpellResistant", "spellresistant"),
            ("DelveCreatureMod_SpellImmune", "spellimmune"),
            ("DelveCreatureMod_PhysicalResistant", "physicalresistant"),
            ("DelveCreatureMod_PhysicalImmune", "physicalimmune"),
            ("DelvePathMod_noChevrons", "pathNoChevrons"),
            ("DelvePathMod_FlaskRefiller", "flaskrefiller"),
            ("DelvePathMod_SpeedBlocks", "speedblocks"),
            ("DelvePathMod_explosions", "pathExplosions"),
            ("DelveLairMod_bossBerserker", "berserkerboss"),
            ("DelveLairMod_floorToLavaShort", "deltalith_bossFloorLavaShort"),
            ("DelveLairMod_floorToLavaLong", "deltalith_bossFloorLavaLong"),
            ("DelveLairMod_floorToIce", "deltalith_bossFloorIce"),
            ("DelveLairMod_Gravity", "deltalith_bossAntiGrav"),
            ("DelveLairMod_graveyardAoeRoot", "deltalith_graveyard_creaturAoeRoot"),
            ("DelveLairMod_stopDpsStun", "deltalith_stopDpsStun"),
        ]
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
        for n in [15, 30, 45] {
            m.insert(format!("DelveLairMod_increaseIncDmg_{n}"), format!("deltalith_bossIncreaseDamage_{n}"));
        }
        for n in [15, 30, 45, 60, 75] {
            m.insert(format!("DelveLairMod_reduceIncDmg_{n}"), format!("deltalith_bossReduceIncDmg_{n}"));
        }
        m
    })
}

/// `$Stat_` key -> (label, scale, how Add reads).
fn stats(stat: &str) -> Option<(&'static str, f64, &'static str)> {
    Some(match stat {
        "IncomingDamageMod" => ("Damage taken", 1.0, "mult"),
        "OutgoingDamageMod" => ("Damage dealt", 1.0, "mult"),
        "PhysicalDamage" => ("Physical damage", 1.0, "flat"),
        "SpellDamage" => ("Magic damage", 1.0, "flat"),
        "MaxHealth" => ("Maximum health", 1.0, "flat"),
        "MovementSpeed" => ("Movement speed", 1.0, "flat"),
        "HealthRegen_controller" => ("Health regen", 1.0, "flat"),
        "CriticalHitChance" => ("Critical hit", 0.1, "pct"),
        "Jump" => ("Jump", 1.0, "flat"),
        "Glide" => ("Glide", 1.0, "flat"),
        "MaxFlasks" => ("Flasks", 1.0, "flat"),
        _ => return None,
    })
}

/// `f"{v:,.Nf}"`.
fn fixed_commas(v: f64, places: usize) -> String {
    let text = format!("{:.*}", places, v);
    let (sign, body) = match text.strip_prefix('-') {
        Some(b) => ("-", b),
        None => ("", text.as_str()),
    };
    let (int_part, frac) = body.split_once('.').map_or((body, None), |(a, b)| (a, Some(b)));
    let mut out = sign.to_owned();
    out.push_str(&crate::decode::common::group_digits(int_part));
    if let Some(f) = frac {
        out.push('.');
        out.push_str(f);
    }
    out
}

/// `_num`: two places, thousands separated, trailing zeros dropped.
fn num(v: f64) -> String {
    let v = crate::pyfloat::round(v, 2);
    if v == v.trunc() { fixed_commas(v, 0) } else { fixed_commas(v, 2).trim_end_matches('0').to_owned() }
}

fn stat_mods(data: &[u8]) -> Vec<(&'static str, &'static str, f64)> {
    let mut out = Vec::new();
    for m in mod_re().captures_iter(data) {
        let stat = stat_keys(unzig(i128::from(m[1][0]))).map_or("", |k| k.strip_prefix("$Stat_").unwrap_or(k));
        let op = operation(i128::from(m[2][0]));
        let end = m.get(0).expect("m").end();
        let len = usize::from(m[4][0]);
        let label = &data[end.min(data.len())..(end + len).min(data.len())];
        if !stat.is_empty()
            && let Some(op) = op
            && label.iter().all(|b| (0x20..=0x7e).contains(b))
        {
            let raw = &m[3];
            out.push((stat, op, f64::from(f32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))));
        }
    }
    out
}

fn effect_lines(data: &[u8]) -> Result<Vec<String>, DecodeError> {
    let mut lines = Vec::new();
    for (stat, op, value) in stat_mods(data) {
        if op == "Nullify" {
            continue;
        }
        let Some((label, scale, kind)) = stats(stat) else {
            return Err(DecodeError::value(format!("unmapped stat {stat}")));
        };
        let value = value * scale;
        if op == "Multiply" || (op == "Set" && kind == "mult") {
            lines.push(format!("{label} ×{}", num(value)));
        } else if op == "Set" {
            lines.push(format!("{label} set to {}", num(value)));
        } else if op == "Add" && (kind == "mult" || kind == "pct") {
            let pct = if kind == "mult" { value * 100.0 } else { value };
            lines.push(format!("{label} {}{}%", if pct >= 0.0 { "+" } else { "−" }, num(pct.abs())));
        } else if op == "Add" {
            lines.push(format!("{label} {}{}", if value >= 0.0 { "+" } else { "−" }, num(value.abs())));
        } else if op == "MultiplySum" {
            lines.push(format!("{label} +{}%", num(value * 100.0)));
        } else {
            return Err(DecodeError::value(format!("unhandled {op} on {stat}")));
        }
    }
    Ok(lines)
}

fn roman(tail: &str) -> Option<i64> {
    Some(match tail {
        "I" => 1,
        "II" => 2,
        "III" => 3,
        "IV" => 4,
        "V" => 5,
        "VI" => 6,
        "VII" => 7,
        "VIII" => 8,
        "IX" => 9,
        "X" => 10,
        "XI" => 11,
        "XII" => 12,
        "XIII" => 13,
        _ => return None,
    })
}

fn natural(name: &str) -> (String, i64) {
    if let Some((head, tail)) = name.rsplit_once(' ')
        && !head.is_empty()
        && let Some(n) = roman(tail)
    {
        return (head.to_lowercase(), n);
    }
    (name.to_lowercase(), 0)
}

pub fn build(ctx: &Ctx) -> Built {
    let tree = &ctx.tree;
    let mut loc = IndexMap::new();
    for path in tree.files("languages/en/prefabs_placeable_deco_delve", ".binfab") {
        loc.extend(extract_localization_map(&tree.read(&path).unwrap_or_default()));
    }
    let legacy_loc = extract_localization_map(&tree.read("languages/en/delve.binfab").unwrap_or_default());
    let effect_loc =
        extract_localization_map(&tree.read("languages/en/prefabs_effects_delve.binfab").unwrap_or_default());
    let prefab = |rel: &str| {
        tree.read(&format!("prefabs/abilities/delve/{rel}.binfab"))
            .ok_or_else(|| DecodeError::value(format!("missing prefab abilities/delve/{rel}")))
    };
    let mut found: IndexMap<String, (String, String)> = IndexMap::new();
    for path in tree.files(SIGN_DIR, "_interactable.binfab") {
        let sid = if path.matches('/').count() == 4 {
            let data = tree.read(&path).unwrap_or_default();
            sign_re().captures(&data).map(|m| String::from_utf8_lossy(&m[1]).into_owned())
        } else {
            None
        };
        let Some(sid) = sid.filter(|s| !s.is_empty()) else { continue };
        let base = format!("$prefabs_placeable_deco_delve_{sid}_interactable_sign");
        if let Some(title) = loc.get(&format!("{base}_title")).filter(|t| !t.is_empty()) {
            let content = loc.get(&format!("{base}_content")).cloned().unwrap_or_default();
            found.insert(sid, (title.clone(), content));
        }
    }
    let mut missing: Vec<&String> = signs().keys().filter(|k| !found.contains_key(*k)).collect();
    if !missing.is_empty() {
        missing.sort();
        let list: Vec<String> = missing.iter().map(|m| format!("'{m}'")).collect();
        return Err(DecodeError::value(format!("signs no longer in the game: [{}]", list.join(", "))));
    }
    let mut modifiers: Vec<Map> = Vec::new();
    for (sid, (name, description)) in &found {
        let (group, effects) = signs().get(sid).map_or(("new", Vec::new()), |(g, e)| (*g, e.clone()));
        let mut rows = Vec::new();
        for (who, rel) in effects {
            rows.push(obj! {"who" => who, "lines" => J::from(effect_lines(&prefab(&rel)?)?)});
        }
        if let Some(m) = boss_re().captures(sid) {
            let sign = if &m[1] == "IncreaseDamage" { "+" } else { "−" };
            rows.push(obj! {"who" => "The boss", "lines" => vec![J::from(format!("Damage taken {sign}{}%", &m[2]))]});
        }
        let mut row = Map::new();
        row.insert("key".into(), J::from(sid.as_str()));
        row.insert("category".into(), J::from(group));
        row.insert("name".into(), J::from(name.as_str()));
        row.insert("description".into(), J::from(description.as_str()));
        row.insert("effects".into(), J::List(rows));
        modifiers.push(row);
    }
    for (key, name) in &legacy_loc {
        let key = &key[1..];
        let group = [("DelveCreatureMod_", "creature"), ("DelveLairMod_", "lair"), ("DelvePathMod_", "path")]
            .iter()
            .find(|(p, _)| key.starts_with(p))
            .map(|(_, g)| *g);
        let Some(group) = group else { continue };
        match legacy().get(key) {
            None => {
                let mut row = Map::new();
                row.insert("key".into(), J::from(key));
                row.insert("category".into(), J::from(group));
                row.insert("name".into(), J::from(name.as_str()));
                row.insert("description".into(), J::from(""));
                row.insert("effects".into(), J::List(Vec::new()));
                modifiers.push(row);
            }
            Some(sign) if !found.contains_key(sign) => {
                return Err(DecodeError::value(format!("{key} maps to a sign that is gone: {sign}")));
            }
            Some(_) => {}
        }
    }
    let order =
        |c: &str| ["creature", "lair", "path", "player", "tier", "new"].iter().position(|g| *g == c).unwrap_or(0);
    modifiers.sort_by_cached_key(|m| {
        (order(m["category"].as_str().unwrap_or("")), natural(m["name"].as_str().unwrap_or("")))
    });
    let pressure = obj! {
        "name" => effect_loc.get("$prefabs_effects_delve_healthregen_scaling_name").cloned().ok_or_else(|| DecodeError::key("$prefabs_effects_delve_healthregen_scaling_name"))?,
        "description" => effect_loc.get("$prefabs_effects_delve_healthregen_scaling_description").cloned().ok_or_else(|| DecodeError::key("$prefabs_effects_delve_healthregen_scaling_description"))?,
    };
    Ok(obj! {"modifiers" => J::List(modifiers.into_iter().map(J::Dict).collect()), "pressure" => pressure})
}
