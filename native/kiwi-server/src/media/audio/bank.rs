//! `app/trove/audio/bank.py`: a Wwise SoundBank's sections, its media index
//! and the HIRC objects that play them.

pub const BANK_VERSION: u32 = 128;
pub const HIRC_SOUND: u8 = 2;
pub const HIRC_ACTION: u8 = 3;
pub const HIRC_EVENT: u8 = 4;
pub const SOURCE_ID_AT: usize = 9;
pub const MEDIA_SIZE_AT: usize = 13;
const MAX_SECTION_BYTES: usize = 512 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Section {
    pub tag: String,
    pub offset: usize,
    pub size: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct Media {
    pub media_id: u32,
    pub offset: u32,
    pub size: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct HircObject {
    pub kind: u8,
    pub object_id: u32,
    pub offset: usize,
    pub size: usize,
    pub source_id: Option<u32>,
    pub media_size_at: Option<usize>,
}

pub struct Bank<'a> {
    pub raw: &'a [u8],
    pub version: u32,
    pub bank_id: u32,
    pub sections: Vec<Section>,
    pub media: Vec<Media>,
    pub objects: Vec<HircObject>,
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

impl Bank<'_> {
    pub fn data_offset(&self) -> Option<usize> {
        self.sections.iter().find(|s| s.tag == "DATA").map(|s| s.offset)
    }

    pub fn media_bytes(&self, m: &Media) -> Result<&[u8], String> {
        let base = self.data_offset().ok_or("bank has no DATA section")?;
        let start = (base + m.offset as usize).min(self.raw.len());
        let end = (base + m.offset as usize + m.size as usize).min(self.raw.len());
        Ok(&self.raw[start..end.max(start)])
    }

    /// `sound_for_media`: `{source_id: object}` in first-key order, last object winning.
    pub fn sound_for_media(&self) -> Vec<(u32, HircObject)> {
        let mut out: Vec<(u32, HircObject)> = Vec::new();
        for o in &self.objects {
            if o.kind != HIRC_SOUND {
                continue;
            }
            let Some(src) = o.source_id else { continue };
            match out.iter_mut().find(|(k, _)| *k == src) {
                Some(slot) => slot.1 = *o,
                None => out.push((src, *o)),
            }
        }
        out
    }
}

fn sections(raw: &[u8]) -> Result<Vec<Section>, String> {
    let mut out = Vec::new();
    let mut off = 0;
    while off + 8 <= raw.len() {
        let tag = &raw[off..off + 4];
        let size = u32_at(raw, off + 4) as usize;
        if size > MAX_SECTION_BYTES || off + 8 + size > raw.len() {
            let latin: String = tag.iter().map(|&c| c as char).collect();
            return Err(format!("section '{latin}' runs past the end of the file"));
        }
        if !tag.is_ascii() {
            return Err("section tag is not ASCII".into());
        }
        out.push(Section { tag: String::from_utf8_lossy(tag).into_owned(), offset: off + 8, size });
        off += 8 + size;
    }
    if out.first().is_none_or(|s| s.tag != "BKHD") {
        return Err("file does not start with a BKHD section".into());
    }
    Ok(out)
}

fn objects(raw: &[u8], s: &Section) -> Result<Vec<HircObject>, String> {
    if s.size < 4 {
        return Err("HIRC section is too small".into());
    }
    let count = u32_at(raw, s.offset);
    let mut out = Vec::new();
    let mut off = s.offset + 4;
    let end = s.offset + s.size;
    for _ in 0..count {
        if off + 5 > end {
            return Err("HIRC ends mid-object".into());
        }
        let kind = raw[off];
        let size = u32_at(raw, off + 1) as usize;
        let body = off + 5;
        if size < 4 || body + size > end {
            return Err("HIRC object runs past the section".into());
        }
        let mut o =
            HircObject { kind, object_id: u32_at(raw, body), offset: body, size, source_id: None, media_size_at: None };
        if kind == HIRC_SOUND && size >= MEDIA_SIZE_AT + 4 {
            o.source_id = Some(u32_at(raw, body + SOURCE_ID_AT));
            o.media_size_at = Some(body + MEDIA_SIZE_AT);
        }
        out.push(o);
        off = body + size;
    }
    Ok(out)
}

/// `bank.parse`; Err is the `BankError` text.
pub fn parse(raw: &[u8]) -> Result<Bank<'_>, String> {
    let sections = sections(raw)?;
    let header = &sections[0];
    if header.size < 8 {
        return Err("BKHD is too small".into());
    }
    let (version, bank_id) = (u32_at(raw, header.offset), u32_at(raw, header.offset + 4));
    if version != BANK_VERSION {
        return Err(format!("unsupported bank version {version} (expected {BANK_VERSION})"));
    }
    let (mut media, mut objs) = (Vec::new(), Vec::new());
    for s in &sections {
        if s.tag == "DIDX" {
            if s.size % 12 != 0 {
                return Err("DIDX size is not a whole number of entries".into());
            }
            media = (0..s.size / 12)
                .map(|i| {
                    let o = s.offset + i * 12;
                    Media { media_id: u32_at(raw, o), offset: u32_at(raw, o + 4), size: u32_at(raw, o + 8) }
                })
                .collect();
        } else if s.tag == "HIRC" {
            objs = objects(raw, s)?;
        }
    }
    if !media.is_empty() {
        let data = sections.iter().find(|s| s.tag == "DATA").ok_or("DIDX present but DATA is missing")?;
        for m in &media {
            if u64::from(m.offset) + u64::from(m.size) > data.size as u64 {
                return Err(format!("media {} runs past DATA", m.media_id));
            }
        }
    }
    Ok(Bank { raw, version, bank_id, sections, media, objects: objs })
}
