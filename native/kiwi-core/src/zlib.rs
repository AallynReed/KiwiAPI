//! zlib with CPython's semantics. `inflate` is `decompressobj`: trailing bytes
//! after the stream are ignored and a truncated stream yields what it decoded.

use flate2::{Compress, Compression, Decompress, FlushCompress, FlushDecompress, Status};

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

/// `compressobj(level)`, fed `chunk`-sized `compress()` calls, then
/// `flush(Z_FINISH)`, or `flush(Z_SYNC_FLUSH)` when `sync` (the `.tmod` stream).
pub fn deflate(input: &[u8], level: u32, chunk: usize, sync: bool) -> Option<Vec<u8>> {
    let mut c = Compress::new(Compression::new(level), true);
    let mut out = Vec::with_capacity(input.len() / 2 + 64);
    let mut feed = |data: &[u8], flush: FlushCompress, out: &mut Vec<u8>| -> Option<()> {
        let start = c.total_in();
        loop {
            // Room for a whole call: a flush that fills the buffer exactly would be
            // repeated, and zlib-rs (unlike zlib) emits the repeat.
            let room = data.len() + data.len() / 8 + (1 << 17);
            if out.capacity() - out.len() < room {
                out.reserve(room);
            }
            let status = c.compress_vec(&data[(c.total_in() - start) as usize..], out, flush).ok()?;
            if status == Status::StreamEnd {
                return Some(());
            }
            let drained = (c.total_in() - start) as usize == data.len() && out.len() < out.capacity();
            if drained && !matches!(flush, FlushCompress::Finish) {
                return Some(());
            }
        }
    };
    for part in input.chunks(chunk.max(1)) {
        feed(part, FlushCompress::None, &mut out)?;
    }
    feed(&[], if sync { FlushCompress::Sync } else { FlushCompress::Finish }, &mut out)?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

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
