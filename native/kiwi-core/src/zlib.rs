//! zlib with CPython's semantics. `inflate` is `decompressobj`: trailing bytes
//! after the stream are ignored and a truncated stream yields what it decoded.
//! Deflate is classic zlib (kiwi-zlib), whose bytes are CPython's; inflate is
//! zlib-rs, whose output is the same.

use flate2::{Decompress, FlushDecompress, Status};

pub struct Inflated {
    pub data: Vec<u8>,
    /// The stream ended (`decompressobj().eof`).
    pub eof: bool,
}

/// None where Python's zlib raises, and when the output reached `cap` before
/// the stream ended (Python then decides between a bomb and a short read).
pub fn inflate(input: &[u8], cap: usize) -> Option<Inflated> {
    let mut d = Decompress::new(true);
    let mut out = Vec::with_capacity(input.len().saturating_mul(4).clamp(256, cap.max(256)));
    loop {
        if out.len() == out.capacity() {
            if out.len() >= cap {
                return None;
            }
            out.reserve(out.len().max(4096));
        }
        let (in0, out0) = (d.total_in(), d.total_out());
        let status = d.decompress_vec(&input[in0 as usize..], &mut out, FlushDecompress::None).ok()?;
        if out.len() > cap {
            return None;
        }
        if status == Status::StreamEnd {
            return Some(Inflated { data: out, eof: true });
        }
        if d.total_in() == in0 && d.total_out() == out0 && out.len() < out.capacity() {
            return Some(Inflated { data: out, eof: false });
        }
    }
}

/// `inflate` straight into `buf`: Some(eof) when the output is exactly
/// `buf.len()` bytes, None when it is longer, shorter or invalid.
pub fn inflate_exact(input: &[u8], buf: &mut [u8]) -> Option<bool> {
    let mut d = Decompress::new(true);
    loop {
        let (in0, out0) = (d.total_in() as usize, d.total_out() as usize);
        let status = d.decompress(&input[in0..], &mut buf[out0..], FlushDecompress::None).ok()?;
        let out = d.total_out() as usize;
        if status == Status::StreamEnd {
            return (out == buf.len()).then_some(true);
        }
        if d.total_in() as usize == in0 && out == out0 {
            if out < buf.len() {
                return None;
            }
            // Full: any further byte means the guess was short.
            let mut probe = [0u8; 1];
            let status = d.decompress(&input[d.total_in() as usize..], &mut probe, FlushDecompress::None).ok()?;
            return (d.total_out() as usize == buf.len()).then_some(status == Status::StreamEnd);
        }
    }
}

/// `zlib.compress(data, level)`; with a `chunk` shorter than the input, or
/// `sync`, `compressobj(level)` fed `chunk`-sized `compress()` calls, then
/// `flush(Z_FINISH)`, or `flush(Z_SYNC_FLUSH)` when `sync` (the `.tmod` stream).
pub fn deflate(input: &[u8], level: u32, chunk: usize, sync: bool) -> Option<Vec<u8>> {
    let level = i32::try_from(level).ok()?;
    if !sync && chunk >= input.len() {
        return kiwi_zlib::compress(input, level, 15);
    }
    let mut z = kiwi_zlib::compressobj(level, 15, 8, kiwi_zlib::Z_DEFAULT_STRATEGY)?;
    let mut out = Vec::with_capacity(input.len() / 2 + 64);
    for part in input.chunks(chunk.max(1)) {
        out.extend(z.compress(part)?);
    }
    out.extend(z.flush(if sync { kiwi_zlib::Z_SYNC_FLUSH } else { kiwi_zlib::Z_FINISH })?);
    Some(out)
}

/// What `gzip.open(path, "wb", compresslevel=level).write(data)` leaves on
/// disk: the header names `basename(path)` (minus `.gz`, if Latin-1).
pub fn gzip_file(data: &[u8], level: u32, basename: &str, mtime: u32) -> Option<Vec<u8>> {
    let fname: Option<Vec<u8>> = basename.chars().map(|c| u8::try_from(c as u32).ok()).collect();
    let fname = fname.map(|f| f.strip_suffix(b".gz").map(<[u8]>::to_vec).unwrap_or(f)).unwrap_or_default();
    let mut out = vec![0x1f, 0x8b, 8, if fname.is_empty() { 0 } else { 8 }];
    out.extend_from_slice(&mtime.to_le_bytes());
    out.push(match level {
        9 => 2,
        1 => 4,
        _ => 0,
    });
    out.push(255);
    if !fname.is_empty() {
        out.extend_from_slice(&fname);
        out.push(0);
    }
    let mut z = kiwi_zlib::compressobj(i32::try_from(level).ok()?, -15, 8, kiwi_zlib::Z_DEFAULT_STRATEGY)?;
    out.extend(z.compress(data)?);
    out.extend(z.flush(kiwi_zlib::Z_FINISH)?);
    let mut crc = flate2::Crc::new();
    crc.update(data);
    out.extend_from_slice(&crc.sum().to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    Some(out)
}

/// `gzip.open(path).read()`: every member, `Err` with the message of what
/// Python raises for a wrong magic number or a truncated stream.
pub fn gunzip(data: &[u8]) -> Result<Vec<u8>, String> {
    if data.is_empty() {
        return Ok(Vec::new());
    }
    if !data.starts_with(&[0x1f, 0x8b]) {
        return Err(format!("Not a gzipped file ({})", bytes_repr(&data[..data.len().min(2)])));
    }
    let mut out = Vec::with_capacity(data.len().saturating_mul(4));
    match std::io::Read::read_to_end(&mut flate2::read::MultiGzDecoder::new(data), &mut out) {
        Ok(_) => Ok(out),
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            Err("Compressed file ended before the end-of-stream marker was reached".into())
        }
        Err(e) => Err(e.to_string()),
    }
}

/// `repr(bytes)`.
fn bytes_repr(b: &[u8]) -> String {
    let quote = if b.contains(&b'\'') && !b.contains(&b'"') { '"' } else { '\'' };
    let mut out = format!("b{quote}");
    for &c in b {
        match c {
            b'\\' => out.push_str("\\\\"),
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            c if char::from(c) == quote => {
                out.push('\\');
                out.push(quote);
            }
            0x20..0x7f => out.push(char::from(c)),
            c => out.push_str(&format!("\\x{c:02x}")),
        }
    }
    out.push(quote);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gunzips_members_and_reports_like_python() {
        let one = gzip_file(b"one\r\n", 6, "a", 0).unwrap();
        let two = gzip_file(b"two", 6, "b", 0).unwrap();
        assert_eq!(gunzip(&[one.clone(), two].concat()).unwrap(), b"one\r\ntwo");
        assert_eq!(gunzip(b"").unwrap(), b"");
        assert_eq!(gunzip(b"A$").unwrap_err(), "Not a gzipped file (b'A$')");
        assert_eq!(gunzip(b"\x00\n").unwrap_err(), "Not a gzipped file (b'\\x00\\n')");
        assert_eq!(gunzip(b"'").unwrap_err(), "Not a gzipped file (b\"'\")");
        assert_eq!(
            gunzip(&one[..one.len() - 6]).unwrap_err(),
            "Compressed file ended before the end-of-stream marker was reached"
        );
    }

    #[test]
    fn gzip_file_round_trips() {
        let data = b"N$C$1 = A$B##1;x;1\n".repeat(1000);
        let gz = gzip_file(&data, 6, ".1700000000.cfg.gz.tmp", 7).unwrap();
        assert_eq!(&gz[..10], &[0x1f, 0x8b, 8, 8, 7, 0, 0, 0, 0, 255]);
        assert_eq!(&gz[10..33], b".1700000000.cfg.gz.tmp\0");
        let mut d = flate2::read::GzDecoder::new(&gz[..]);
        let mut back = Vec::new();
        std::io::Read::read_to_end(&mut d, &mut back).unwrap();
        assert_eq!(back, data);
    }

    // zlib.compress(b"hello hello hello")
    const HELLO: &[u8] = &[120, 156, 203, 72, 205, 201, 201, 87, 200, 64, 144, 0, 58, 46, 6, 125];

    #[test]
    fn inflates_and_ignores_trailing() {
        let mut v = HELLO.to_vec();
        v.extend_from_slice(b"junk");
        let r = inflate(&v, usize::MAX).unwrap();
        assert_eq!(r.data, b"hello hello hello");
        assert!(r.eof);
    }

    #[test]
    fn truncated_is_partial_and_garbage_errors() {
        let r = inflate(&HELLO[..8], usize::MAX).unwrap();
        assert!(!r.eof);
        assert!(inflate(b"\x78\x9c\xff\xff\xff", usize::MAX).is_none());
    }

    #[test]
    fn exact_inflate_only_on_the_right_size() {
        for (len, want) in [(17, Some(true)), (16, None), (18, None), (0, None)] {
            assert_eq!(inflate_exact(HELLO, &mut vec![0; len]), want, "{len}");
        }
        assert_eq!(
            inflate_exact(&HELLO[..12], &mut vec![0; inflate(&HELLO[..12], usize::MAX).unwrap().data.len()]),
            Some(false)
        );
    }

    #[test]
    fn cap_defers_to_python() {
        assert!(inflate(HELLO, 4).is_none());
        assert!(inflate(HELLO, 17).is_some());
    }
}
