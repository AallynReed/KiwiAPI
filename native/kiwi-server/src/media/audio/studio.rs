//! `app/trove/audio/studio.py`: the browser's change list applied to a bank,
//! then packaged as the `.bnk` itself or a `.tmod` that ships it.

use std::collections::HashMap;

use axum::http::StatusCode;

use super::{
    edit, encode,
    pyval::{self, IntErr, PyInt, PyVal},
};
use crate::error::ApiError;

pub const MAX_EDITS: usize = 200;
pub const MAX_CLIP_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_TOTAL_CLIP_BYTES: usize = 28 * 1024 * 1024;

fn fail(msg: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, "bad_request", msg)
}

/// Inside the per-change `try`: an APIError passes through, the caught
/// exceptions become the generic message, an `OverflowError` is a 500.
enum ChangeErr {
    Api(ApiError),
    Missing,
    Encode(String),
    Crash,
}

impl From<ApiError> for ChangeErr {
    fn from(e: ApiError) -> Self {
        ChangeErr::Api(e)
    }
}

fn int_of(v: &PyVal) -> Result<PyInt, ChangeErr> {
    pyval::int(v).map_err(|e| match e {
        IntErr::Invalid => ChangeErr::Missing,
        IntErr::Overflow => ChangeErr::Crash,
    })
}

/// `int(item[key])`: a missing key is the `KeyError`.
fn int_key(item: &PyVal, key: &str) -> Result<PyInt, ChangeErr> {
    int_of(item.get(key).ok_or(ChangeErr::Missing)?)
}

/// `_safe_title`'s regexes: runs outside `[A-Za-z0-9 _.\-()&'!,]` become a
/// space, whitespace runs one space, stripped, 80 code points, stripped.
pub fn safe_title(title: &str) -> String {
    let allowed = |c: char| c.is_ascii_alphanumeric() || " _.-()&'!,".contains(c);
    let mut a = String::new();
    let mut in_bad = false;
    for c in title.chars() {
        if allowed(c) {
            a.push(c);
            in_bad = false;
        } else if !in_bad {
            a.push(' ');
            in_bad = true;
        }
    }
    let mut b = String::new();
    let mut in_ws = false;
    for c in a.chars() {
        if kiwi_core::pystr::is_space(c) {
            if !in_ws {
                b.push(' ');
            }
            in_ws = true;
        } else {
            b.push(c);
            in_ws = false;
        }
    }
    let t = kiwi_core::pystr::strip(&b);
    let t = kiwi_core::pystr::strip(kiwi_core::pystr::prefix(t, 80));
    if t.is_empty() { "Trove Sound Pack".into() } else { t.into() }
}

fn clip<'a>(clips: &'a HashMap<String, Vec<u8>>, spec: &PyVal) -> Result<(&'a [u8], i128, i128), ChangeErr> {
    let key = pyval::str_or(spec.get("clip"), "");
    let samples = clips.get(&key).ok_or_else(|| fail("A replacement is missing its audio."))?;
    let channels = match spec.get("channels").filter(|v| v.truthy()) {
        Some(v) => int_of(v)?,
        None => PyInt::from_i64(1),
    };
    let rate = match spec.get("rate").filter(|v| v.truthy()) {
        Some(v) => int_of(v)?,
        None => PyInt::from_i64(0),
    };
    let channels = channels
        .to_i128()
        .filter(|c| *c == 1 || *c == 2)
        .ok_or_else(|| fail("Replacement audio must be mono or stereo."))?;
    let rate = rate
        .to_i128()
        .filter(|r| (1000..=96000).contains(r))
        .ok_or_else(|| fail("Replacement audio has an unusable sample rate."))?;
    if (samples.len() as i128) < channels * 2 {
        return Err(fail("A replacement clip is empty.").into());
    }
    Ok((samples, channels, rate))
}

fn media(clips: &HashMap<String, Vec<u8>>, spec: &PyVal, codec: &str) -> Result<Vec<u8>, ChangeErr> {
    if !spec.get("wem").is_some_and(PyVal::truthy) {
        let (samples, channels, rate) = clip(clips, spec)?;
        return encode::encode(samples, channels as u16, rate as u32, codec).map_err(ChangeErr::Encode);
    }
    let data =
        clips.get(&pyval::str_or(spec.get("clip"), "")).ok_or_else(|| fail("A replacement is missing its audio."))?;
    if let Err(e) = kiwi_core::audio::parse(data) {
        return Err(fail(format!("That .wem is not one the game can load: {e}")).into());
    }
    Ok(data.clone())
}

/// `^[A-Za-z0-9_]{3,96}$`.
fn event_name_ok(name: &str) -> bool {
    let core = name.strip_suffix('\n').unwrap_or(name);
    (3..=96).contains(&core.len()) && core.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// `apply_edits(raw, spec, clips)`.
pub fn apply_edits(raw: &[u8], spec: &PyVal, clips: &HashMap<String, Vec<u8>>) -> Result<edit::Result, ApiError> {
    let edits = match spec.get("edits") {
        Some(PyVal::List(l)) if !l.is_empty() => l,
        _ => return Err(fail("No changes were requested.")),
    };
    if edits.len() > MAX_EDITS {
        return Err(fail(format!("At most {MAX_EDITS} changes can be applied at once.")));
    }
    if clips.values().map(Vec::len).sum::<usize>() > MAX_TOTAL_CLIP_BYTES {
        return Err(fail("That is more replacement audio than one build can carry."));
    }
    if clips.values().any(|v| v.len() > MAX_CLIP_BYTES) {
        return Err(fail("One of the replacement clips is too long."));
    }
    let codec = pyval::str_or(spec.get("codec"), "adpcm");
    if codec != "adpcm" && codec != "pcm" {
        return Err(fail("Replacement audio can only be written as ADPCM or PCM."));
    }

    let mut replacements: Vec<(PyInt, Vec<u8>)> = Vec::new();
    let mut put = |id: PyInt, data: Vec<u8>| match replacements.iter_mut().find(|(k, _)| *k == id) {
        Some(slot) => slot.1 = data,
        None => replacements.push((id, data)),
    };
    let mut additions = Vec::new();
    for item in edits {
        if !matches!(item, PyVal::Dict(_)) {
            return Err(fail("A change was not understood."));
        }
        let kind = pyval::str_or(item.get("kind"), "");
        let step = (|| -> Result<(), ChangeErr> {
            match kind.as_str() {
                "mute" => {
                    let data = encode::silence();
                    put(int_key(item, "id")?, data);
                }
                "replace" => {
                    let data = media(clips, item, &codec)?;
                    put(int_key(item, "id")?, data);
                }
                "add" => {
                    let name = pyval::str_or(item.get("event"), "");
                    let name = kiwi_core::pystr::strip(&name).to_owned();
                    if !event_name_ok(&name) {
                        return Err(fail("An event name must be 3-96 letters, digits or underscores.").into());
                    }
                    let clone = item.get("clone_from").filter(|v| !matches!(v, PyVal::None));
                    let data = media(clips, item, &codec)?;
                    let clone_from = clone.map(int_of).transpose()?;
                    additions.push(edit::Addition { event_name: name, media: data, clone_from });
                }
                other => {
                    return Err(fail(format!("Unknown change type {}.", kiwi_core::pystr::repr(other))).into());
                }
            }
            Ok(())
        })();
        match step {
            Ok(()) => {}
            Err(ChangeErr::Api(e)) => return Err(e),
            Err(ChangeErr::Missing) => return Err(fail("A change was missing something it needed.")),
            Err(ChangeErr::Encode(e)) => return Err(fail(format!("That audio could not be converted: {e}"))),
            Err(ChangeErr::Crash) => return Err(ApiError::internal()),
        }
    }
    match edit::rebuild(raw, &replacements, &additions) {
        Ok(r) => Ok(r),
        Err(edit::Fail::Edit(e)) => Err(fail(e)),
        Err(edit::Fail::Crash(e)) => {
            tracing::error!("sound studio: {e}");
            Err(ApiError::internal())
        }
    }
}

/// `package(result, bank_path, spec)`: (bytes, filename, media type).
pub fn package(result: edit::Result, bank_path: &str, spec: &PyVal) -> Result<(Vec<u8>, String), ApiError> {
    let stem = bank_path.rsplit('/').next().unwrap_or(bank_path).to_owned();
    if pyval::str_or(spec.get("output"), "bnk") != "tmod" {
        return Ok((result.data, stem));
    }
    let empty = PyVal::Dict(Vec::new());
    let m = match spec.get("mod") {
        Some(v) if v.truthy() => v,
        _ => &empty,
    };
    if !matches!(m, PyVal::Dict(_)) {
        return Err(ApiError::internal());
    }
    let title = safe_title(&pyval::str_or(m.get("title"), ""));
    let author = kiwi_core::pystr::prefix(&pyval::str_or(m.get("author"), ""), 64).to_owned();
    let notes = kiwi_core::pystr::prefix(&pyval::str_or(m.get("notes"), ""), 500).to_owned();
    let path = bank_path.replace('\\', "/");
    let path = path.trim_start_matches('/');
    let path = if path.to_lowercase().ends_with(".cfg") { path.to_owned() } else { path.to_lowercase() };
    if path.len() > 255 {
        return Err(fail(format!(
            "The mod could not be packed: internal path too long for the .tmod format (>255 bytes): {path}"
        )));
    }
    let props = [
        ("title", title.as_str()),
        ("author", author.as_str()),
        ("notes", notes.as_str()),
        ("type", "Audio"),
        ("modLoader", "KiwiAPI"),
    ];
    let blob = kiwi_core::tmod::pack(1, &props, &[(path.as_str(), &result.data)]).ok_or_else(ApiError::internal)?;
    Ok((blob, format!("{title}.tmod")))
}
