//! A project's bare repository on disk, read and written the way dulwich does
//! it: loose objects and packs are read, new objects are written loose
//! (zlib level -1 behind a `.lock` file, mode 0o444), and a missing repository
//! is created with `Repo.init_bare`'s exact layout.

use std::{
    collections::HashMap,
    fs,
    io::{self, Write},
    os::raw::{c_int, c_void},
    path::{Path, PathBuf},
    sync::OnceLock,
};

use libz_sys as z;
use memmap2::Mmap;
use sha1::{Digest, Sha1};

pub type Oid = [u8; 20];

pub const ZERO_HEX: &str = "0000000000000000000000000000000000000000";

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("corrupt: {0}")]
    Corrupt(String),
    /// A lookup Python answers with `KeyError`.
    #[error("missing object {0}")]
    Missing(String),
    /// Anything else Python raises (an unhandled 500 there too).
    #[error("{0}")]
    Invalid(String),
}

pub type GitResult<T> = Result<T, GitError>;

fn corrupt(what: impl Into<String>) -> GitError {
    GitError::Corrupt(what.into())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Commit,
    Tree,
    Blob,
    Tag,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Commit => "commit",
            Kind::Tree => "tree",
            Kind::Blob => "blob",
            Kind::Tag => "tag",
        }
    }

    fn from_name(b: &[u8]) -> Option<Kind> {
        Some(match b {
            b"commit" => Kind::Commit,
            b"tree" => Kind::Tree,
            b"blob" => Kind::Blob,
            b"tag" => Kind::Tag,
            _ => return None,
        })
    }

    fn from_pack(t: u8) -> Option<Kind> {
        Some(match t {
            1 => Kind::Commit,
            2 => Kind::Tree,
            3 => Kind::Blob,
            4 => Kind::Tag,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Object {
    pub kind: Kind,
    pub data: Vec<u8>,
}

pub fn to_hex(oid: &Oid) -> String {
    hex::encode(oid)
}

/// `hex_to_sha` on 40 lowercase-or-not hex characters.
pub fn from_hex(s: &[u8]) -> Option<Oid> {
    if s.len() != 40 {
        return None;
    }
    let mut out = [0u8; 20];
    hex::decode_to_slice(s, &mut out).ok()?;
    Some(out)
}

pub fn object_id(kind: Kind, data: &[u8]) -> Oid {
    let mut h = Sha1::new();
    h.update(format!("{} {}\0", kind.name(), data.len()).as_bytes());
    h.update(data);
    let mut oid = [0u8; 20];
    oid.copy_from_slice(&h.finalize());
    oid
}

struct Pack {
    idx: Mmap,
    data: Mmap,
    count: usize,
    v2: bool,
}

impl Pack {
    fn open(idx_path: &Path) -> io::Result<Option<Pack>> {
        let pack_path = idx_path.with_extension("pack");
        let (Ok(fi), Ok(fp)) = (fs::File::open(idx_path), fs::File::open(&pack_path)) else { return Ok(None) };
        if fi.metadata()?.len() < 1032 || fp.metadata()?.len() < 32 {
            return Ok(None);
        }
        // SAFETY: packs and their indexes are immutable once written.
        let (idx, data) = unsafe { (Mmap::map(&fi)?, Mmap::map(&fp)?) };
        let v2 = idx[..4] == [0xff, b't', b'O', b'c'];
        let fan = if v2 { 8 } else { 0 };
        let count = u32::from_be_bytes(idx[fan + 255 * 4..fan + 256 * 4].try_into().unwrap_or_default()) as usize;
        Ok(Some(Pack { idx, data, count, v2 }))
    }

    fn be32(&self, at: usize) -> Option<u32> {
        Some(u32::from_be_bytes(self.idx.get(at..at + 4)?.try_into().ok()?))
    }

    fn name_at(&self, i: usize) -> Option<&[u8]> {
        if self.v2 {
            let at = 8 + 1024 + i * 20;
            self.idx.get(at..at + 20)
        } else {
            let at = 1024 + i * 24 + 4;
            self.idx.get(at..at + 20)
        }
    }

    fn find(&self, oid: &Oid) -> Option<usize> {
        let fan = if self.v2 { 8 } else { 0 };
        let first = oid[0] as usize;
        let mut lo = if first == 0 { 0 } else { self.be32(fan + (first - 1) * 4)? as usize };
        let mut hi = self.be32(fan + first * 4)? as usize;
        while lo < hi {
            let mid = (lo + hi) / 2;
            match self.name_at(mid)?.cmp(&oid[..]) {
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
                std::cmp::Ordering::Equal => return self.offset_at(mid),
            }
        }
        None
    }

    fn offset_at(&self, i: usize) -> Option<usize> {
        if !self.v2 {
            return self.be32(1024 + i * 24).map(|o| o as usize);
        }
        let base = 8 + 1024 + self.count * 24;
        let off = self.be32(base + i * 4)?;
        if off & 0x8000_0000 == 0 {
            return Some(off as usize);
        }
        let at = base + self.count * 4 + (off & 0x7fff_ffff) as usize * 8;
        Some(u64::from_be_bytes(self.idx.get(at..at + 8)?.try_into().ok()?) as usize)
    }
}

fn inflate_sized(input: &[u8], size: usize) -> GitResult<Vec<u8>> {
    let mut buf = vec![0u8; size];
    match kiwi_core::zlib::inflate_exact(input, &mut buf) {
        Some(_) => Ok(buf),
        None => Err(corrupt("bad zlib stream in pack")),
    }
}

fn varint(d: &[u8], mut i: usize) -> Option<(usize, usize)> {
    let mut n = 0usize;
    let mut shift = 0;
    loop {
        let c = *d.get(i)?;
        i += 1;
        n |= ((c & 0x7f) as usize).checked_shl(shift)?;
        shift += 7;
        if c & 0x80 == 0 {
            return Some((n, i));
        }
    }
}

pub fn apply_delta(base: &[u8], delta: &[u8]) -> Option<Vec<u8>> {
    let (src, i) = varint(delta, 0)?;
    let (dst, mut i) = varint(delta, i)?;
    if src != base.len() {
        return None;
    }
    let mut out = Vec::with_capacity(dst);
    while i < delta.len() {
        let op = delta[i];
        i += 1;
        if op & 0x80 != 0 {
            let (mut off, mut size) = (0usize, 0usize);
            for k in 0..4 {
                if op & (1 << k) != 0 {
                    off |= (*delta.get(i)? as usize) << (8 * k);
                    i += 1;
                }
            }
            for k in 0..3 {
                if op & (0x10 << k) != 0 {
                    size |= (*delta.get(i)? as usize) << (8 * k);
                    i += 1;
                }
            }
            if size == 0 {
                size = 0x10000;
            }
            out.extend_from_slice(base.get(off..off.checked_add(size)?)?);
        } else if op != 0 {
            out.extend_from_slice(delta.get(i..i + op as usize)?);
            i += op as usize;
        } else {
            return None;
        }
    }
    (out.len() == dst).then_some(out)
}

pub struct Repo {
    pub dir: PathBuf,
    packs: OnceLock<Vec<Pack>>,
    cache: std::sync::Mutex<HashMap<Oid, Object>>,
}

/// `(d / "objects").is_dir() or (d / "HEAD").is_file()`.
pub fn exists(dir: &Path) -> bool {
    dir.join("objects").is_dir() || dir.join("HEAD").is_file()
}

impl Repo {
    pub fn at(dir: &Path) -> Repo {
        Repo { dir: dir.to_path_buf(), packs: OnceLock::new(), cache: Default::default() }
    }

    /// `_open`: the repository when it exists.
    pub fn open(dir: &Path) -> Option<Repo> {
        exists(dir).then(|| Repo::at(dir))
    }

    /// `_ensure`: open it, or create it as dulwich does with HEAD on `main`.
    pub fn ensure(dir: &Path) -> GitResult<Repo> {
        if !exists(dir) {
            fs::create_dir_all(dir)?;
            init_bare(dir)?;
        }
        Ok(Repo::at(dir))
    }

    fn packs(&self) -> &[Pack] {
        self.packs.get_or_init(|| {
            let mut out = Vec::new();
            let Ok(rd) = fs::read_dir(self.dir.join("objects").join("pack")) else { return out };
            let mut idx: Vec<PathBuf> = rd
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|x| x == "idx"))
                .collect();
            idx.sort();
            for p in idx {
                if let Ok(Some(pack)) = Pack::open(&p) {
                    out.push(pack);
                }
            }
            out
        })
    }

    fn loose_path(&self, hex: &str) -> PathBuf {
        self.dir.join("objects").join(&hex[..2]).join(&hex[2..])
    }

    fn read_loose(&self, oid: &Oid) -> GitResult<Option<Object>> {
        let raw = match fs::read(self.loose_path(&to_hex(oid))) {
            Ok(b) => b,
            Err(e) if matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let data = kiwi_core::zlib::inflate(&raw, usize::MAX).ok_or_else(|| corrupt("bad loose object"))?.data;
        let nul = data.iter().position(|&b| b == 0).ok_or_else(|| corrupt("loose header"))?;
        let header = &data[..nul];
        let sp = header.iter().position(|&b| b == b' ').ok_or_else(|| corrupt("loose header"))?;
        let kind = Kind::from_name(&header[..sp]).ok_or_else(|| corrupt("loose type"))?;
        Ok(Some(Object { kind, data: data[nul + 1..].to_vec() }))
    }

    fn read_packed(&self, pack: &Pack, off: usize, depth: u32) -> GitResult<Object> {
        if depth > 4096 {
            return Err(corrupt("delta chain too deep"));
        }
        let d = &pack.data[..];
        let short = || corrupt("truncated pack");
        let mut i = off;
        let mut c = *d.get(i).ok_or_else(short)?;
        i += 1;
        let typ = (c >> 4) & 7;
        let mut size = (c & 15) as usize;
        let mut shift = 4;
        while c & 0x80 != 0 {
            c = *d.get(i).ok_or_else(short)?;
            i += 1;
            size |= ((c & 0x7f) as usize).checked_shl(shift).ok_or_else(short)?;
            shift += 7;
        }
        match typ {
            6 => {
                let mut c = *d.get(i).ok_or_else(short)?;
                i += 1;
                let mut ofs = (c & 0x7f) as usize;
                while c & 0x80 != 0 {
                    c = *d.get(i).ok_or_else(short)?;
                    i += 1;
                    ofs = ((ofs + 1) << 7) | (c & 0x7f) as usize;
                }
                let base_off = off.checked_sub(ofs).ok_or_else(short)?;
                let delta = inflate_sized(d.get(i..).ok_or_else(short)?, size)?;
                let base = self.read_packed(pack, base_off, depth + 1)?;
                let data = apply_delta(&base.data, &delta).ok_or_else(|| corrupt("bad delta"))?;
                Ok(Object { kind: base.kind, data })
            }
            7 => {
                let base_oid: Oid = d.get(i..i + 20).ok_or_else(short)?.try_into().map_err(|_| short())?;
                let delta = inflate_sized(d.get(i + 20..).ok_or_else(short)?, size)?;
                let base = self.read(&base_oid)?.ok_or_else(|| GitError::Missing(to_hex(&base_oid)))?;
                let data = apply_delta(&base.data, &delta).ok_or_else(|| corrupt("bad delta"))?;
                Ok(Object { kind: base.kind, data })
            }
            t => {
                let kind = Kind::from_pack(t).ok_or_else(|| corrupt("bad pack type"))?;
                Ok(Object { kind, data: inflate_sized(d.get(i..).ok_or_else(short)?, size)? })
            }
        }
    }

    /// `object_store[sha]`: None where dulwich raises `KeyError`.
    pub fn read(&self, oid: &Oid) -> GitResult<Option<Object>> {
        if let Some(o) = self.cache.lock().ok().and_then(|c| c.get(oid).cloned()) {
            return Ok(Some(o));
        }
        let mut found = None;
        for pack in self.packs() {
            if let Some(off) = pack.find(oid) {
                found = Some(self.read_packed(pack, off, 0)?);
                break;
            }
        }
        if found.is_none() {
            found = self.read_loose(oid)?;
        }
        if let (Some(o), Ok(mut c)) = (&found, self.cache.lock())
            && o.kind != Kind::Blob
        {
            c.insert(*oid, o.clone());
        }
        Ok(found)
    }

    /// `object_store[sha]` where a missing object is an error.
    pub fn get(&self, oid: &Oid) -> GitResult<Object> {
        self.read(oid)?.ok_or_else(|| GitError::Missing(to_hex(oid)))
    }

    /// `DiskObjectStore.add_object`: an existing loose object only has its
    /// mtime refreshed; a new one is written behind a lock file.
    pub fn write(&self, kind: Kind, data: &[u8]) -> GitResult<Oid> {
        let oid = object_id(kind, data);
        let hex = to_hex(&oid);
        let dir = self.dir.join("objects").join(&hex[..2]);
        match fs::create_dir(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        let path = dir.join(&hex[2..]);
        match fs::File::open(&path) {
            Ok(f) => {
                let _ = f.set_modified(std::time::SystemTime::now());
                return Ok(oid);
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let header = format!("{} {}\0", kind.name(), data.len());
        let body = deflate(&[header.as_bytes(), data], -1, 15).ok_or_else(|| GitError::Invalid("zlib".into()))?;
        locked_write(&path, &body, 0o444, false)?;
        Ok(oid)
    }
}

/// `GitFile(path, "wb", mask)`: `<path>.lock` created exclusively, written,
/// optionally fsynced, then renamed over `path`.
pub fn locked_write(path: &Path, data: &[u8], mode: u32, fsync: bool) -> GitResult<()> {
    let lock = lock_path(path);
    let mut f = create_lock(&lock, mode)?;
    let res = (|| -> io::Result<()> {
        f.write_all(data)?;
        if fsync {
            f.sync_all()?;
        }
        drop(f);
        fs::rename(&lock, path)
    })();
    if res.is_err() {
        let _ = fs::remove_file(&lock);
    }
    res.map_err(Into::into)
}

pub fn lock_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".lock");
    PathBuf::from(s)
}

/// The exclusive lock file (`FileLocked` when it already exists).
pub fn create_lock(lock: &Path, mode: u32) -> GitResult<fs::File> {
    let mut opts = fs::OpenOptions::new();
    opts.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(mode);
    }
    #[cfg(not(unix))]
    let _ = mode;
    opts.open(lock).map_err(|e| {
        if e.kind() == io::ErrorKind::AlreadyExists {
            GitError::Invalid(format!("file locked: {}", lock.display()))
        } else {
            e.into()
        }
    })
}

/// `Repo._determine_file_mode`: whether toggling the owner's exec bit sticks.
fn file_mode_trusted(dir: &Path) -> io::Result<bool> {
    let probe = dir.join(".probe-permissions");
    fs::write(&probe, b"")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let m1 = fs::symlink_metadata(&probe)?.permissions().mode();
        if fs::set_permissions(&probe, fs::Permissions::from_mode(m1 ^ 0o100)).is_err() {
            return Ok(false);
        }
        let m2 = fs::symlink_metadata(&probe)?.permissions().mode();
        fs::remove_file(&probe)?;
        Ok(m1 != m2 && m2 & 0o100 != 0)
    }
    #[cfg(not(unix))]
    {
        // Python's chmod on Windows only flips the read-only flag: the exec bit never differs.
        fs::remove_file(&probe)?;
        Ok(false)
    }
}

/// `Repo.init_bare(dir)` then `set_symbolic_ref(HEAD, refs/heads/main)`.
pub fn init_bare(dir: &Path) -> GitResult<()> {
    for d in ["branches", "refs", "refs/tags", "refs/heads", "hooks", "info"] {
        fs::create_dir(dir.join(d))?;
    }
    match fs::create_dir(dir.join("objects")) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    fs::create_dir(dir.join("objects").join("info"))?;
    fs::create_dir(dir.join("objects").join("pack"))?;
    locked_write(&dir.join("HEAD"), b"ref: refs/heads/master\n", 0o644, true)?;
    locked_write(&dir.join("description"), b"Unnamed repository", 0o644, true)?;
    let filemode = if file_mode_trusted(dir)? { "true" } else { "false" };
    let config = format!(
        "[core]\n\trepositoryformatversion = 0\n\tfilemode = {filemode}\n\tbare = true\n\tlogallrefupdates = true\n"
    );
    locked_write(&dir.join("config"), config.as_bytes(), 0o644, true)?;
    locked_write(&dir.join("info").join("exclude"), b"", 0o644, true)?;
    locked_write(&dir.join("HEAD"), b"ref: refs/heads/main\n", 0o644, true)?;
    Ok(())
}

unsafe extern "C" fn zalloc(_: *mut c_void, items: z::uInt, size: z::uInt) -> *mut c_void {
    unsafe { libc_calloc(items as usize, size as usize) }
}

unsafe extern "C" fn zfree(_: *mut c_void, ptr: *mut c_void) {
    unsafe { libc_free(ptr) }
}

unsafe extern "C" {
    #[link_name = "calloc"]
    fn libc_calloc(n: usize, size: usize) -> *mut c_void;
    #[link_name = "free"]
    fn libc_free(ptr: *mut c_void);
}

/// `zlib.compressobj(level, DEFLATED, wbits)`, one `compress()` per chunk,
/// then `flush()`: zlib itself (libz-sys), so the bytes are CPython's.
pub fn deflate(chunks: &[&[u8]], level: c_int, wbits: c_int) -> Option<Vec<u8>> {
    let total: usize = chunks.iter().map(|c| c.len()).sum();
    // SAFETY: a z_stream with our allocator, initialised by deflateInit2_
    // and torn down by deflateEnd on every path below.
    unsafe {
        let mut s = Box::new(z::z_stream {
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
        let rc = z::deflateInit2_(
            &mut *s,
            level,
            z::Z_DEFLATED,
            wbits,
            8,
            z::Z_DEFAULT_STRATEGY,
            z::zlibVersion(),
            std::mem::size_of::<z::z_stream>() as c_int,
        );
        if rc != z::Z_OK {
            return None;
        }
        let bound = z::deflateBound(&mut *s, total as z::uLong) as usize + 64;
        let mut out: Vec<u8> = vec![0u8; bound];
        s.next_out = out.as_mut_ptr();
        s.avail_out = bound as z::uInt;
        let mut ok = true;
        for c in chunks {
            s.next_in = c.as_ptr() as *mut u8;
            s.avail_in = c.len() as z::uInt;
            while s.avail_in > 0 {
                let rc = z::deflate(&mut *s, z::Z_NO_FLUSH);
                if rc != z::Z_OK || s.avail_out == 0 {
                    ok = false;
                    break;
                }
            }
        }
        while ok {
            match z::deflate(&mut *s, z::Z_FINISH) {
                z::Z_STREAM_END => break,
                z::Z_OK | z::Z_BUF_ERROR if s.avail_out > 0 => {}
                _ => ok = false,
            }
        }
        let written = bound - s.avail_out as usize;
        z::deflateEnd(&mut *s);
        if !ok {
            return None;
        }
        out.truncate(written);
        Some(out)
    }
}
