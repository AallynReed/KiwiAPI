//! `app/trove/codexes/badges.py`: badge rank requirements from `meta/badges.binfab`
//! and the `PlayerMetric` enum from the client executable.

use std::collections::{BTreeSet, HashSet};

use serde_json::{Map, Value, json};

use super::{
    int_value, py,
    wire::{read_varint, unzig},
};

pub const EXE_PATH: &str = "Trove_x64.exe";
const METRIC_SENTINEL: &[u8] = b"KPlayerMetric\x00";

const RANK_MARKER: &[u8] = b"\xbe\x02";
const KIND_MARKER: &[u8] = b"\x08\xae\x03\x08";
const PAYLOAD_MARKER: &[u8] = b"\xae\x01\x1e\x00";
const PAYLOAD_MARKER_STR: &[u8] = b"\xae\x01\x1e\x08";

fn rank_name(rank: u32) -> String {
    match rank {
        1 => "bronze".into(),
        2 => "silver".into(),
        3 => "gold".into(),
        4 => "platinum".into(),
        5 => "diamond".into(),
        6 => "obsidian".into(),
        7 => "trovium".into(),
        r => format!("rank{r}"),
    }
}

fn is_ident(b: &[u8]) -> bool {
    !b.is_empty()
        && (b[0].is_ascii_alphabetic() || b[0] == b'_')
        && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_')
}

/// `re.search(rb"[A-Za-z_][A-Za-z0-9_]*$", run)`.
fn trailing_ident(run: &[u8]) -> Option<&[u8]> {
    let body = match run.last() {
        Some(b'\n') => &run[..run.len() - 1],
        _ => run,
    };
    let mut start = body.len();
    while start > 0 && (body[start - 1].is_ascii_alphanumeric() || body[start - 1] == b'_') {
        start -= 1;
    }
    let word = &body[start..];
    let first = word.iter().position(|c| c.is_ascii_alphabetic() || *c == b'_')?;
    Some(&word[first..])
}

/// `parse_metric_names`: index == ordinal, 0 unnamed; empty when the layout differs.
pub fn parse_metric_names(exe: &[u8]) -> Vec<String> {
    let Some(end) = py::find(exe, METRIC_SENTINEL, 0) else { return Vec::new() };
    let mut names: Vec<String> = Vec::new();
    let mut pos = end;
    while pos > 0 {
        let mut stop = pos - 1;
        while stop > 0 && exe[stop] == 0 {
            stop -= 1;
        }
        if stop == 0 {
            break;
        }
        let mut start = stop;
        while start > 0 && (0x20..0x7F).contains(&exe[start - 1]) {
            start -= 1;
        }
        let run = &exe[start..=stop];
        if !is_ident(run) {
            if let Some(m) = trailing_ident(run)
                && m.len() >= 3
            {
                names.push(py::latin1(m));
            }
            break;
        }
        if run.len() < 3 {
            break;
        }
        names.push(py::latin1(run));
        pos = start;
    }
    if names.is_empty() {
        return Vec::new();
    }
    names.reverse();
    let mut out = vec![String::new()];
    out.extend(names);
    out
}

/// `display_metric`: `FlawlessHydraUltraKills` -> `Flawless Hydra Ultra Kills`.
pub fn display_metric(name: &str) -> String {
    let c: Vec<char> = name.chars().collect();
    let mut out = String::new();
    for (i, &ch) in c.iter().enumerate() {
        if i > 0 && ch.is_ascii_uppercase() && (c[i - 1].is_ascii_lowercase() || c[i - 1].is_ascii_digit()) {
            out.push(' ');
        }
        out.push(ch);
    }
    py::strip(&out).to_owned()
}

fn read_zigzag(data: &[u8], pos: usize, limit: usize) -> (Option<i128>, usize) {
    match read_varint(&data[..limit.min(data.len())], pos) {
        (Some(v), nxt) => (Some(unzig(v)), nxt),
        (None, _) => (None, pos),
    }
}

fn read_string(data: &[u8], pos: usize, limit: usize) -> (Option<String>, usize) {
    if pos >= limit {
        return (None, pos);
    }
    let length = data[pos] as usize;
    let pos = pos + 1;
    if pos + length > limit {
        return (None, pos);
    }
    (Some(py::latin1(&data[pos..pos + length])), pos + length)
}

fn index_pattern(index: i64) -> Vec<u8> {
    let mut code1 = index * 16 - 12;
    let mut code2 = code1 / 128;
    let code3 = code2 / 128;
    if code1 > 128 {
        code1 -= (code1 / 128 - 1) * 128;
    }
    if code2 >= 256 {
        code2 -= (code2 / 128 - 1) * 128;
    }
    let mut out = vec![code1 as u8];
    if code2 > 0 {
        out.push(code2 as u8);
    }
    if code3 > 0 {
        out.push(code3 as u8);
    }
    out.push(0x08);
    out
}

fn find_before(data: &[u8], needle: &[u8], start: usize, end: usize) -> Option<usize> {
    py::find(data, needle, start).filter(|&at| at < end)
}

struct Group {
    start: usize,
    ranks_start: usize,
    badge_id: String,
    rank_count: u8,
    end: usize,
}

struct Layout {
    reward_ids: Vec<String>,
    groups: Vec<Group>,
    errors: Vec<String>,
}

fn layout(data: &[u8]) -> Layout {
    let mut out = Layout { reward_ids: Vec::new(), groups: Vec::new(), errors: Vec::new() };
    let n = data.len();
    if n < 16 {
        out.errors.push("badges.binfab is too short".into());
        return out;
    }
    let (count, mut pos) = read_varint(data, 2);
    let count = match count {
        Some(c) if c > 0 && c <= 5000 => c as i64,
        _ => {
            out.errors.push("reward count is out of range".into());
            return out;
        }
    };
    for i in 1..=count {
        let pattern = index_pattern(i);
        let Some(found) = py::find(data, &pattern, pos) else {
            out.errors.push(format!("reward row {i} not found"));
            return out;
        };
        let p = found + pattern.len();
        let (reward_id, mut p) = read_string(data, p, n);
        let Some(reward_id) = reward_id else {
            out.errors.push(format!("reward id {i} unreadable"));
            return out;
        };
        out.reward_ids.push(reward_id);
        p += 1;
        for _ in 0..2 {
            let (value, q) = read_string(data, p, n);
            if value.is_none() {
                out.errors.push(format!("reward metadata {i} unreadable"));
                return out;
            }
            p = q + 1;
        }
        pos = p;
    }
    let badge_count = match read_varint(data, pos + 4).0 {
        Some(c) if c > 0 && c <= 1000 => c as i64,
        _ => {
            out.errors.push("badge group count is out of range".into());
            return out;
        }
    };
    let mut search = pos + 4;
    for i in 1..=badge_count {
        let pattern = index_pattern(i);
        let Some(found) = py::find(data, &pattern, search) else {
            out.errors.push(format!("badge group {i} not found"));
            return out;
        };
        let p = found + pattern.len();
        let (badge_id, p) = read_string(data, p, n);
        let Some(badge_id) = badge_id else {
            out.errors.push(format!("badge group id {i} unreadable"));
            return out;
        };
        let p = p + 3;
        if p >= n {
            out.errors.push(format!("badge group {i} rank count missing"));
            return out;
        }
        let rank_count = data[p];
        out.groups.push(Group { start: found, ranks_start: p + 2, badge_id, rank_count, end: n });
        search = p + 2;
    }
    let starts: Vec<usize> = out.groups.iter().map(|g| g.start).collect();
    for (i, g) in out.groups.iter_mut().enumerate() {
        g.end = starts.get(i + 1).copied().unwrap_or(n);
    }
    out
}

/// What a payload decoder reports; `None` fields are absent from Python's dict.
#[derive(Default)]
struct Decoded {
    decoder: Option<&'static str>,
    requirement_key: Option<String>,
    label: Option<String>,
    amount: Option<Value>,
    difficulty: Option<i128>,
    context: Option<Value>,
    collection: Option<&'static str>,
    status: &'static str,
    blocker: Option<&'static str>,
    reason: Option<String>,
}

fn blocked(reason: &str) -> Decoded {
    Decoded {
        status: "blocked",
        blocker: Some("source_parse_incomplete"),
        reason: Some(reason.into()),
        ..Default::default()
    }
}

fn decoded(decoder: &'static str, key: String, label: String, amount: Value, context: Value) -> Decoded {
    Decoded {
        decoder: Some(decoder),
        requirement_key: Some(key),
        label: Some(label),
        amount: Some(amount),
        context: Some(context),
        status: "decoded",
        blocker: Some(""),
        reason: Some(String::new()),
        ..Default::default()
    }
}

fn opt_int(v: Option<i128>) -> Value {
    v.map_or(Value::Null, int_value)
}

fn payload_at(data: &[u8], marker: &[u8], start: usize, end: usize) -> Option<usize> {
    find_before(data, marker, start, end).map(|at| at + marker.len())
}

fn metric(data: &[u8], start: usize, end: usize, names: &[String]) -> Decoded {
    let Some(at) = payload_at(data, PAYLOAD_MARKER, start, end) else {
        return blocked("metric payload marker not found");
    };
    let (encoded, p) = read_varint(data, at);
    let encoded = match encoded {
        Some(e) if p < end && data[p] == 0x10 => e,
        _ => return blocked("metric amount field missing"),
    };
    let (amount, _) = read_zigzag(data, p + 1, end);
    let index = (encoded % 2 == 0).then_some(encoded / 2);
    let key = index.and_then(|i| usize::try_from(i).ok()).and_then(|i| names.get(i)).cloned().unwrap_or_default();
    let ok = !key.is_empty();
    Decoded {
        decoder: Some("metric"),
        label: Some(display_metric(&key)),
        requirement_key: Some(key),
        amount: Some(opt_int(amount)),
        context: Some(
            json!({"metric_index": opt_int(index.map(|i| i as i128)), "encoded_metric": int_value(encoded as i128)}),
        ),
        status: if ok { "decoded" } else { "blocked" },
        blocker: Some(if ok { "" } else { "metric_name_unresolved" }),
        reason: Some(if ok { String::new() } else { "metric ordinal has no name in the client enum table".into() }),
        ..Default::default()
    }
}

fn dragon_souls(data: &[u8], start: usize, end: usize, badge_id: &str) -> Decoded {
    let Some(at) = payload_at(data, PAYLOAD_MARKER, start, end) else {
        return blocked("dragon souls payload marker not found");
    };
    let (shape, p) = read_varint(data, at);
    let shape = match shape {
        Some(s) if p < end && data[p] == 0x10 => s,
        _ => return blocked("dragon souls amount field missing"),
    };
    let (amount, _) = read_zigzag(data, p + 1, end);
    let label = sub_end(badge_id, "_badge", "");
    let label = label.strip_prefix("dragon_").unwrap_or(&label);
    let label = py::title(&label.replace('_', " "));
    decoded(
        "dragonsouls",
        format!("{badge_id}:dragonsouls"),
        format!("{label} Dragon Souls"),
        opt_int(amount),
        json!({"shape": int_value(shape as i128)}),
    )
}

/// `re.sub(suffix + "$", repl, s)`: Python's `$` also matches before a final newline.
fn sub_end(s: &str, suffix: &str, repl: &str) -> String {
    if let Some(t) = s.strip_suffix(suffix) {
        return format!("{t}{repl}");
    }
    if let Some(body) = s.strip_suffix('\n')
        && let Some(t) = body.strip_suffix(suffix)
    {
        return format!("{t}{repl}\n");
    }
    s.to_owned()
}

fn st_boss(data: &[u8], start: usize, end: usize) -> Decoded {
    let Some(at) = payload_at(data, PAYLOAD_MARKER_STR, start, end) else {
        return blocked("Shadow Tower boss payload marker not found");
    };
    let (boss, p) = read_string(data, at, end);
    let boss = match boss {
        Some(b) if p < end && data[p] == 0x10 => b,
        _ => return blocked("Shadow Tower boss payload incomplete"),
    };
    let (amount, p) = read_zigzag(data, p + 1, end);
    if amount.is_none() || p >= end || data[p] != 0x20 {
        return blocked("Shadow Tower difficulty field missing");
    }
    let (difficulty, _) = read_zigzag(data, p + 1, end);
    let d = difficulty.unwrap_or(0);
    let diff_name = match d {
        0 => "Normal",
        1 => "Hard",
        2 => "Ultra",
        _ => "Unknown",
    };
    let boss_name = match boss.as_str() {
        "quest/shadow_tower_boss_hydra.prefab" => "Shadow Hydrakken".to_owned(),
        "quest/shadow_tower_boss_dreadnought.prefab" => "Darknik Dreadnought".to_owned(),
        other => other.to_owned(),
    };
    let mut out = decoded(
        "stboss",
        format!("{boss}:{diff_name}"),
        format!("Defeat the {boss_name} on {diff_name} difficulty"),
        opt_int(amount),
        json!({"boss_prefab": boss, "difficulty": diff_name}),
    );
    out.difficulty = Some(d + 1);
    out
}

fn scalar(data: &[u8], start: usize, end: usize, kind: &str) -> Decoded {
    let Some(at) = payload_at(data, PAYLOAD_MARKER, start, end) else {
        return blocked("scalar payload marker not found");
    };
    let (amount, _) = read_zigzag(data, at, end);
    let label = match kind {
        "friends" => "Friends added",
        "loyalty" => "Loyalty",
        "referafriend" => "Referrals",
        k => k,
    };
    decoded("scalar", kind.to_owned(), label.to_owned(), opt_int(amount), json!({}))
}

fn two_values(data: &[u8], start: usize, end: usize) -> Option<(i128, i128)> {
    let at = payload_at(data, PAYLOAD_MARKER, start, end)?;
    let (first, p) = read_zigzag(data, at, end);
    let first = first?;
    if p >= end || data[p] != 0x10 {
        return None;
    }
    let (second, _) = read_zigzag(data, p + 1, end);
    Some((first, second?))
}

fn upgrade_gems(data: &[u8], start: usize, end: usize) -> Decoded {
    let at = match payload_at(data, PAYLOAD_MARKER, start, end) {
        Some(at) if at < end => at,
        _ => return blocked("upgrade gems payload marker not found"),
    };
    let scope = unzig(u128::from(data[at]));
    let p = at + 1;
    if p >= end || data[p] != 0x10 {
        return blocked("upgrade gems amount field missing");
    }
    let (amount, _) = read_zigzag(data, p + 1, end);
    let all = scope == 1;
    decoded(
        "upgradegems",
        format!("UpgradeGems:{}", if all { "all_gem_stats" } else { "single_gem_stat" }),
        if all {
            "Fully upgrade all stats to 100% on max level Stellar Gems".into()
        } else {
            "Fully upgrade max level Stellar Gems to 100%".into()
        },
        opt_int(amount),
        json!({"scope": int_value(scope)}),
    )
}

fn subclass(data: &[u8], start: usize, end: usize) -> Decoded {
    let Some((level, power_rank)) = two_values(data, start, end) else { return blocked("subclass payload incomplete") };
    let label = if level > 0 {
        format!("Upgrade 15 classes to Level {level} + Power Rank {power_rank}")
    } else {
        format!("Upgrade 15 classes to Power Rank {power_rank}")
    };
    decoded(
        "subclass",
        format!("SubClassEquipped:level={level}:power_rank={power_rank}"),
        label,
        json!(15),
        json!({"level": int_value(level), "power_rank": int_value(power_rank), "class_count": 15}),
    )
}

fn multi_class(data: &[u8], start: usize, end: usize) -> Decoded {
    let Some((level, classes)) = two_values(data, start, end) else { return blocked("multi-class payload incomplete") };
    decoded(
        "multi_class_paragon",
        format!("minprestigelevelsacrossclasses:level={level}:classes={classes}"),
        format!("Reach Paragon Level {level} with {classes} classes"),
        int_value(classes),
        json!({"paragon_level": int_value(level), "class_count": int_value(classes)}),
    )
}

fn entitlement(data: &[u8], start: usize, end: usize) -> Decoded {
    if !py::contains(data.get(start..end).unwrap_or_default(), b"PLAYERCLASS_CRIMEFIGHTER") {
        return blocked("entitlement class identifier not found");
    }
    let mut out = decoded(
        "entitlement",
        "PLAYERCLASS_CRIMEFIGHTER".into(),
        "Unlock the Vanguardian class".into(),
        json!(0),
        json!({"entitlement": "PLAYERCLASS_CRIMEFIGHTER"}),
    );
    out.collection = Some("collections/badge/hero_unlocked");
    out
}

fn tinyquest_pet_levels(data: &[u8], start: usize, end: usize) -> Decoded {
    let Some((level, count)) = two_values(data, start, end) else {
        return blocked("tinyquest pet-level payload incomplete");
    };
    let noun = if count == 1 { "Ally" } else { "Allies" };
    decoded(
        "tinyquest_pet_levels",
        "tinyquesttotalpetsoflevel".into(),
        format!("Reach level {level} with {count} {noun}"),
        int_value(count),
        json!({"ally_level": int_value(level), "ally_count": int_value(count)}),
    )
}

fn tinyquest_buffed_pets(data: &[u8], start: usize, end: usize) -> Decoded {
    let Some(at) = payload_at(data, PAYLOAD_MARKER, start, end) else {
        return blocked("tinyquest buffed-pets payload marker not found");
    };
    let (amount, _) = read_zigzag(data, at, end);
    let noun = if amount == Some(1) { "Ally" } else { "Allies" };
    let shown = amount.map_or("None".to_owned(), |a| a.to_string());
    decoded(
        "tinyquest_buffed_pets",
        "tinyquestconcurrentbuffedpets".into(),
        format!("Buff {shown} {noun} at once"),
        opt_int(amount),
        json!({"ally_count": opt_int(amount)}),
    )
}

#[allow(clippy::too_many_arguments)]
fn decode_payload(
    data: &[u8],
    kind: &str,
    after_kind: usize,
    rank_start: usize,
    rank_end: usize,
    badge_id: &str,
    names: &[String],
) -> Decoded {
    match kind {
        "metric" => metric(data, after_kind, rank_end, names),
        "dragonsouls" => dragon_souls(data, after_kind, rank_end, badge_id),
        "STBossKilled" => st_boss(data, after_kind, rank_end),
        "friends" | "loyalty" | "referafriend" => scalar(data, after_kind, rank_end, kind),
        "UpgradeGems" => upgrade_gems(data, after_kind, rank_end),
        "SubClassEquipped" => subclass(data, after_kind, rank_end),
        "minprestigelevelsacrossclasses" => multi_class(data, after_kind, rank_end),
        "entitlementUnlocked" => entitlement(data, after_kind, rank_end),
        "tinyquesttotalpetsoflevel" => tinyquest_pet_levels(data, rank_start, rank_end),
        "tinyquestconcurrentbuffedpets" => tinyquest_buffed_pets(data, rank_start, rank_end),
        "none" => Decoded {
            decoder: Some("descriptive"),
            requirement_key: Some("none".into()),
            label: Some(String::new()),
            amount: Some(json!(0)),
            context: Some(json!({})),
            status: "decoded",
            blocker: Some("descriptive"),
            reason: Some("no scalar requirement".into()),
            ..Default::default()
        },
        _ => Decoded {
            status: "blocked",
            blocker: Some("unsupported_completion_kind"),
            requirement_key: Some(kind.to_owned()),
            reason: Some(format!("completion kind {} is not decoded", crate::pystr::repr(kind))),
            ..Default::default()
        },
    }
}

fn reward_ids_in(segment: &[u8], known: &HashSet<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut pos = 0;
    let n = segment.len();
    while pos + 2 <= n {
        if segment[pos] & 0x0F != 4 {
            pos += 1;
            continue;
        }
        let length = segment[pos + 1] as usize;
        if length < 1 || pos + 2 + length > n {
            pos += 1;
            continue;
        }
        let candidate = py::latin1(&segment[pos + 2..pos + 2 + length]);
        if known.contains(&candidate) {
            if !out.contains(&candidate) {
                out.push(candidate);
            }
            pos += length + 2;
        } else {
            pos += 1;
        }
    }
    out
}

fn collection_for(reward_id: &str) -> String {
    if reward_id.is_empty() {
        return String::new();
    }
    let ident = sub_end(&sub_end(reward_id, "_plat_v2", "_platinum"), "_plat", "_platinum");
    let ident = ident.strip_prefix("badge_").unwrap_or(&ident);
    let ident = if ident == "hero_unlocked_badge" { "hero_unlocked" } else { ident };
    format!("collections/badge/{ident}")
}

fn rank_reward(rewards: &[String], rank_name: &str) -> String {
    let suffix = format!("_{rank_name}");
    rewards.iter().find(|r| r.ends_with(&suffix)).or(rewards.first()).cloned().unwrap_or_default()
}

/// One `(badge, rank)` row: the fields `requirement_rows` stores.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub collection: String,
    pub badge_id: String,
    pub rank: u32,
    pub rank_name: String,
    pub completion_kind: String,
    pub requirement_key: String,
    pub label: String,
    pub amount: Value,
    pub difficulty: i128,
    pub context: Value,
    pub status: String,
    pub blocker: String,
    pub reason: String,
}

#[allow(clippy::too_many_arguments)]
fn decode_rank(
    data: &[u8],
    g: &Group,
    rank: u32,
    rank_start: usize,
    rank_end: usize,
    known: &HashSet<String>,
    names: &[String],
) -> Row {
    let rname = rank_name(rank);
    let rewards = reward_ids_in(&data[rank_start..rank_end], known);
    let reward_id = rank_reward(&rewards, &rname);
    let mut row = Row {
        collection: collection_for(&reward_id),
        badge_id: g.badge_id.clone(),
        rank,
        rank_name: rname,
        completion_kind: String::new(),
        requirement_key: String::new(),
        label: String::new(),
        amount: Value::Null,
        difficulty: 0,
        context: json!({}),
        status: "blocked".into(),
        blocker: "source_parse_incomplete".into(),
        reason: String::new(),
    };
    let Some(kind_at) = find_before(data, KIND_MARKER, rank_start, rank_end) else {
        row.reason = format!("{} rank {rank}: completion kind marker not found", g.badge_id);
        return row;
    };
    let (kind, after_kind) = read_string(data, kind_at + KIND_MARKER.len(), rank_end);
    let Some(kind) = kind else {
        row.reason = format!("{} rank {rank}: completion kind unreadable", g.badge_id);
        return row;
    };
    row.completion_kind = kind.clone();
    let d = decode_payload(data, &kind, after_kind, rank_start, rank_end, &g.badge_id, names);
    if let Some(c) = d.collection {
        row.collection = c.to_owned();
    }
    if let Some(v) = d.requirement_key {
        row.requirement_key = v;
    }
    if let Some(v) = d.label {
        row.label = v;
    }
    if let Some(v) = d.amount {
        row.amount = v;
    }
    if let Some(v) = d.difficulty {
        row.difficulty = v;
    }
    if let Some(v) = d.context {
        row.context = v;
    }
    row.status = d.status.to_owned();
    if let Some(v) = d.blocker {
        row.blocker = v.to_owned();
    }
    if let Some(v) = d.reason {
        row.reason = v;
    }
    let _ = d.decoder;
    if row.status == "blocked" && row.reason.is_empty() {
        row.reason = format!("{} rank {rank}: {}", g.badge_id, row.blocker);
    }
    row
}

/// `parse_badges`: rows sorted by (collection, rank), plus the sorted error set.
pub fn parse_badges(data: &[u8], metric_names: &[String]) -> (Vec<Row>, Vec<String>) {
    let lay = layout(data);
    if !lay.errors.is_empty() {
        return (Vec::new(), lay.errors);
    }
    let known: HashSet<String> = lay.reward_ids.iter().cloned().collect();
    let mut rows = Vec::new();
    for g in &lay.groups {
        let mut cursor = g.ranks_start;
        for rank in 1..=u32::from(g.rank_count) {
            let Some(rank_start) = find_before(data, RANK_MARKER, cursor, g.end) else { break };
            let nxt = if rank < u32::from(g.rank_count) {
                find_before(data, RANK_MARKER, rank_start + 2, g.end)
            } else {
                None
            };
            let rank_end = nxt.unwrap_or(g.end);
            rows.push(decode_rank(data, g, rank, rank_start, rank_end, &known, metric_names));
            cursor = rank_end;
        }
    }
    rows.sort_by(|a, b| a.collection.cmp(&b.collection).then(a.rank.cmp(&b.rank)));
    let errors: BTreeSet<String> = rows
        .iter()
        .filter(|r| r.status == "blocked" && r.blocker != "descriptive" && !r.reason.is_empty())
        .map(|r| r.reason.clone())
        .collect();
    (rows, errors.into_iter().collect())
}

/// `models.requirement_rows`: rows with a collection, first per (collection, rank).
pub fn requirement_rows(rows: &[Row]) -> Vec<&Row> {
    let mut seen = HashSet::new();
    rows.iter().filter(|r| !r.collection.is_empty() && seen.insert((r.collection.clone(), r.rank))).collect()
}

/// The stored `amount`: `int(amount)` of a number, else NULL.
pub fn stored_amount(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f.trunc() as i64)),
        _ => None,
    }
}

pub fn context_map(v: &Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}
