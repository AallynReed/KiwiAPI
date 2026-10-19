//! Byte scanners over Trove's `.binfab` wire format.

/// A uleb128 as the Python reader decodes it: up to 10 bytes, so up to 70 bits.
/// None where Python raises (runs off the end, or an 11th byte is needed).
#[inline]
fn read_uleb(data: &[u8], mut pos: usize) -> Option<(u128, usize)> {
    let mut result: u128 = 0;
    let mut shift = 0u32;
    loop {
        let byte = *data.get(pos)?;
        pos += 1;
        result |= u128::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Some((result, pos));
        }
        shift += 7;
        if shift > 63 {
            return None;
        }
    }
}

/// One length-prefixed ascii string: (offset, field number, text).
pub struct Harvested {
    pub offset: usize,
    pub field: u128,
    pub text: String,
}

/// Every `<key wt8><uleb len><printable ascii>` at every byte offset.
pub fn harvest_strings(data: &[u8], min_len: u128, max_len: u128) -> Vec<Harvested> {
    let mut out = Vec::new();
    let n = data.len();
    for i in 0..n.saturating_sub(1) {
        let Some((key, j)) = read_uleb(data, i) else { continue };
        if key & 0xF != 8 {
            continue;
        }
        let Some((length, k)) = read_uleb(data, j) else { continue };
        if length < min_len || length > max_len || k as u128 + length > n as u128 {
            continue;
        }
        let raw = &data[k..k + length as usize];
        if raw.iter().all(|&b| (32..127).contains(&b)) {
            // SAFETY: every byte is printable ascii.
            let text = unsafe { String::from_utf8_unchecked(raw.to_vec()) };
            out.push(Harvested { offset: i, field: key >> 4, text });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_field_string() {
        let data = [0x18, 3, b'a', b'b', b'c'];
        let got = harvest_strings(&data, 2, 512);
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].offset, got[0].field, got[0].text.as_str()), (0, 1, "abc"));
    }
}
