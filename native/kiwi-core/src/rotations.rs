//! Rotation and timer math from `app/trove/{server_time,rotations,calendar,luxion,delves}.py`.
//! Instants are microseconds since the Unix epoch (Python datetimes hold
//! microseconds); every result is in real-UTC unix seconds like Python's.
//! Python's float `//` and `%` on these values agree with integer floor math
//! on microseconds, which is what this uses.

#[cfg(test)]
mod tests;

pub const US: i64 = 1_000_000;
pub const MINUTE: i64 = 60 * US;
pub const HOUR: i64 = 3600 * US;
pub const DAY: i64 = 24 * HOUR;
pub const WEEK: i64 = 7 * DAY;

/// The trove-time frame is real UTC minus 11h (the in-game day rolls at 11:00 UTC).
pub const TROVE_OFFSET: i64 = 11 * HOUR;

/// Days from 1970-01-01 to a proleptic Gregorian date.
pub const fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// A UTC instant in microseconds.
pub const fn utc(y: i64, mo: i64, d: i64, h: i64) -> i64 {
    days_from_civil(y, mo, d) * DAY + h * HOUR
}

pub const FIRST_WEEK_BUFF: i64 = utc(2020, 3, 23, 0);
pub const FIRST_CORRUXION: i64 = utc(2024, 3, 8, 0);
pub const FIRST_FLUXION: i64 = utc(2023, 7, 18, 0);
pub const FIRST_GARDENING: i64 = utc(2025, 5, 23, 0);
pub const DRAGON_DURATION: i64 = 3 * DAY;
pub const DRAGON_INTERVAL: i64 = 14 * DAY;
pub const FLUXION_INTERVAL: i64 = WEEK;
pub const CHALLENGE_DURATION: i64 = 20 * MINUTE;

pub const D15_EPOCH: i64 = utc(2024, 6, 18, 11);
pub const D15_INTERVAL: i64 = 3 * HOUR;
pub const BIOME1: [&str; 12] = [
    "Sundered Uplands",
    "Cerise Sandsea",
    "Deep Forest",
    "Alkali Flats",
    "Dead of Winter",
    "Sundered Uplands",
    "Firefly Party",
    "Desert of Secrets",
    "Weathered Wastelands",
    "Frozen Wastes",
    "Frigga's Fjord",
    "Abandoned Boneyard",
];
pub const BIOME2: [&str; 11] = [
    "Cursed Vale",
    "Hollow Dunes",
    "Bewitching Wood",
    "Primal Preserve",
    "Hollow Dunes",
    "Ancient Heights",
    "Viking Burial Grounds",
    "Spellbound Thicket",
    "Saurian Swamp",
    "Restless Range",
    "Uncanny Valley",
];
pub const BIOME3: [&str; 14] = [
    "Sugar Steppes",
    "Volcanic Fields",
    "The Lost Isles",
    "Luminopolis",
    "The Lost Isles",
    "Blazing Emberlands",
    "Cocoa Craters",
    "Data Spires",
    "The Lost Isles",
    "Cupcake Canyon",
    "Dragon's Teeth",
    "Luminopolis",
    "The Lost Isles",
    "Data Spires",
];

pub const MANA_BASE: i64 = utc(2023, 11, 20, 11);
pub const MANA_BIOMES: [&str; 11] = [
    "Neon City",
    "Jurassic Jungle",
    "Dragonfire Peaks",
    "Forbidden Spires",
    "Sundered Uplands",
    "Medieval Highlands",
    "Permafrost",
    "Cursed Vale",
    "Desert Frontier",
    "Fae Forest",
    "Candoria",
];

pub const STAMPY_BASE: i64 = utc(2023, 9, 30, 11);
pub const STAMPY_PERIOD: i64 = WEEK;
pub const STAMPY_DURATION: i64 = 48 * HOUR;
pub const STAMPY_BIOMES: [&str; 13] = [
    "Desert Frontier",
    "The Lost Isles",
    "Geode Topside",
    "Neon City",
    "Dragonfire Peaks",
    "Permafrost",
    "Candoria",
    "Cursed Vale",
    "Forbidden Spires",
    "Fae Forest",
    "Medieval Highlands",
    "Jurassic Jungle",
    "Sundered Uplands",
];

/// Parent-biome icons when `biomes.json` has none (`rotations._FALLBACK`).
pub const BIOME_ICON_FALLBACK: [(&str, &str); 13] = [
    ("Neon City", "neon"),
    ("Jurassic Jungle", "dinosaur"),
    ("Dragonfire Peaks", "dragon"),
    ("Forbidden Spires", "spires"),
    ("Sundered Uplands", "giantland"),
    ("Medieval Highlands", "forest"),
    ("Permafrost", "tundra"),
    ("Cursed Vale", "undead"),
    ("Desert Frontier", "frontier"),
    ("Fae Forest", "fae"),
    ("Candoria", "candy"),
    ("Geode Topside", "dunes"),
    ("The Lost Isles", "pirate"),
];

pub const WEEKDAYS: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];

/// `int(dt.timestamp())`: truncates toward zero.
pub const fn unix(us: i64) -> i64 {
    us / US
}

/// Python `weekday()` (Monday = 0) of a UTC instant.
pub const fn weekday(us: i64) -> usize {
    (us.div_euclid(DAY) + 3).rem_euclid(7) as usize
}

fn index<T: Copy, const N: usize>(list: &[T; N], i: i64) -> T {
    list[i.rem_euclid(N as i64) as usize]
}

// --- server time -----------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerTime {
    pub now_unix: i64,
    /// `trove_now(now).weekday()`.
    pub trove_weekday: usize,
    pub daily_reset_at: i64,
    pub weekly_reset_at: i64,
}

/// Start of the trove-day (00:00 = 11:00 UTC) holding `now`.
fn next_daily_reset(now: i64) -> i64 {
    let today = now.div_euclid(DAY) * DAY + 11 * HOUR;
    if now < today { today } else { today + DAY }
}

fn next_weekly_reset(now: i64) -> i64 {
    let t = now - TROVE_OFFSET;
    let completed = (t - FIRST_WEEK_BUFF).div_euclid(WEEK);
    FIRST_WEEK_BUFF + (completed + 1) * WEEK + TROVE_OFFSET
}

pub fn server_time(now: i64) -> ServerTime {
    ServerTime {
        now_unix: unix(now),
        trove_weekday: weekday(now - TROVE_OFFSET),
        daily_reset_at: unix(next_daily_reset(now)),
        weekly_reset_at: unix(next_weekly_reset(now)),
    }
}

/// `str(trove_now(now).weekday())`, the `daily_buffs.json` key.
pub fn daily_buff_index(now: i64) -> usize {
    weekday(now - TROVE_OFFSET)
}

/// `_week_index(trove_now(now))`, the `weekly_buffs.json` key.
pub fn weekly_buff_index(now: i64) -> usize {
    (now - TROVE_OFFSET - FIRST_WEEK_BUFF).div_euclid(WEEK).rem_euclid(4) as usize
}

// --- challenge -------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChallengeWindow {
    pub starts_at: i64,
    pub ends_at: i64,
    pub active: bool,
    pub is_friday_window: bool,
    pub seconds_remaining: i64,
}

pub fn is_trove_friday(now: i64) -> bool {
    weekday(now - TROVE_OFFSET) == 4
}

/// `server_time.challenge_window`: the latest window start (hourly, or every
/// half hour on a trove Friday), active for 20 minutes.
pub fn challenge_window(now: i64) -> ChallengeWindow {
    let real = now.div_euclid(MINUTE) * MINUTE;
    let friday = is_trove_friday(real);
    let minute = (real / MINUTE).rem_euclid(60);
    let window_minute = if friday && minute >= 30 { 30 } else { 0 };
    let start = real - (minute - window_minute) * MINUTE;
    let end = start + CHALLENGE_DURATION;
    ChallengeWindow {
        starts_at: unix(start),
        ends_at: unix(end),
        active: start <= now && now < end,
        is_friday_window: friday,
        seconds_remaining: (unix(end) - unix(now)).max(0),
    }
}

/// `captures.classify_challenge`: the literals are a public contract.
pub fn classify_challenge(name: Option<&str>) -> Option<&'static str> {
    let n = py_strip(name?);
    if n.is_empty() {
        return None;
    }
    Some(match n {
        "Collection Challenge" => "collection",
        "RAMPAGE ALERT!" => "rampage",
        "Racing Challenge" => "racing",
        "Target Challenge" => "target",
        _ => "dungeon",
    })
}

/// `str.strip()`: Rust's whitespace plus U+001C..U+001F.
pub fn py_strip(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
}

// --- merchants -------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    pub starts_at: i64,
    pub ends_at: i64,
    /// Fluxion "voting"/"selling", Luxion "Day N"; Corruxion has none.
    pub state: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Merchant {
    pub active: bool,
    /// Fluxion only: "voting" | "selling" | "away".
    pub state: Option<&'static str>,
    pub starts_at: i64,
    pub ends_at: i64,
    pub seconds_remaining: i64,
    pub schedule: Vec<Window>,
}

fn real_unix(trove: i64) -> i64 {
    unix(trove + TROVE_OFFSET)
}

pub fn corruxion(now: i64, count: i64) -> Merchant {
    let t = now - TROVE_OFFSET;
    let secs = (t - FIRST_CORRUXION) / US;
    let interval = DRAGON_INTERVAL / US;
    let (completed, current) = (secs.div_euclid(interval), secs.rem_euclid(interval));
    let next_dragon = FIRST_CORRUXION + (completed + 1) * DRAGON_INTERVAL;
    let active = current < DRAGON_DURATION / US;
    let first_start = if active { next_dragon - DRAGON_INTERVAL } else { next_dragon };
    let end = first_start + DRAGON_DURATION;
    let remaining = if active { end - t } else { first_start - t };
    let schedule = (0..count)
        .map(|i| {
            let s = first_start + i * DRAGON_INTERVAL;
            Window { starts_at: real_unix(s), ends_at: real_unix(s + DRAGON_DURATION), state: None }
        })
        .collect();
    Merchant {
        active,
        state: None,
        starts_at: real_unix(first_start),
        ends_at: real_unix(end),
        seconds_remaining: (remaining / US).max(0),
        schedule,
    }
}

/// Each 7-day window flips phase, starting with voting at `FIRST_FLUXION`.
fn fluxion_state(window_start: i64) -> &'static str {
    let k = ((window_start - FIRST_FLUXION) as f64 / US as f64 / (FLUXION_INTERVAL / US) as f64).round_ties_even();
    if (k as i64).rem_euclid(2) == 0 { "voting" } else { "selling" }
}

pub fn fluxion(now: i64, count: i64) -> Merchant {
    let t = now - TROVE_OFFSET;
    let delta = t - FIRST_FLUXION;
    let (completed, rest) = (delta.div_euclid(DRAGON_INTERVAL), delta.rem_euclid(DRAGON_INTERVAL));
    let (phase, current) = (rest.div_euclid(FLUXION_INTERVAL), rest.rem_euclid(FLUXION_INTERVAL));
    let next_phase = FIRST_FLUXION + (completed * 2 + phase + 1) * FLUXION_INTERVAL;
    let active = current < DRAGON_DURATION;
    let first_start = if active { next_phase - FLUXION_INTERVAL } else { next_phase };
    let end = first_start + DRAGON_DURATION;
    let remaining = if active { end - t } else { first_start - t };
    let schedule = (0..count)
        .map(|i| {
            let s = first_start + i * FLUXION_INTERVAL;
            Window {
                starts_at: real_unix(s),
                ends_at: real_unix(s + DRAGON_DURATION),
                state: Some(fluxion_state(s).to_owned()),
            }
        })
        .collect();
    Merchant {
        active,
        state: Some(if active { fluxion_state(first_start) } else { "away" }),
        starts_at: real_unix(first_start),
        ends_at: real_unix(end),
        seconds_remaining: (remaining / US).max(0),
        schedule,
    }
}

/// `server_time.chaos_chest_window`: 7-day windows from the fluxion epoch in
/// real UTC (Tue 11:00).
pub fn chaos_chest_window(now: i64) -> (i64, i64) {
    let base = FIRST_FLUXION + TROVE_OFFSET;
    let start = base + (now - base).div_euclid(WEEK) * WEEK;
    (unix(start), unix(start + WEEK))
}

// --- gardening -------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Harvest {
    pub name: &'static str,
    pub active: bool,
    pub starts_at: i64,
    pub ends_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gardening {
    pub two_day: Harvest,
    pub three_day: Harvest,
    pub upcoming: Vec<Harvest>,
}

const TWO_DAY: &str = "2-day plants";
const THREE_DAY: &str = "3-day plants";

pub fn gardening(now: i64) -> Gardening {
    let base = FIRST_GARDENING + TROVE_OFFSET;
    let cycle_start = |days: i64| base + (now - base).div_euclid(days * DAY) * days * DAY;
    let win =
        |name, s: i64| Harvest { name, active: s <= now && now < s + DAY, starts_at: unix(s), ends_at: unix(s + DAY) };
    let (c2, c3) = (cycle_start(2), cycle_start(3));
    let mut upcoming = Vec::new();
    for i in 0..8 {
        let s2 = c2 + (i * 2 + 1) * DAY;
        if s2 > now {
            upcoming.push(win(TWO_DAY, s2));
        }
        let s3 = c3 + (i * 3 + 2) * DAY;
        if s3 > now {
            upcoming.push(win(THREE_DAY, s3));
        }
    }
    upcoming.sort_by_key(|h| h.starts_at);
    upcoming.truncate(10);
    Gardening { two_day: win(TWO_DAY, c2 + DAY), three_day: win(THREE_DAY, c3 + 2 * DAY), upcoming }
}

// --- biome rotations -------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BiomeSlot {
    pub starts_at: i64,
    pub ends_at: i64,
    /// d15 entries are sub-biome keys; wild mana / stampy entries parent biomes.
    pub biomes: Vec<&'static str>,
}

/// `rotations.biome_rotation`: the current 3-hour slot plus `count` more.
pub fn biome_rotation(now: i64, count: i64) -> Vec<BiomeSlot> {
    let consumed = (now - D15_EPOCH).div_euclid(D15_INTERVAL);
    let start = D15_EPOCH + consumed * D15_INTERVAL;
    (0..=count)
        .map(|i| {
            let offset = consumed + i;
            let s = start + i * D15_INTERVAL;
            BiomeSlot {
                starts_at: unix(s),
                ends_at: unix(s + D15_INTERVAL),
                biomes: vec![index(&BIOME1, offset), index(&BIOME2, offset), index(&BIOME3, offset)],
            }
        })
        .collect()
}

pub fn wild_mana(now: i64, count: i64) -> Vec<BiomeSlot> {
    let weeks = (now - MANA_BASE).div_euclid(WEEK);
    (0..count)
        .map(|i| {
            let w = weeks + i;
            let s = MANA_BASE + w * WEEK;
            BiomeSlot { starts_at: unix(s), ends_at: unix(s + WEEK), biomes: mana_biomes(w) }
        })
        .collect()
}

fn mana_biomes(w: i64) -> Vec<&'static str> {
    (0..3).map(|i| index(&MANA_BIOMES, w - i)).collect()
}

/// `rotations.stampy`: the open or next 48-hour windows, at most `count`.
pub fn stampy(now: i64, count: usize) -> Vec<BiomeSlot> {
    let weeks = (now - STAMPY_BASE).div_euclid(STAMPY_PERIOD);
    let mut out = Vec::new();
    for w in weeks - 1..weeks + 10 {
        let s = STAMPY_BASE + w * STAMPY_PERIOD;
        let e = s + STAMPY_DURATION;
        if e > now {
            out.push(BiomeSlot { starts_at: unix(s), ends_at: unix(e), biomes: vec![index(&STAMPY_BIOMES, w)] });
            if out.len() == count {
                break;
            }
        }
    }
    out
}

// --- Luxion ----------------------------------------------------------------

pub const LUXION_RUN_DAYS: i64 = 7;
const LUXION_WINDOW: i64 = 3 * 3600;
const LUXION_CYCLE: i64 = 27 * 3600;
/// `luxion.CYCLE_EPOCH` in unix seconds (trove 2025-10-10 00:00).
pub const LUXION_EPOCH: i64 = utc(2025, 10, 10, 11) / US;

/// The first 27h-grid opening at or after `after` (unix seconds).
pub fn next_grid_slot(after: i64) -> i64 {
    let cycles = -(LUXION_EPOCH - after).div_euclid(LUXION_CYCLE);
    LUXION_EPOCH + cycles * LUXION_CYCLE
}

/// `(run start, run end)`: the first sighting (else the day's reset) to
/// `ended` (else the reset `LUXION_RUN_DAYS` later).
pub fn run_bounds(day_anchor: i64, first_seen: Option<i64>, ended: Option<i64>) -> (i64, i64) {
    (first_seen.unwrap_or(day_anchor), ended.unwrap_or(day_anchor + LUXION_RUN_DAYS * 86_400))
}

pub fn schedule_for(day_anchor: i64, first_seen: Option<i64>, ended: Option<i64>) -> Vec<Window> {
    let (start, end) = run_bounds(day_anchor, first_seen, ended);
    let mut out = Vec::new();
    let mut opens = next_grid_slot(start);
    while opens < end {
        out.push(Window {
            starts_at: opens,
            ends_at: opens + LUXION_WINDOW,
            state: Some(format!("Day {}", out.len() + 1)),
        });
        opens += LUXION_CYCLE;
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Luxion {
    pub active: bool,
    pub merchant_open: bool,
    pub starts_at: i64,
    pub ends_at: i64,
    pub seconds_remaining: i64,
    pub current_window: Option<Window>,
    pub next_window: Option<Window>,
    pub schedule: Vec<Window>,
}

/// `luxion.build_response` without the stored datetimes it echoes back.
pub fn luxion(day_anchor: i64, first_seen: Option<i64>, ended: Option<i64>, now: i64) -> Luxion {
    let now_ts = unix(now);
    let (starts_at, ends_at) = run_bounds(day_anchor, first_seen, ended);
    let schedule = schedule_for(day_anchor, first_seen, ended);
    let current = schedule.iter().find(|w| w.starts_at <= now_ts && now_ts < w.ends_at).cloned();
    let next = schedule.iter().find(|w| w.starts_at > now_ts).cloned();
    let seconds_remaining = match (&current, &next) {
        (Some(c), _) => c.ends_at - now_ts,
        (None, Some(n)) => n.starts_at - now_ts,
        (None, None) => (ends_at - now_ts).max(0),
    };
    Luxion {
        active: starts_at <= now_ts && now_ts < ends_at,
        merchant_open: current.is_some(),
        starts_at,
        ends_at,
        seconds_remaining: seconds_remaining.max(0),
        current_window: current,
        next_window: next,
        schedule,
    }
}

// --- yearly calendar -------------------------------------------------------

pub const CALENDAR_WINDOW_DAYS: i64 = 365;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalendarEvent {
    pub kind: &'static str,
    pub name: String,
    pub starts_at: i64,
    pub ends_at: i64,
    pub color: Option<String>,
    pub state: Option<&'static str>,
    /// Parent biome names (stampy / mana).
    pub biomes: Option<Vec<&'static str>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Calendar {
    pub starts_at: i64,
    pub ends_at: i64,
    pub generated_at: i64,
    pub events: Vec<CalendarEvent>,
}

/// A captured Luxion run as `captures.list_luxion_runs` hands it over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LuxionRun {
    pub day_anchor: i64,
    pub first_seen: Option<i64>,
    pub ended: Option<i64>,
}

/// `(k, start)` for every `interval`-spaced occurrence of `base` from one
/// interval before `ws` up to `we`.
fn occurrences(base: i64, interval: i64, ws: i64, we: i64) -> impl Iterator<Item = (i64, i64)> {
    let k0 = (ws - base).div_euclid(interval) - 1;
    (k0..).map(move |k| (k, base + k * interval)).take_while(move |&(_, s)| s < we)
}

/// `(name, color)` of weekly-buff rotation index `k`.
pub type WeeklyLookup<'a> = &'a dyn Fn(i64) -> (String, Option<String>);

/// `calendar.yearly_calendar`. `weekly_buffs` is `len(weekly_buffs.json)` and
/// the `(name, color)` of rotation index `k % n` (`None` when the file is empty).
pub fn yearly_calendar(now: i64, weekly_buffs: Option<(i64, WeeklyLookup<'_>)>, luxion_runs: &[LuxionRun]) -> Calendar {
    let (ws, we) = (now - CALENDAR_WINDOW_DAYS * DAY, now + CALENDAR_WINDOW_DAYS * DAY);
    let overlaps = |s: i64, e: i64| e > ws && s < we;
    let ev = |kind, name: &str, s: i64, e: i64| CalendarEvent {
        kind,
        name: name.to_owned(),
        starts_at: unix(s),
        ends_at: unix(e),
        color: None,
        state: None,
        biomes: None,
    };
    let mut events = Vec::new();

    if let Some((n, lookup)) = weekly_buffs.filter(|(n, _)| *n > 0) {
        for (k, s) in occurrences(FIRST_WEEK_BUFF + TROVE_OFFSET, WEEK, ws, we) {
            if overlaps(s, s + WEEK) {
                let (name, color) = lookup(k.rem_euclid(n));
                events.push(CalendarEvent { color, ..ev("weekly_buff", &name, s, s + WEEK) });
            }
        }
    }
    for (_, s) in occurrences(FIRST_CORRUXION + TROVE_OFFSET, DRAGON_INTERVAL, ws, we) {
        if overlaps(s, s + DRAGON_DURATION) {
            events.push(ev("corruxion", "Corruxion", s, s + DRAGON_DURATION));
        }
    }
    for (_, s) in occurrences(FIRST_FLUXION + TROVE_OFFSET, DRAGON_INTERVAL, ws, we) {
        let phases = [
            (s, "Fluxion (Voting)", "voting", "5ca8cc"),
            (s + FLUXION_INTERVAL, "Fluxion (Selling)", "selling", "02679e"),
        ];
        for (start, name, state, color) in phases {
            if overlaps(start, start + DRAGON_DURATION) {
                events.push(CalendarEvent {
                    state: Some(state),
                    color: Some(color.to_owned()),
                    ..ev("fluxion", name, start, start + DRAGON_DURATION)
                });
            }
        }
    }
    for run in luxion_runs {
        for w in schedule_for(run.day_anchor, run.first_seen, run.ended) {
            let (s, e) = (w.starts_at * US, w.ends_at * US);
            if overlaps(s, e) {
                events.push(ev("luxion", "Luxion", s, e));
            }
        }
    }
    let gardens = [(2, 1, "gardening_2", TWO_DAY, "8bc34a"), (3, 2, "gardening_3", THREE_DAY, "4caf50")];
    for (cycle, ripe, kind, name, color) in gardens {
        for (_, s) in occurrences(FIRST_GARDENING + TROVE_OFFSET, cycle * DAY, ws, we) {
            let (hs, he) = (s + ripe * DAY, s + (ripe + 1) * DAY);
            if overlaps(hs, he) {
                events.push(CalendarEvent { color: Some(color.to_owned()), ..ev(kind, name, hs, he) });
            }
        }
    }
    for (w, s) in occurrences(STAMPY_BASE, STAMPY_PERIOD, ws, we) {
        let e = s + STAMPY_DURATION;
        if overlaps(s, e) {
            events.push(CalendarEvent { biomes: Some(vec![index(&STAMPY_BIOMES, w)]), ..ev("stampy", "Stampy", s, e) });
        }
    }
    for (w, s) in occurrences(MANA_BASE, WEEK, ws, we) {
        if overlaps(s, s + WEEK) {
            events.push(CalendarEvent { biomes: Some(mana_biomes(w)), ..ev("mana", "Wild Mana", s, s + WEEK) });
        }
    }
    events.sort_by(|a, b| (a.starts_at, a.kind).cmp(&(b.starts_at, b.kind)));
    Calendar { starts_at: unix(ws), ends_at: unix(we), generated_at: unix(now), events }
}

// --- delves ----------------------------------------------------------------

/// Week 1 began Monday 2025-11-03 00:00 UTC-11 (11:00 UTC).
pub const DELVE_WEEK_ONE: i64 = utc(2025, 11, 3, 11);

/// `delves.current_week_id`.
pub fn delve_week_id(now: i64) -> i64 {
    (now - DELVE_WEEK_ONE).div_euclid(DAY).div_euclid(7) + 1
}

/// `delves._seconds_until_next_pull` in microseconds: hourly on the delve
/// Monday (UTC-11), else the next 11:00 UTC reset; at least one second.
pub fn delve_next_pull(now: i64) -> i64 {
    let mut reset = now.div_euclid(DAY) * DAY + 11 * HOUR;
    if reset <= now {
        reset += DAY;
    }
    let target = if weekday(now - TROVE_OFFSET) == 0 { (now.div_euclid(HOUR) * HOUR + HOUR).min(reset) } else { reset };
    (target - now).max(US)
}
