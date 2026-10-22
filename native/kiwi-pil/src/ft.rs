//! FFI to the vendored FreeType through `csrc/shim.c`.

use std::{
    ffi::{c_int, c_long, c_uint, c_ulong, c_void},
    marker::PhantomData,
    ptr::NonNull,
};

#[repr(C)]
struct RawBitmap {
    left: c_int,
    top: c_int,
    rows: c_uint,
    width: c_uint,
    pitch: c_int,
    pixel_mode: c_int,
    scale: c_int,
    buffer: *const u8,
}

unsafe extern "C" {
    fn kp_lib_new(out: *mut *mut c_void) -> c_int;
    fn kp_lib_free(lib: *mut c_void);
    fn kp_face_new(
        lib: *mut c_void,
        data: *const u8,
        len: c_long,
        index: c_long,
        size: f32,
        out: *mut *mut c_void,
    ) -> c_int;
    fn kp_face_free(face: *mut c_void);
    fn kp_has_kerning(face: *mut c_void) -> c_int;
    fn kp_size_metrics(face: *mut c_void, ascender: *mut c_long, descender: *mut c_long);
    fn kp_char_index(face: *mut c_void, ch: c_ulong) -> c_uint;
    fn kp_load_advance(face: *mut c_void, index: c_uint, flags: c_int, advance: *mut c_long) -> c_int;
    fn kp_kerning(face: *mut c_void, left: c_uint, right: c_uint, dx: *mut c_long, dy: *mut c_long) -> c_int;
    fn kp_cbox(face: *mut c_void, index: c_uint, flags: c_int, bbox: *mut c_long) -> c_int;
    fn kp_render(lib: *mut c_void, face: *mut c_void, index: c_uint, flags: c_int, out: *mut RawBitmap) -> c_int;
}

/// A FreeType error code (Pillow raises it as `OSError`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FtError(pub i32);

impl std::fmt::Display for FtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "freetype error {:#x}", self.0)
    }
}

impl std::error::Error for FtError {}

fn check(code: c_int) -> Result<(), FtError> {
    if code == 0 { Ok(()) } else { Err(FtError(code)) }
}

pub const FT_PIXEL_MODE_GRAY: i32 = 2;

/// One `FT_Library`. Not `Sync`: a render owns its own.
pub struct Library {
    raw: NonNull<c_void>,
}

impl Library {
    pub fn new() -> Result<Self, FtError> {
        let mut raw = std::ptr::null_mut();
        check(unsafe { kp_lib_new(&mut raw) })?;
        Ok(Library { raw: NonNull::new(raw).ok_or(FtError(-1))? })
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        unsafe { kp_lib_free(self.raw.as_ptr()) }
    }
}

/// A face over borrowed font bytes, sized like `ImageFont.truetype(path, size)`.
pub struct Face<'a> {
    raw: NonNull<c_void>,
    lib: &'a Library,
    _data: PhantomData<&'a [u8]>,
}

pub struct Bitmap<'f> {
    pub left: i32,
    pub top: i32,
    pub rows: u32,
    pub width: u32,
    pub pitch: i32,
    pub pixel_mode: i32,
    pub scale: u32,
    buffer: *const u8,
    _face: PhantomData<&'f ()>,
}

impl Bitmap<'_> {
    pub fn has_buffer(&self) -> bool {
        !self.buffer.is_null()
    }

    /// Row `y` of the bitmap (valid for `y < rows`).
    pub fn row(&self, y: u32) -> &[u8] {
        let start = y as isize * self.pitch as isize;
        unsafe { std::slice::from_raw_parts(self.buffer.offset(start), self.width as usize) }
    }
}

impl<'a> Face<'a> {
    pub fn new(lib: &'a Library, data: &'a [u8], index: i64, size: f32) -> Result<Self, FtError> {
        let mut raw = std::ptr::null_mut();
        check(unsafe {
            kp_face_new(lib.raw.as_ptr(), data.as_ptr(), data.len() as c_long, index as c_long, size, &mut raw)
        })?;
        Ok(Face { raw: NonNull::new(raw).ok_or(FtError(-1))?, lib, _data: PhantomData })
    }

    pub fn has_kerning(&self) -> bool {
        unsafe { kp_has_kerning(self.raw.as_ptr()) != 0 }
    }

    /// `size->metrics.ascender`, `descender` (26.6).
    pub fn size_metrics(&self) -> (i64, i64) {
        let (mut a, mut d) = (0, 0);
        unsafe { kp_size_metrics(self.raw.as_ptr(), &mut a, &mut d) };
        (a as i64, d as i64)
    }

    pub fn char_index(&self, ch: u32) -> u32 {
        unsafe { kp_char_index(self.raw.as_ptr(), ch as c_ulong) }
    }

    /// Loads the glyph and returns `metrics.horiAdvance` (26.6).
    pub fn load_advance(&self, index: u32, flags: i32) -> Result<i64, FtError> {
        let mut adv = 0;
        check(unsafe { kp_load_advance(self.raw.as_ptr(), index, flags, &mut adv) })?;
        Ok(adv as i64)
    }

    pub fn kerning(&self, left: u32, right: u32) -> Result<(i64, i64), FtError> {
        let (mut dx, mut dy) = (0, 0);
        check(unsafe { kp_kerning(self.raw.as_ptr(), left, right, &mut dx, &mut dy) })?;
        Ok((dx as i64, dy as i64))
    }

    /// Pixel cbox `(xMin, yMin, xMax, yMax)` of the loaded glyph.
    pub fn cbox(&self, index: u32, flags: i32) -> Result<[i64; 4], FtError> {
        let mut b: [c_long; 4] = [0; 4];
        check(unsafe { kp_cbox(self.raw.as_ptr(), index, flags, b.as_mut_ptr()) })?;
        Ok(b.map(|v| v as i64))
    }

    /// Loads and renders; the bitmap lives until the next call on this face.
    pub fn render(&mut self, index: u32, flags: i32) -> Result<Bitmap<'_>, FtError> {
        let mut r = RawBitmap {
            left: 0,
            top: 0,
            rows: 0,
            width: 0,
            pitch: 0,
            pixel_mode: 0,
            scale: 1,
            buffer: std::ptr::null(),
        };
        check(unsafe { kp_render(self.lib.raw.as_ptr(), self.raw.as_ptr(), index, flags, &mut r) })?;
        Ok(Bitmap {
            left: r.left,
            top: r.top,
            rows: r.rows,
            width: r.width,
            pitch: r.pitch,
            pixel_mode: r.pixel_mode,
            scale: r.scale as u32,
            buffer: r.buffer,
            _face: PhantomData,
        })
    }
}

impl Drop for Face<'_> {
    fn drop(&mut self) {
        unsafe { kp_face_free(self.raw.as_ptr()) }
    }
}
