//! `zipfile.ZipFile(BytesIO(data))` reading as CPython 3.13 does it on Linux:
//! the table of contents and `read(info)`, with its exception text.

use flate2::{Decompress, FlushDecompress, Status};

/// Why a read failed: `Bad` is what callers catch (`BadZipFile`,
/// `RuntimeError`, `OSError`) and `Unhandled` escapes them (a 500).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ZipErr {
    Bad(String),
    Unhandled(String),
}

fn bad<T>(msg: impl Into<String>) -> Result<T, ZipErr> {
    Err(ZipErr::Bad(msg.into()))
}

#[derive(Clone, Debug)]
pub struct Info {
    pub orig_filename: String,
    pub filename: String,
    pub flag_bits: u16,
    pub compress_type: u16,
    pub crc: u32,
    pub compress_size: u64,
    pub file_size: u64,
    pub header_offset: i128,
    pub external_attr: u32,
    end_offset: i128,
}

impl Info {
    /// `ZipInfo.is_dir()` on Linux.
    pub fn is_dir(&self) -> bool {
        self.filename.ends_with('/')
    }

    /// `repr(zinfo)`.
    pub fn repr(&self) -> String {
        let mut out = format!("<ZipInfo filename={}", crate::pystr::repr(&self.filename));
        if self.compress_type != 0 {
            match compressor_name(self.compress_type) {
                Some(n) => out.push_str(&format!(" compress_type={n}")),
                None => out.push_str(&format!(" compress_type={}", self.compress_type)),
            }
        }
        let (hi, lo) = (self.external_attr >> 16, self.external_attr & 0xFFFF);
        if hi != 0 {
            out.push_str(&format!(" filemode='{}'", filemode(hi)));
        }
        if lo != 0 {
            out.push_str(&format!(" external_attr={lo:#x}"));
        }
        let dir = self.is_dir();
        if !dir || self.file_size != 0 {
            out.push_str(&format!(" file_size={}", self.file_size));
        }
        if (!dir || self.compress_size != 0) && (self.compress_type != 0 || self.file_size != self.compress_size) {
            out.push_str(&format!(" compress_size={}", self.compress_size));
        }
        out.push('>');
        out
    }
}

fn compressor_name(t: u16) -> Option<&'static str> {
    Some(match t {
        0 => "store",
        1 => "shrink",
        2..=5 => "reduce",
        6 | 10 => "implode",
        7 => "tokenize",
        8 => "deflate",
        9 => "deflate64",
        12 => "bzip2",
        14 => "lzma",
        18 => "terse",
        19 => "lz77",
        97 => "wavpack",
        98 => "ppmd",
        _ => return None,
    })
}

/// `stat.filemode(mode)` (the C `_stat` version).
pub fn filemode(mode: u32) -> String {
    let kind = match mode & 0o170000 {
        0o100000 => '-',
        0o040000 => 'd',
        0o120000 => 'l',
        0o060000 => 'b',
        0o020000 => 'c',
        0o010000 => 'p',
        0o140000 => 's',
        _ => '?',
    };
    let bit = |b: u32, c: char| if mode & b != 0 { c } else { '-' };
    let x = |exec: u32, special: u32, on: char, off: char| match (mode & special != 0, mode & exec != 0) {
        (true, true) => on,
        (true, false) => off,
        (false, true) => 'x',
        (false, false) => '-',
    };
    [
        kind,
        bit(0o400, 'r'),
        bit(0o200, 'w'),
        x(0o100, 0o4000, 's', 'S'),
        bit(0o040, 'r'),
        bit(0o020, 'w'),
        x(0o010, 0o2000, 's', 'S'),
        bit(0o004, 'r'),
        bit(0o002, 'w'),
        x(0o001, 0o1000, 't', 'T'),
    ]
    .iter()
    .collect()
}

const CP437_HIGH: &str = "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜ¢£¥₧ƒáíóúñÑªº¿⌐¬½¼¡«»░▒▓│┤╡╢╖╕╣║╗╝╜╛┐└┴┬├─┼╞╟╚╔╩╦╠═╬╧╨╤╥╙╘╒╓╫╪┘┌█▄▌▐▀αßΓπΣσµτΦΘΩδ∞φε∩≡±≥≤⌠⌡÷≈°∙·√ⁿ²■\u{a0}";

/// `bytes.decode("cp437")`.
pub fn cp437(b: &[u8]) -> String {
    let high: Vec<char> = CP437_HIGH.chars().collect();
    b.iter().map(|&c| if c < 0x80 { char::from(c) } else { high[usize::from(c - 0x80)] }).collect()
}

/// `repr(bytes)`.
pub fn bytes_repr(b: &[u8]) -> String {
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
                out.push(char::from(c));
            }
            0x20..=0x7e => out.push(char::from(c)),
            c => out.push_str(&format!("\\x{c:02x}")),
        }
    }
    out.push(quote);
    out
}

/// `_sanitize_filename` on Linux: cut at the first NUL.
fn sanitize(name: &str) -> String {
    name.split('\0').next().unwrap_or_default().to_owned()
}

fn decode_name(raw: &[u8], utf8: bool) -> Result<String, ZipErr> {
    if utf8 {
        String::from_utf8(raw.to_vec()).map_err(|_| ZipErr::Unhandled("UnicodeDecodeError".into()))
    } else {
        Ok(cp437(raw))
    }
}

/// `BytesIO` reads: past the end is short or empty, never an error.
fn slice(d: &[u8], pos: i128, n: i128) -> &[u8] {
    let len = d.len() as i128;
    let start = pos.clamp(0, len);
    let end = (pos + n).clamp(start, len);
    &d[start as usize..end as usize]
}

fn crc32(b: &[u8]) -> u32 {
    let mut c = flate2::Crc::new();
    c.update(b);
    c.sum()
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap_or_default())
}

struct EndRec {
    size: i128,
    offset: i128,
    location: i128,
}

const EOCD: &[u8] = b"PK\x05\x06";

/// `_EndRecData`.
fn end_rec(d: &[u8]) -> Result<Option<EndRec>, ZipErr> {
    let filesize = d.len() as i128;
    if d.len() >= 22 {
        let data = &d[d.len() - 22..];
        if &data[..4] == EOCD && data[20..] == [0, 0] {
            let rec =
                EndRec { size: u32_at(data, 12).into(), offset: u32_at(data, 16).into(), location: filesize - 22 };
            return end_rec64(d, filesize - 22, rec).map(Some);
        }
    }
    let max_start = (filesize - 65535 - 22).max(0);
    let data = &d[max_start as usize..];
    let Some(start) = data.windows(4).rposition(|w| w == EOCD) else { return Ok(None) };
    let rec = &data[start..];
    if rec.len() < 22 {
        return Ok(None);
    }
    let location = max_start + start as i128;
    let end = EndRec { size: u32_at(rec, 12).into(), offset: u32_at(rec, 16).into(), location };
    end_rec64(d, location, end).map(Some)
}

/// `_EndRecData64`.
fn end_rec64(d: &[u8], offset: i128, mut rec: EndRec) -> Result<EndRec, ZipErr> {
    let mut offset = offset - 20;
    if offset < 0 {
        return Ok(rec);
    }
    let loc = slice(d, offset, 20);
    if loc.len() != 20 {
        return bad("File is not a zip file");
    }
    if &loc[..4] != b"PK\x06\x07" {
        return Ok(rec);
    }
    let (diskno, reloff, disks) = (u32_at(loc, 4), i128::from(u64_at(loc, 8)), u32_at(loc, 16));
    if diskno != 0 || disks > 1 {
        return bad("zipfiles that span multiple disks are not supported");
    }
    offset -= 56;
    if reloff > offset {
        return bad("Corrupt zip64 end of central directory locator");
    }
    let mut extrasz = offset - reloff;
    let mut data = slice(d, reloff, 56);
    if data.len() != 56 {
        return bad("File is not a zip file");
    }
    if !data.starts_with(b"PK\x06\x06") && reloff != offset {
        extrasz = 0;
        data = slice(d, offset, 56);
        if data.len() != 56 {
            return bad("File is not a zip file");
        }
    }
    if !data.starts_with(b"PK\x06\x06") {
        return bad("Zip64 end of central directory record not found");
    }
    let sz = i128::from(u64_at(data, 4));
    let (dirsize, diroffset) = (i128::from(u64_at(data, 40)), i128::from(u64_at(data, 48)));
    if diroffset + dirsize != reloff || sz + 12 != 56 + extrasz {
        return bad("Corrupt zip64 end of central directory record");
    }
    rec.size = dirsize;
    rec.offset = diroffset;
    rec.location = offset - extrasz;
    Ok(rec)
}

/// `ZipInfo._decodeExtra`.
fn decode_extra(info: &mut Info, mut extra: &[u8], name_crc: u32) -> Result<(), ZipErr> {
    while extra.len() >= 4 {
        let (tp, ln) = (u16_at(extra, 0), usize::from(u16_at(extra, 2)));
        if ln + 4 > extra.len() {
            return bad(format!("Corrupt extra field {tp:04x} (size={ln})"));
        }
        let mut data = &extra[4..ln + 4];
        if tp == 0x0001 {
            let mut take = |field: &str| -> Result<u64, ZipErr> {
                if data.len() < 8 {
                    return bad(format!("Corrupt zip64 extra field. {field} not found."));
                }
                let v = u64_at(data, 0);
                data = &data[8..];
                Ok(v)
            };
            if info.file_size == 0xFFFF_FFFF_FFFF_FFFF || info.file_size == 0xFFFF_FFFF {
                info.file_size = take("File size")?;
            }
            if info.compress_size == 0xFFFF_FFFF {
                info.compress_size = take("Compress size")?;
            }
            if info.header_offset == 0xFFFF_FFFF {
                info.header_offset = take("Header offset")?.into();
            }
        } else if tp == 0x7075 {
            if data.len() < 5 {
                return bad("Corrupt unicode path extra field (0x7075)");
            }
            if data[0] == 1 && u32_at(data, 1) == name_crc {
                match std::str::from_utf8(&data[5..]) {
                    Ok("") => {}
                    Ok(s) => info.filename = sanitize(s),
                    Err(_) => return bad("Corrupt unicode path extra field (0x7075): invalid utf-8 bytes"),
                }
            }
        }
        extra = &extra[ln + 4..];
    }
    Ok(())
}

/// `ZipFile(BytesIO(data)).infolist()`.
pub fn open(d: &[u8]) -> Result<Vec<Info>, ZipErr> {
    let Some(rec) = end_rec(d)? else { return bad("File is not a zip file") };
    let concat = rec.location - rec.size - rec.offset;
    let start_dir = rec.offset + concat;
    if start_dir < 0 {
        return bad("Bad offset for central directory");
    }
    let cd = slice(d, start_dir, rec.size);
    let mut pos = 0usize;
    let mut read = |n: usize| -> &[u8] {
        let s = &cd[pos.min(cd.len())..(pos + n).min(cd.len())];
        pos += s.len();
        s
    };
    let mut out = Vec::new();
    let mut total: i128 = 0;
    while total < rec.size {
        let c = read(46);
        if c.len() != 46 {
            return bad("Truncated central directory");
        }
        if &c[..4] != b"PK\x01\x02" {
            return bad("Bad magic number for central directory");
        }
        let c = c.to_vec();
        let (fnlen, extlen, comlen) =
            (usize::from(u16_at(&c, 28)), usize::from(u16_at(&c, 30)), usize::from(u16_at(&c, 32)));
        let raw = read(fnlen).to_vec();
        let flags = u16_at(&c, 8);
        let name = decode_name(&raw, flags & 0x800 != 0)?;
        let extra = read(extlen).to_vec();
        read(comlen);
        let extract_version = c[6];
        let mut info = Info {
            filename: sanitize(&name),
            orig_filename: name,
            flag_bits: flags,
            compress_type: u16_at(&c, 10),
            crc: u32_at(&c, 16),
            compress_size: u32_at(&c, 20).into(),
            file_size: u32_at(&c, 24).into(),
            header_offset: u32_at(&c, 42).into(),
            external_attr: u32_at(&c, 38),
            end_offset: 0,
        };
        if extract_version > 63 {
            return bad(format!("zip file version {:.1}", f64::from(extract_version) / 10.0));
        }
        decode_extra(&mut info, &extra, crc32(&raw))?;
        info.header_offset += concat;
        out.push(info);
        total += 46 + (fnlen + extlen + comlen) as i128;
    }
    let mut order: Vec<usize> = (0..out.len()).collect();
    order.sort_by_key(|&i| out[i].header_offset);
    let mut end = start_dir;
    for &i in order.iter().rev() {
        out[i].end_offset = end;
        end = out[i].header_offset;
    }
    Ok(out)
}

/// `zf.read(info)`.
pub fn read(d: &[u8], info: &Info) -> Result<Vec<u8>, ZipErr> {
    let mut pos = info.header_offset;
    if pos < 0 {
        return Err(ZipErr::Unhandled(format!("negative seek value {pos}")));
    }
    let h = slice(d, pos, 30);
    if h.len() != 30 {
        return bad("Truncated file header");
    }
    if &h[..4] != b"PK\x03\x04" {
        return bad("Bad magic number for file header");
    }
    let (lflags, fnlen, extlen) = (u16_at(h, 6), i128::from(u16_at(h, 26)), i128::from(u16_at(h, 28)));
    pos += 30;
    let fname = slice(d, pos, fnlen);
    pos += fname.len() as i128 + extlen;
    if info.flag_bits & 0x20 != 0 {
        return bad("compressed patched data (flag bit 5)");
    }
    if info.flag_bits & 0x40 != 0 {
        return bad("strong encryption (flag bit 6)");
    }
    let fname_str = decode_name(fname, lflags & 0x800 != 0)?;
    if fname_str != info.orig_filename {
        return bad(format!(
            "File name in directory {} and header {} differ.",
            crate::pystr::repr(&info.orig_filename),
            bytes_repr(fname)
        ));
    }
    if pos + i128::from(info.compress_size) > info.end_offset && info.end_offset != info.header_offset {
        return bad(format!("Overlapped entries: {} (possible zip bomb)", crate::pystr::repr(&info.orig_filename)));
    }
    if info.flag_bits & 1 != 0 {
        return bad(format!("File {} is encrypted, password required for extraction", info.repr()));
    }
    let body = slice(d, pos, i128::from(info.compress_size));
    let short = (body.len() as u64) < info.compress_size;
    let eof_error = || Err(ZipErr::Unhandled("EOFError".into()));
    let data = match info.compress_type {
        0 => {
            if info.compress_size > 0 && body.is_empty() {
                return eof_error();
            }
            let n = (body.len() as u64).min(info.file_size) as usize;
            if short && (n as u64) < info.file_size {
                return eof_error();
            }
            body[..n].to_vec()
        }
        8 => {
            if info.compress_size > 0 && body.is_empty() {
                return eof_error();
            }
            let cap = info.file_size.saturating_add(1 << 16).min(usize::MAX as u64) as usize;
            let (mut out, ended) = inflate_raw(body, cap)?;
            if (out.len() as u64) < info.file_size && !ended && short {
                return eof_error();
            }
            out.truncate(info.file_size.min(out.len() as u64) as usize);
            out
        }
        12 | 14 => {
            if info.compress_size > 0 && body.is_empty() {
                return eof_error();
            }
            let cap = info.file_size.saturating_add(1 << 16).min(usize::MAX as u64) as usize;
            let (mut out, ended) = if info.compress_type == 12 { bunzip(body, cap)? } else { unlzma(body, cap)? };
            if (out.len() as u64) < info.file_size && !ended && short {
                return eof_error();
            }
            out.truncate(info.file_size.min(out.len() as u64) as usize);
            out
        }
        _ => return bad("That compression method is not supported"),
    };
    if crc32(&data) != info.crc {
        return bad(format!("Bad CRC-32 for file {}", crate::pystr::repr(&info.filename)));
    }
    Ok(data)
}

/// `decompressobj(-15).decompress(data)` up to `cap` bytes: the output and
/// whether the stream ended.
fn inflate_raw(input: &[u8], cap: usize) -> Result<(Vec<u8>, bool), ZipErr> {
    let mut d = Decompress::new(false);
    let mut out = Vec::with_capacity(input.len().saturating_mul(3).clamp(64, cap.max(64)));
    loop {
        if out.len() >= cap {
            return Ok((out, false));
        }
        if out.len() == out.capacity() {
            out.reserve((out.len()).clamp(4096, cap - out.len()));
        }
        let (in0, out0) = (d.total_in(), d.total_out());
        let status = d
            .decompress_vec(&input[in0 as usize..], &mut out, FlushDecompress::None)
            .map_err(|e| ZipErr::Unhandled(format!("zlib.error: {e}")))?;
        if status == Status::StreamEnd {
            return Ok((out, true));
        }
        if d.total_in() == in0 && d.total_out() == out0 && out.len() < out.capacity() {
            return Ok((out, false));
        }
    }
}

/// `bz2.BZ2Decompressor().decompress(data)` up to `cap` bytes.
fn bunzip(input: &[u8], cap: usize) -> Result<(Vec<u8>, bool), ZipErr> {
    let mut d = bzip2::Decompress::new(false);
    let mut out = Vec::with_capacity(input.len().saturating_mul(3).clamp(64, cap.max(64)));
    loop {
        if out.len() >= cap {
            return Ok((out, false));
        }
        if out.len() == out.capacity() {
            out.reserve(out.len().clamp(4096, cap - out.len()));
        }
        let (in0, out0) = (d.total_in(), d.total_out());
        match d.decompress_vec(&input[in0 as usize..], &mut out) {
            Ok(bzip2::Status::StreamEnd) => return Ok((out, true)),
            Ok(bzip2::Status::MemNeeded) => return Err(ZipErr::Unhandled("MemoryError".into())),
            Ok(_) => {}
            Err(bzip2::Error::Data | bzip2::Error::DataMagic) => return bad("Invalid data stream"),
            Err(e) => return Err(ZipErr::Unhandled(format!("bz2: {e}"))),
        }
        if d.total_in() == in0 && d.total_out() == out0 && out.len() < out.capacity() {
            return Ok((out, false));
        }
    }
}

/// `zipfile.LZMADecompressor().decompress(data)` up to `cap` bytes. Every
/// liblzma failure is an `LZMAError`, which no caller catches.
fn unlzma(input: &[u8], cap: usize) -> Result<(Vec<u8>, bool), ZipErr> {
    let fail = || ZipErr::Unhandled("LZMAError".into());
    if input.len() <= 4 {
        return Ok((Vec::new(), false));
    }
    let psize = usize::from(u16_at(input, 2));
    if input.len() <= 4 + psize {
        return Ok((Vec::new(), false));
    }
    let props = input[4];
    if psize != 5 || props > (4 * 5 + 4) * 9 + 8 || props % 9 + (props / 9) % 5 > 4 {
        return Err(fail());
    }
    let mut s = lzma_rust2::LzmaStream::new_with_props(u64::MAX, props, u32_at(input, 5), None).map_err(|_| fail())?;
    let data = &input[9..];
    if data.first().is_some_and(|&b| b != 0) {
        return Err(fail());
    }
    let mut out = Vec::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut pos = 0;
    while out.len() < cap {
        let r = s.process(&data[pos..], &mut buf, lzma_rust2::Action::Run).map_err(|_| fail())?;
        pos += r.bytes_consumed;
        out.extend_from_slice(&buf[..r.bytes_produced]);
        if r.status == lzma_rust2::Status::StreamEnd {
            return Ok((out, true));
        }
        if r.bytes_consumed == 0 && r.bytes_produced == 0 {
            break;
        }
    }
    Ok((out, false))
}
