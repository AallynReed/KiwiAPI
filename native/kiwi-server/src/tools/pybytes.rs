//! CPython 3.13 byte codecs with their exact error text: `bytes.decode("utf-8")`
//! and `base64.b64decode(s, validate=True)` (`binascii.a2b_base64` strict mode).

fn is_cont(b: u8) -> bool {
    (0x80..0xC0).contains(&b)
}

/// `str(UnicodeDecodeError)` for the first bad sequence, `None` if `b` is UTF-8.
pub fn utf8_error(b: &[u8]) -> Option<String> {
    let Err(_) = std::str::from_utf8(b) else { return None };
    let mut s = 0;
    let n = b.len();
    // CPython's `ucs1lib_utf8_decode` outcome codes: 0 truncated, 1 bad start,
    // 2..4 bad continuation at offset code-1.
    let code = loop {
        if s >= n {
            return None;
        }
        let ch = b[s];
        let left = n - s;
        let need = match ch {
            0x00..=0x7F => {
                s += 1;
                continue;
            }
            0xC2..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF4 => 4,
            _ => break 1,
        };
        let c2 = b.get(s + 1).copied();
        let bad2 = |c: u8| {
            !is_cont(c)
                || match ch {
                    0xE0 => c < 0xA0,
                    0xED => c >= 0xA0,
                    0xF0 => c < 0x90,
                    0xF4 => c >= 0x90,
                    _ => false,
                }
        };
        if left < need {
            match c2 {
                None => break 0,
                Some(c) if bad2(c) => break 2,
                Some(_) => {}
            }
            if need == 4 && left >= 3 && !is_cont(b[s + 2]) {
                break 3;
            }
            break 0;
        }
        if bad2(b[s + 1]) {
            break 2;
        }
        if need >= 3 && !is_cont(b[s + 2]) {
            break 3;
        }
        if need == 4 && !is_cont(b[s + 3]) {
            break 4;
        }
        s += need;
    };
    let (end, reason) = match code {
        0 => (n, "unexpected end of data"),
        1 => (s + 1, "invalid start byte"),
        c => (s + c - 1, "invalid continuation byte"),
    };
    Some(if end - s == 1 {
        format!("'utf-8' codec can't decode byte 0x{:02x} in position {s}: {reason}", b[s])
    } else {
        format!("'utf-8' codec can't decode bytes in position {s}-{}: {reason}", end - 1)
    })
}

fn b64_value(c: u8) -> Option<u8> {
    Some(match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => return None,
    })
}

/// `base64.b64decode(s, validate=True)` of a `str`; `Err` is `str(exc)`.
pub fn b64decode_strict(s: &str) -> Result<Vec<u8>, String> {
    if !s.is_ascii() {
        return Err("string argument should contain only ASCII characters".into());
    }
    let data = s.as_bytes();
    let mut out = Vec::with_capacity(data.len().div_ceil(4) * 3);
    let (mut quad_pos, mut leftchar, mut pads) = (0usize, 0u8, 0usize);
    for (i, &c) in data.iter().enumerate() {
        if c == b'=' {
            pads += 1;
            if quad_pos >= 2 && quad_pos + pads <= 4 {
                continue;
            }
            if quad_pos == 1 {
                break;
            }
            return Err(if quad_pos == 0 && i == 0 {
                "Leading padding not allowed"
            } else {
                "Excess padding not allowed"
            }
            .into());
        }
        let Some(v) = b64_value(c) else { return Err("Only base64 data is allowed".into()) };
        if pads != 0 {
            return Err(if quad_pos + pads == 4 {
                "Excess data after padding"
            } else {
                "Discontinuous padding not allowed"
            }
            .into());
        }
        pads = 0;
        match quad_pos {
            0 => {
                quad_pos = 1;
                leftchar = v;
            }
            1 => {
                quad_pos = 2;
                out.push(leftchar << 2 | v >> 4);
                leftchar = v & 0x0f;
            }
            2 => {
                quad_pos = 3;
                out.push(leftchar << 4 | v >> 2);
                leftchar = v & 0x03;
            }
            _ => {
                quad_pos = 0;
                out.push(leftchar << 6 | v);
                leftchar = 0;
            }
        }
    }
    if quad_pos == 1 {
        return Err(format!(
            "Invalid base64-encoded string: number of data characters ({}) cannot be 1 more than a multiple of 4",
            out.len() / 3 * 4 + 1
        ));
    }
    if quad_pos != 0 && quad_pos + pads < 4 {
        return Err("Incorrect padding".into());
    }
    Ok(out)
}
