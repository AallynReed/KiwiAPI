//! Classic zlib deflate, the library CPython's `zlib` module and Pillow's
//! Linux wheels link (zlib-ng and zlib-rs choose other matches), driven the
//! way CPython and Pillow drive it. Inflate is unaffected and stays elsewhere.
//! libz-sys builds zlib 1.3.2 from source; native/port/parity/zlib_corpus.py
//! shows its deflate output equals CPython's zlib 1.3.1 byte for byte.

use std::os::raw::{c_int, c_void};

use libz_sys as z;

pub use z::{Z_DEFAULT_STRATEGY, Z_FILTERED, Z_FINISH, Z_NO_FLUSH, Z_STREAM_END, Z_SYNC_FLUSH};

unsafe extern "C" fn zalloc(_: *mut c_void, items: z::uInt, size: z::uInt) -> *mut c_void {
    unsafe { calloc(items as usize, size as usize) }
}

unsafe extern "C" fn zfree(_: *mut c_void, ptr: *mut c_void) {
    unsafe { free(ptr) }
}

unsafe extern "C" {
    fn calloc(n: usize, size: usize) -> *mut c_void;
    fn free(ptr: *mut c_void);
}

/// The linked zlib's `zlibVersion()`.
pub fn version() -> String {
    // SAFETY: zlib returns a static NUL-terminated string.
    unsafe { std::ffi::CStr::from_ptr(z::zlibVersion()) }.to_string_lossy().into_owned()
}

/// A `deflateInit2`-ed stream.
pub struct Deflate(Box<z::z_stream>);

// One owner at a time; zlib keeps no thread-local state.
unsafe impl Send for Deflate {}

/// CPython's `_BlocksOutputBuffer`: the output buffer of one `compress()` /
/// `flush()` call grows by these, and stored blocks (level 0) follow them.
const BLOCKS: [usize; 17] = [
    32 << 10,
    64 << 10,
    256 << 10,
    1 << 20,
    4 << 20,
    8 << 20,
    16 << 20,
    16 << 20,
    32 << 20,
    32 << 20,
    32 << 20,
    32 << 20,
    64 << 20,
    64 << 20,
    128 << 20,
    128 << 20,
    256 << 20,
];

impl Deflate {
    /// `deflateInit2(level, Z_DEFLATED, wbits, mem_level, strategy)`.
    pub fn new(level: i32, wbits: i32, mem_level: i32, strategy: i32) -> Option<Self> {
        let mut strm = Box::new(z::z_stream {
            next_in: std::ptr::null_mut(),
            avail_in: 0,
            total_in: 0,
            next_out: std::ptr::null_mut(),
            avail_out: 0,
            total_out: 0,
            msg: std::ptr::null_mut(),
            state: std::ptr::null_mut(),
            zalloc,
            zfree,
            opaque: std::ptr::null_mut(),
            data_type: 0,
            adler: 0,
            reserved: 0,
        });
        // SAFETY: a fresh stream with live allocator callbacks.
        let rc = unsafe {
            z::deflateInit2_(
                &mut *strm,
                level,
                z::Z_DEFLATED,
                wbits,
                mem_level,
                strategy,
                z::zlibVersion(),
                std::mem::size_of::<z::z_stream>() as c_int,
            )
        };
        (rc == z::Z_OK).then_some(Deflate(strm))
    }

    /// One `deflate(flush)` call: bytes consumed, bytes written, zlib's code.
    pub fn step(&mut self, input: &[u8], out: &mut [u8], flush: i32) -> (usize, usize, i32) {
        let (inn, outn) = (input.len().min(u32::MAX as usize), out.len().min(u32::MAX as usize));
        let s = &mut *self.0;
        s.next_in = input.as_ptr() as *mut _;
        s.avail_in = inn as z::uInt;
        s.next_out = out.as_mut_ptr();
        s.avail_out = outn as z::uInt;
        // SAFETY: both buffers outlive the call and their lengths are exact.
        let rc = unsafe { z::deflate(s, flush) };
        let used = (inn - s.avail_in as usize, outn - s.avail_out as usize);
        s.next_in = std::ptr::null_mut();
        s.next_out = std::ptr::null_mut();
        (used.0, used.1, rc)
    }

    /// CPython's call loop: `deflate` until the output buffer has room left,
    /// the buffer growing block by block; `last` for the final input chunk.
    fn drive(&mut self, input: &[u8], last: i32) -> Option<Vec<u8>> {
        let mut out = Vec::new();
        let mut blocks = 0;
        let mut free = 0;
        let mut rest = input;
        loop {
            let (mut chunk, tail) = rest.split_at(rest.len().min(u32::MAX as usize));
            rest = tail;
            let flush = if rest.is_empty() { last } else { Z_NO_FLUSH };
            loop {
                if free == 0 {
                    free = BLOCKS[blocks.min(BLOCKS.len() - 1)];
                    blocks += 1;
                    out.resize(out.len() + free, 0);
                }
                let at = out.len() - free;
                let (used, made, rc) = self.step(chunk, &mut out[at..], flush);
                chunk = &chunk[used..];
                free -= made;
                if rc == z::Z_STREAM_ERROR {
                    return None;
                }
                if free != 0 {
                    break;
                }
            }
            if rest.is_empty() {
                break;
            }
        }
        out.truncate(out.len() - free);
        Some(out)
    }

    /// `compressobj.compress(data)`.
    pub fn compress(&mut self, data: &[u8]) -> Option<Vec<u8>> {
        self.drive(data, Z_NO_FLUSH)
    }

    /// `compressobj.flush(mode)`.
    pub fn flush(&mut self, mode: i32) -> Option<Vec<u8>> {
        if mode == Z_NO_FLUSH {
            return Some(Vec::new());
        }
        self.drive(&[], mode)
    }
}

impl Drop for Deflate {
    fn drop(&mut self) {
        // SAFETY: initialised in `new`.
        unsafe { z::deflateEnd(&mut *self.0) };
    }
}

/// `zlib.compressobj(level, DEFLATED, wbits, mem_level, strategy)`.
pub fn compressobj(level: i32, wbits: i32, mem_level: i32, strategy: i32) -> Option<Deflate> {
    Deflate::new(level, wbits, mem_level, strategy)
}

/// `zlib.compress(data, level, wbits)`.
pub fn compress(data: &[u8], level: i32, wbits: i32) -> Option<Vec<u8>> {
    Deflate::new(level, wbits, 8, Z_DEFAULT_STRATEGY)?.drive(data, Z_FINISH)
}

/// `gzip.compress(data, level, mtime=mtime)`: zlib's gzip stream with the
/// header's mtime set and its OS byte 255.
pub fn gzip_compress(data: &[u8], level: i32, mtime: u32) -> Option<Vec<u8>> {
    let mut out = compress(data, level, 31)?;
    out[4..8].copy_from_slice(&mtime.to_le_bytes());
    out[9] = 255;
    Some(out)
}

/// `compressobj(level, DEFLATED, -15)`: `compress(data)` then `flush()`, as
/// `zipfile` writes a ZIP_DEFLATED member.
pub fn deflate_raw(data: &[u8], level: i32) -> Option<Vec<u8>> {
    let mut z = compressobj(level, -15, 8, Z_DEFAULT_STRATEGY)?;
    let mut out = z.compress(data)?;
    out.extend(z.flush(Z_FINISH)?);
    Some(out)
}

/// Pillow's `ImagingZipEncode` for PNG: `deflateInit2(level, Z_DEFLATED, 15,
/// 9, strategy)`, each encoder call filling a `bufsize` buffer (one IDAT
/// each): a row the last call left half-consumed goes in first, then one
/// `deflate(Z_NO_FLUSH)` per filtered row until the buffer is full, then
/// `Z_FINISH`. A stream that ends exactly on a full buffer gets one more,
/// empty, call.
pub fn zip_encode(raw: &[u8], row: usize, level: i32, strategy: i32, bufsize: usize) -> Option<Vec<Vec<u8>>> {
    let mut d = Deflate::new(level, 15, 9, strategy)?;
    let mut parts = Vec::new();
    let (mut next, mut pending): (usize, &[u8]) = (0, &[]);
    let (mut finishing, mut ended) = (false, false);
    loop {
        if ended {
            parts.push(Vec::new());
            return Some(parts);
        }
        let mut buf = vec![0u8; bufsize];
        let mut used = 0;
        if !pending.is_empty() {
            let (n, o, rc) = d.step(pending, &mut buf[used..], Z_NO_FLUSH);
            if rc < 0 && rc != z::Z_BUF_ERROR {
                return None;
            }
            pending = &pending[n..];
            used += o;
        }
        while !finishing && used < bufsize {
            if next >= raw.len() {
                finishing = true;
                break;
            }
            let end = (next + row.max(1)).min(raw.len());
            let (n, o, rc) = d.step(&raw[next..end], &mut buf[used..], Z_NO_FLUSH);
            if rc < 0 && rc != z::Z_BUF_ERROR {
                return None;
            }
            pending = &raw[next + n..end];
            next = end;
            used += o;
        }
        if finishing {
            while used < bufsize {
                let (_, o, rc) = d.step(&[], &mut buf[used..], Z_FINISH);
                used += o;
                if rc == Z_STREAM_END {
                    ended = true;
                    break;
                }
                if rc < 0 && rc != z::Z_BUF_ERROR {
                    return None;
                }
            }
        }
        buf.truncate(used);
        parts.push(buf);
        if ended && used < bufsize {
            return Some(parts);
        }
    }
}

#[cfg(test)]
mod tests;
