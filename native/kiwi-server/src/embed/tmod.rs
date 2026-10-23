//! `read_tmod`'s verdict on an upload, with Python's `TmodError` text: the
//! header read through kiwi-core, and on failure the strict re-read whose
//! exception `read_tmod` reports.

/// `bytes.decode("utf-8")`'s `UnicodeDecodeError` text, if any.
pub fn utf8_error(b: &[u8]) -> Option<String> {
    let e = std::str::from_utf8(b).err()?;
    let start = e.valid_up_to();
    let (end, reason) = match e.error_len() {
        Some(n) => {
            let lead = b[start];
            let bad_start = n == 1 && !matches!(lead, 0xC2..=0xF4);
            (start + n, if bad_start { "invalid start byte" } else { "invalid continuation byte" })
        }
        None => (b.len(), "unexpected end of data"),
    };
    Some(if end - start == 1 {
        format!("'utf-8' codec can't decode byte 0x{:02x} in position {start}: {reason}", b[start])
    } else {
        format!("'utf-8' codec can't decode bytes in position {start}-{}: {reason}", end - 1)
    })
}

const INDEX: &str = "index out of range";

fn leb128(d: &[u8], mut pos: usize) -> Result<(u64, usize), String> {
    let (mut result, mut shift) = (0u128, 0u32);
    loop {
        let byte = *d.get(pos).ok_or(INDEX)?;
        pos += 1;
        result |= ((byte & 0x7F) as u128) << shift;
        if byte & 0x80 == 0 {
            return Ok(((result & 0xFFFF_FFFF) as u64, pos));
        }
        shift += 7;
        if shift >= 64 {
            return Err("varint too long".into());
        }
    }
}

fn string(d: &[u8], pos: usize, declared: u64) -> Result<usize, String> {
    let end = pos.saturating_add(declared as usize);
    let slice = d.get(pos.min(d.len())..end.min(d.len())).unwrap_or(&[]);
    if let Some(e) = utf8_error(slice) {
        return Err(e);
    }
    Ok(end)
}

/// `_parse_header(..., char_lengths=False)`: where it stops, or its exception.
fn strict(d: &[u8], header_size: usize, prop_count: u16) -> Result<usize, String> {
    let mut pos = 12;
    for _ in 0..prop_count {
        let (n, p) = leb128(d, pos)?;
        pos = string(d, p, n)?;
        let (n, p) = leb128(d, pos)?;
        pos = string(d, p, n)?;
    }
    while pos < header_size {
        let len = *d.get(pos).ok_or(INDEX)?;
        pos = string(d, pos + 1, len as u64)?;
        for _ in 0..4 {
            pos = leb128(d, pos)?.1;
        }
    }
    Ok(pos)
}

/// `read_tmod(data)`: `Err` is `str(TmodError)`.
pub fn validate(d: &[u8]) -> Result<(), String> {
    if d.len() < 12 {
        return Err("file too small to be a .tmod".into());
    }
    let header_size = u64::from_le_bytes(d[0..8].try_into().unwrap_or_default());
    let prop_count = u16::from_le_bytes([d[10], d[11]]);
    if !(12..=d.len() as u64).contains(&header_size) {
        return Err("declared header size is outside the file".into());
    }
    let hs = header_size as usize;
    if kiwi_core::tmod::header(d, hs, prop_count).is_some() {
        return Ok(());
    }
    match strict(d, hs, prop_count) {
        Err(e) => Err(format!("malformed .tmod file: {e}")),
        Ok(_) => Err("the file table does not end where the declared header size says".into()),
    }
}
