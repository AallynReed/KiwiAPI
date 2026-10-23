//! `app/trove/audio/edit.py`: a bank written back with media replaced and
//! new sounds appended (each a cloned Sound, a Play action and an Event).

use std::collections::{HashMap, HashSet};

use super::{bank, names};

const DATA_ALIGN: usize = 16;
const ACTION_PLAY: u16 = 0x0403;
const ACTION_FLAGS: u8 = 0x04;
pub const MAX_ADDITIONS: usize = 64;

pub struct Addition {
    pub event_name: String,
    pub media: Vec<u8>,
    pub clone_from: Option<super::pyval::PyInt>,
}

pub struct Added {
    pub event: String,
    pub event_id: u32,
    pub media_id: u32,
    pub object_id: u32,
}

pub struct Result {
    pub data: Vec<u8>,
    pub replaced: usize,
    pub added: Vec<Added>,
}

/// Why `rebuild` failed: `EditError` (a 400 with its text) or a `BankError`
/// Python lets through (a 500).
pub enum Fail {
    Edit(String),
    Crash(String),
}

fn free_id(taken: &mut HashSet<u32>, seed: u32) -> u32 {
    let mut v = seed;
    while taken.contains(&v) || v == 0 {
        v = (u64::from(v) * 2654435761 + 1) as u32;
    }
    taken.insert(v);
    v
}

fn object(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![kind];
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// `rebuild(raw, replacements, additions)`; `replacements` keeps the
/// insertion order of a Python dict (first position, last value).
pub fn rebuild(
    raw: &[u8],
    replacements: &[(super::pyval::PyInt, Vec<u8>)],
    additions: &[Addition],
) -> std::result::Result<Result, Fail> {
    if additions.len() > MAX_ADDITIONS {
        return Err(Fail::Edit(format!("at most {MAX_ADDITIONS} new sounds can be added at once")));
    }
    let parsed = bank::parse(raw).map_err(Fail::Crash)?;
    let known: HashSet<u32> = parsed.media.iter().map(|m| m.media_id).collect();
    let mut repl: HashMap<u32, &[u8]> = HashMap::new();
    for (id, data) in replacements {
        match id.to_u32().filter(|i| known.contains(i)) {
            Some(i) => {
                repl.insert(i, data);
            }
            None => return Err(Fail::Edit(format!("this bank has no sound {id}"))),
        }
    }
    if !additions.is_empty() && parsed.media.is_empty() {
        return Err(Fail::Edit("this bank holds no embedded media to extend".into()));
    }
    let mut taken: HashSet<u32> = parsed.objects.iter().map(|o| o.object_id).chain(known.iter().copied()).collect();
    let by_media = parsed.sound_for_media();
    let mut new_objects = Vec::new();
    let mut added = Vec::new();
    let mut extra: Vec<(u32, &[u8])> = Vec::new();
    for add in additions {
        let name = kiwi_core::pystr::strip(&add.event_name);
        if name.is_empty() {
            return Err(Fail::Edit("a new sound needs an event name".into()));
        }
        let event_id = names::event_id(name);
        if !taken.insert(event_id) {
            return Err(Fail::Edit(format!("this bank already has an event named {}", kiwi_core::pystr::repr(name))));
        }
        let prefer = add.clone_from.as_ref().and_then(|c| c.to_u32());
        let template = match prefer.and_then(|p| by_media.iter().find(|(k, _)| *k == p)) {
            Some((_, o)) => *o,
            None => {
                let Some(first) = by_media.first() else {
                    return Err(Fail::Edit("this bank has no sound objects to model a new one on".into()));
                };
                by_media.iter().fold(first.1, |best, (_, o)| if o.size < best.size { *o } else { best })
            }
        };
        let media_id = free_id(&mut taken, event_id ^ 0x5F5F_5F5F);
        let object_id = free_id(&mut taken, event_id ^ 0xA5A5_A5A5);
        let action_id = free_id(&mut taken, event_id ^ 0x3C3C_3C3C);

        let mut body = parsed.raw[template.offset..template.offset + template.size].to_vec();
        body[0..4].copy_from_slice(&object_id.to_le_bytes());
        body[bank::SOURCE_ID_AT..bank::SOURCE_ID_AT + 4].copy_from_slice(&media_id.to_le_bytes());
        body[bank::MEDIA_SIZE_AT..bank::MEDIA_SIZE_AT + 4].copy_from_slice(&(add.media.len() as u32).to_le_bytes());
        new_objects.extend(object(bank::HIRC_SOUND, &body));

        let mut action = Vec::with_capacity(18);
        action.extend_from_slice(&action_id.to_le_bytes());
        action.extend_from_slice(&ACTION_PLAY.to_le_bytes());
        action.extend_from_slice(&object_id.to_le_bytes());
        action.extend_from_slice(&[0, 0, 0, ACTION_FLAGS]);
        action.extend_from_slice(&parsed.bank_id.to_le_bytes());
        new_objects.extend(object(bank::HIRC_ACTION, &action));
        let mut event = event_id.to_le_bytes().to_vec();
        event.push(1);
        event.extend_from_slice(&action_id.to_le_bytes());
        new_objects.extend(object(bank::HIRC_EVENT, &event));

        extra.push((media_id, &add.media));
        added.push(Added { event: name.to_owned(), event_id, media_id, object_id });
    }
    let data = assemble(&parsed, &repl, &extra, &new_objects, (added.len() * 3) as u32)?;
    Ok(Result { data, replaced: replacements.len(), added })
}

fn assemble(
    parsed: &bank::Bank,
    repl: &HashMap<u32, &[u8]>,
    extra: &[(u32, &[u8])],
    new_objects: &[u8],
    new_count: u32,
) -> std::result::Result<Vec<u8>, Fail> {
    let mut blobs: Vec<(u32, &[u8])> = Vec::new();
    for m in &parsed.media {
        let blob = match repl.get(&m.media_id) {
            Some(r) if !r.is_empty() => *r,
            _ => parsed.media_bytes(m).map_err(Fail::Crash)?,
        };
        blobs.push((m.media_id, blob));
    }
    blobs.extend_from_slice(extra);
    let (mut didx, mut data) = (Vec::new(), Vec::new());
    let mut sizes: HashMap<u32, u32> = HashMap::new();
    for (id, blob) in &blobs {
        if data.len() % DATA_ALIGN != 0 {
            data.resize(data.len() + DATA_ALIGN - data.len() % DATA_ALIGN, 0);
        }
        didx.extend_from_slice(&id.to_le_bytes());
        didx.extend_from_slice(&(data.len() as u32).to_le_bytes());
        didx.extend_from_slice(&(blob.len() as u32).to_le_bytes());
        data.extend_from_slice(blob);
        sizes.insert(*id, blob.len() as u32);
    }
    let mut out = Vec::new();
    for s in &parsed.sections {
        let payload: Vec<u8> = match s.tag.as_str() {
            "DIDX" => didx.clone(),
            "DATA" => data.clone(),
            "HIRC" => patch_hirc(parsed, &parsed.raw[s.offset..s.offset + s.size], &sizes, new_objects, new_count)?,
            _ => parsed.raw[s.offset..s.offset + s.size].to_vec(),
        };
        out.extend_from_slice(s.tag.as_bytes());
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&payload);
    }
    Ok(out)
}

fn patch_hirc(
    parsed: &bank::Bank,
    payload: &[u8],
    sizes: &HashMap<u32, u32>,
    new_objects: &[u8],
    new_count: u32,
) -> std::result::Result<Vec<u8>, Fail> {
    let crash = || Fail::Crash("pack_into offset out of range".into());
    let mut body = payload.to_vec();
    let start = parsed.sections.iter().find(|s| s.tag == "HIRC").map_or(0, |s| s.offset);
    for o in &parsed.objects {
        if o.kind != bank::HIRC_SOUND {
            continue;
        }
        let (Some(at), Some(src)) = (o.media_size_at, o.source_id) else { continue };
        let Some(size) = sizes.get(&src) else { continue };
        let at = at.checked_sub(start).filter(|a| a + 4 <= body.len()).ok_or_else(crash)?;
        body[at..at + 4].copy_from_slice(&size.to_le_bytes());
    }
    if new_count != 0 {
        let count =
            u32::from_le_bytes([body[0], body[1], body[2], body[3]]).checked_add(new_count).ok_or_else(crash)?;
        body[0..4].copy_from_slice(&count.to_le_bytes());
        body.extend_from_slice(new_objects);
    }
    Ok(body)
}
