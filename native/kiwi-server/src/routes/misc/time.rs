//! The time converter and clocks of `app/trove/misc.py`. Zones load from the
//! same files Python's `zoneinfo` reads (TZPATH, then the `tzdata` package),
//! so both sides agree on whatever tzdata the image ships.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

use kiwi_core::{
    pydt::{self, Wall},
    zoneinfo::{self, Zone},
};
use serde_json::{Value, json};

/// `TIMEZONES`: (id, display name), Trove first.
pub const TIMEZONES: [(&str, &str); 11] = [
    ("trove", "Trove Server (Reset)"),
    ("UTC", "UTC"),
    ("America/Sao_Paulo", "Brazil (Brasília)"),
    ("America/New_York", "US Eastern"),
    ("America/Los_Angeles", "US Pacific"),
    ("Europe/Lisbon", "Portugal / UK"),
    ("Europe/Paris", "Central Europe (FR, DE, ES)"),
    ("Europe/Moscow", "Russia (Moscow)"),
    ("Asia/Shanghai", "China (Beijing)"),
    ("Asia/Tokyo", "Japan & South Korea"),
    ("Australia/Sydney", "Australia (Sydney)"),
];

const DISCORD_STYLES: [(&str, &str); 7] = [
    ("t", "Short time (16:20)"),
    ("T", "Long time (16:20:30)"),
    ("d", "Short date (20/04/2021)"),
    ("D", "Long date (20 April 2021)"),
    ("f", "Short date/time (20 April 2021 16:20)"),
    ("F", "Long date/time (Tuesday, 20 April 2021 16:20)"),
    ("R", "Relative (in 2 months)"),
];

const TROVE_OFFSET: i64 = -11 * 3600;

/// Where `ZoneInfo(key)` finds its file.
pub struct ZoneSource {
    tzpath: Vec<PathBuf>,
    tzdata: Option<PathBuf>,
    cache: Mutex<HashMap<String, Arc<Zone>>>,
}

impl ZoneSource {
    pub fn new(tzpath: Vec<PathBuf>, tzdata: Option<PathBuf>) -> Self {
        ZoneSource { tzpath, tzdata, cache: Mutex::default() }
    }

    /// Python's defaults: `PYTHONTZPATH` or the compiled-in TZPATH, then the
    /// `tzdata` package of the interpreter in this image (`KIWI_TZDATA_DIR`
    /// overrides the search).
    pub fn from_env() -> Self {
        let tzpath = match std::env::var("PYTHONTZPATH") {
            Ok(v) => v.split(':').filter(|p| p.starts_with('/')).map(PathBuf::from).collect(),
            Err(_) if cfg!(unix) => {
                ["/usr/share/zoneinfo", "/usr/lib/zoneinfo", "/usr/share/lib/zoneinfo", "/etc/zoneinfo"]
                    .into_iter()
                    .map(PathBuf::from)
                    .collect()
            }
            Err(_) => Vec::new(),
        };
        let tzdata = std::env::var_os("KIWI_TZDATA_DIR").map(PathBuf::from).or_else(find_tzdata_package);
        if tzdata.is_none() && !tzpath.iter().any(|p| p.is_dir()) {
            tracing::error!("no tz database found (TZPATH or the tzdata package); zone lookups will fail");
        }
        ZoneSource::new(tzpath, tzdata)
    }

    /// `ZoneInfo(key)`; `None` where Python raises (unknown key, bad file).
    pub fn get(&self, key: &str) -> Option<Arc<Zone>> {
        if let Some(z) = self.cache.lock().unwrap_or_else(|e| e.into_inner()).get(key) {
            return Some(z.clone());
        }
        let bytes = self.read(key)?;
        let zone = Arc::new(zoneinfo::load(&bytes).ok()?);
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).insert(key.to_owned(), zone.clone());
        Some(zone)
    }

    fn read(&self, key: &str) -> Option<Vec<u8>> {
        if !zoneinfo::valid_key(key) {
            return None;
        }
        for dir in &self.tzpath {
            let path = dir.join(key);
            if path.is_file() {
                return std::fs::read(path).ok();
            }
        }
        // `importlib.resources.files("tzdata.zoneinfo.<dirs>").joinpath(<name>)`:
        // the directory part is a dotted package name.
        let mut path = self.tzdata.clone()?;
        let (dirs, name) = key.rsplit_once('/').unwrap_or(("", key));
        for package in dirs.split('/').filter(|d| !d.is_empty()) {
            for part in package.split('.') {
                if part.is_empty() {
                    return None;
                }
                path.push(part);
            }
        }
        if !path.is_dir() {
            return None;
        }
        path.push(name);
        if path.is_dir() {
            return None;
        }
        std::fs::read(path).ok()
    }
}

fn find_tzdata_package() -> Option<PathBuf> {
    let lib = Path::new("/usr/local/lib");
    let mut found: Vec<PathBuf> = std::fs::read_dir(lib)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("python3"))
        .map(|e| e.path().join("site-packages/tzdata/zoneinfo"))
        .filter(|p| p.is_dir())
        .collect();
    found.sort();
    found.pop()
}

pub fn source() -> &'static ZoneSource {
    static SOURCE: OnceLock<ZoneSource> = OnceLock::new();
    SOURCE.get_or_init(ZoneSource::from_env)
}

/// A `tzinfo`: a fixed `timezone` or a `ZoneInfo`.
#[derive(Clone)]
pub enum Tz {
    Fixed(i64),
    Zone(Arc<Zone>),
}

/// Why a conversion did not produce a body.
#[derive(Debug, PartialEq, Eq)]
pub enum TimeError {
    /// `MiscError`: a 400 with this message.
    Bad(String),
    /// Anything Python lets escape (out-of-range datetimes): a 500.
    Internal,
}

/// `_tzinfo`.
pub fn tzinfo(src: &ZoneSource, id: &str) -> Result<Tz, TimeError> {
    match id {
        "trove" => Ok(Tz::Fixed(TROVE_OFFSET)),
        "UTC" => Ok(Tz::Fixed(0)),
        _ => src.get(id).map(Tz::Zone).ok_or_else(|| TimeError::Bad(format!("Unknown timezone '{id}'."))),
    }
}

/// `datetime.fromtimestamp(unix, tz)` as glibc + `tz.fromutc` give it: the
/// local wall time and the `utcoffset()` its `isoformat()` prints.
pub fn fromtimestamp(unix: i64, tz: &Tz) -> Option<(Wall, i64)> {
    let utc = Wall::from_secs(unix);
    if !utc.in_range() {
        return None;
    }
    let (local, offset) = match tz {
        Tz::Fixed(off) => (Wall::from_secs(unix + off), *off),
        Tz::Zone(z) => {
            let (off, fold) = z.fromutc(unix, utc.year);
            let local = Wall::from_secs(unix + off);
            (local, z.utcoffset(unix + off, local.year, fold))
        }
    };
    local.in_range().then_some((local, offset))
}

/// `_render_zone`.
pub fn render_zone(src: &ZoneSource, unix: i64, id: &str, name: &str) -> Result<Value, TimeError> {
    let tz = tzinfo(src, id)?;
    let (local, offset) = fromtimestamp(unix, &tz).ok_or(TimeError::Internal)?;
    Ok(json!({
        "id": id,
        "name": name,
        "datetime": local.isoformat(offset),
        "date": local.strftime_date(),
        "time": local.strftime_time(),
    }))
}

pub fn all_zones(src: &ZoneSource, unix: i64) -> Result<Vec<Value>, TimeError> {
    TIMEZONES.iter().map(|(id, name)| render_zone(src, unix, id, name)).collect()
}

/// `timezones()`.
pub fn timezones() -> Value {
    let items: Vec<Value> = TIMEZONES.iter().map(|(id, name)| json!({"id": id, "name": name})).collect();
    json!({"items": items, "count": TIMEZONES.len()})
}

/// `time_now()` for a Unix time in whole seconds.
pub fn time_now(src: &ZoneSource, unix: i64) -> Result<Value, TimeError> {
    Ok(json!({"unix": unix, "zones": all_zones(src, unix)?}))
}

/// `int(dt.replace(tzinfo=tz).timestamp())` for a naive wall time.
pub fn naive_timestamp(wall_us: i128, tz: &Tz, year: i64) -> i64 {
    let offset = match tz {
        Tz::Fixed(off) => *off,
        Tz::Zone(z) => z.utcoffset(wall_us.div_euclid(1_000_000) as i64, year, false),
    };
    pydt::total_seconds_int(wall_us - i128::from(offset) * 1_000_000)
}

/// `convert_time(datetime_str, timezone_id, unix)`.
pub fn convert_time(
    src: &ZoneSource,
    datetime: Option<&str>,
    timezone: &str,
    unix: Option<i64>,
) -> Result<Value, TimeError> {
    let unix = match unix {
        Some(u) => u,
        None => {
            let text = datetime
                .filter(|s| !s.is_empty())
                .ok_or_else(|| TimeError::Bad("Provide either 'unix' or 'datetime'.".into()))?;
            let parsed =
                pydt::fromisoformat(text).map_err(|e| TimeError::Bad(format!("Invalid datetime '{text}': {e}")))?;
            match parsed.offset_us {
                Some(off) => pydt::total_seconds_int(parsed.wall_us() - i128::from(off)),
                None => naive_timestamp(parsed.wall_us(), &tzinfo(src, timezone)?, parsed.year),
            }
        }
    };
    let (utc, _) = fromtimestamp(unix, &Tz::Fixed(0)).ok_or(TimeError::Internal)?;
    let zones = all_zones(src, unix)?;
    let discord: Vec<Value> = DISCORD_STYLES
        .iter()
        .map(|(style, label)| json!({"style": style, "label": label, "code": format!("<t:{unix}:{style}>")}))
        .collect();
    Ok(json!({"unix": unix, "iso_utc": utc.isoformat(0), "zones": zones, "discord": discord}))
}
