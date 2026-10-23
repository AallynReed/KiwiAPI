//! Wwise media (`.wem`) to browser audio, mirroring `app/trove/audio/{wem,wwise_vorbis,ogg}.py`.
//!
//! [`convert_detailed`] fails the way the Python reference does: a `WemError`
//! with its exact text, or a [`ConvertError::Crash`] where Python raises
//! something else. [`convert`] is the same as an `Option` (None wherever
//! Python raises), and [`parse`] is `wem.parse`.

const FMT_PCM: u16 = 0xFFFE;
const FMT_ADPCM: u16 = 0x0002;
const FMT_VORBIS: u16 = 0xFFFF;
const VORBIS_FMT_SIZE: usize = 0x42;
const VORB_AT: usize = 0x18;
const SETUP_OFFSET_AT: usize = 0x10;
const FIRST_PACKET_AT: usize = 0x14;
const BLOCKSIZE_AT: usize = 0x28;
const ADPCM_BLOCK: usize = 0x24;
const ADPCM_HEADER: usize = 4;
const MAX_DECODED_BYTES: usize = 96 * 1024 * 1024;

/// Why a conversion failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConvertError {
    /// `WemError(text)` (a `VorbisError` from the setup rebuild included).
    Wem(String),
    /// Any other exception Python raises on the way.
    Crash(&'static str),
}

type R<T> = Result<T, ConvertError>;

fn wem<T>(msg: impl Into<String>) -> R<T> {
    Err(ConvertError::Wem(msg.into()))
}

const CRASH: ConvertError = ConvertError::Crash("unhandled");

#[inline]
fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}

#[inline]
fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

/// Python slicing: clamp both ends, empty when reversed.
#[inline]
fn slice(b: &[u8], start: usize, end: usize) -> &[u8] {
    let end = end.min(b.len());
    if start >= end { &[] } else { &b[start..end] }
}

fn ilog(mut x: i64) -> u32 {
    let mut n = 0;
    while x > 0 {
        n += 1;
        x >>= 1;
    }
    n
}

// ─── bits ─────────────────────────────────────────────────────────────────

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        BitReader { data, pos: 0 }
    }

    fn read(&mut self, n: u32) -> R<u64> {
        let mut v = 0u64;
        for i in 0..n {
            let Some(&byte) = self.data.get(self.pos >> 3) else {
                return wem("ran off the end of a packet");
            };
            v |= u64::from((byte >> (self.pos & 7)) & 1) << i;
            self.pos += 1;
        }
        Ok(v)
    }
}

#[derive(Default)]
struct BitWriter {
    out: Vec<u8>,
    acc: u64,
    nbits: u32,
}

impl BitWriter {
    #[inline]
    fn write(&mut self, value: u64, n: u32) {
        let mask = if n >= 64 { u64::MAX } else { (1u64 << n) - 1 };
        self.acc |= (value & mask) << self.nbits;
        self.nbits += n;
        while self.nbits >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.nbits -= 8;
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.nbits > 0 {
            self.out.push(self.acc as u8);
        }
        self.out
    }
}

// ─── codebooks ────────────────────────────────────────────────────────────

struct Codebooks<'a> {
    data: &'a [u8],
    table: usize,
    count: usize,
}

impl<'a> Codebooks<'a> {
    fn new(raw: &'a [u8]) -> R<Self> {
        if raw.len() < 4 {
            return wem("codebook library is truncated");
        }
        let table = u32_at(raw, raw.len() - 4).ok_or(CRASH)? as usize;
        if table > raw.len() - 4 || !(raw.len() - 4 - table).is_multiple_of(4) {
            return wem("codebook library has a bad offset table");
        }
        Ok(Codebooks { data: raw, table, count: (raw.len() - table) / 4 })
    }

    fn offset(&self, i: usize) -> usize {
        u32_at(self.data, self.table + 4 * i).unwrap_or(0) as usize
    }

    fn expand(&self, cb_id: u64, out: &mut BitWriter) -> R<()> {
        let id = cb_id as usize;
        if id >= self.count.saturating_sub(1) {
            return wem(format!("codebook {cb_id} is not in the library"));
        }
        let mut bits = BitReader::new(slice(self.data, self.offset(id), self.offset(id + 1)));
        let dimensions = bits.read(4)?;
        let entries = bits.read(14)?;
        out.write(0x564342, 24);
        out.write(dimensions, 16);
        out.write(entries, 24);

        let ordered = bits.read(1)?;
        out.write(ordered, 1);
        if ordered == 1 {
            out.write(bits.read(5)?, 5);
            let mut current = 0u64;
            while current < entries {
                let w = ilog((entries - current) as i64);
                let n = bits.read(w)?;
                out.write(n, w);
                current += n;
            }
            if current > entries {
                return wem("ordered codebook overruns its entry count");
            }
        } else {
            let length_bits = bits.read(3)? as u32;
            let sparse = bits.read(1)?;
            if !(1..=5).contains(&length_bits) {
                return wem(format!("bad codeword length width {length_bits}"));
            }
            out.write(sparse, 1);
            for _ in 0..entries {
                let mut present = true;
                if sparse == 1 {
                    present = bits.read(1)? == 1;
                    out.write(u64::from(present), 1);
                }
                if present {
                    out.write(bits.read(length_bits)?, 5);
                }
            }
        }

        let lookup = bits.read(1)?;
        out.write(lookup, 4);
        if lookup == 1 {
            out.write(bits.read(32)?, 32);
            out.write(bits.read(32)?, 32);
            let value_bits = bits.read(4)? as u32;
            out.write(u64::from(value_bits), 4);
            out.write(bits.read(1)?, 1);
            for _ in 0..quantvals(entries as i64, dimensions as i64)? {
                out.write(bits.read(value_bits + 1)?, value_bits + 1);
            }
        }
        Ok(())
    }
}

/// libvorbis `_book_maptype1_quantvals`: Python's `VorbisError` for no
/// dimensions, its `ValueError` (a negative shift) as a crash.
fn quantvals(entries: i64, dimensions: i64) -> R<i64> {
    if dimensions <= 0 {
        return wem("codebook has no dimensions");
    }
    let bits = i64::from(ilog(entries));
    let num = (bits - 1) * (dimensions - 1);
    let shift = num.div_euclid(dimensions);
    if shift < 0 {
        return Err(ConvertError::Crash("negative shift count"));
    }
    let mut vals = if shift >= 63 { 0 } else { entries >> shift };
    let pow = |v: i64| -> i128 {
        let mut acc: i128 = 1;
        for _ in 0..dimensions {
            acc = acc.saturating_mul(i128::from(v));
        }
        acc
    };
    loop {
        let (acc, acc1) = (pow(vals), pow(vals + 1));
        let e = i128::from(entries);
        if acc <= e && e < acc1 {
            return Ok(vals);
        }
        vals += if acc > e { -1 } else { 1 };
    }
}

// ─── setup header ─────────────────────────────────────────────────────────

struct Setup {
    body: Vec<u8>,
    blockflags: Vec<u8>,
    mode_bits: u32,
}

fn rebuild_setup(packed: &[u8], channels: i64, lib: &Codebooks) -> R<Setup> {
    let mut bits = BitReader::new(packed);
    let mut out = BitWriter::default();

    let codebook_count = bits.read(8)? + 1;
    out.write(codebook_count - 1, 8);
    for _ in 0..codebook_count {
        let id = bits.read(10)?;
        lib.expand(id, &mut out)?;
    }
    out.write(0, 6);
    out.write(0, 16);

    let floor_count = bits.read(6)? + 1;
    out.write(floor_count - 1, 6);
    for _ in 0..floor_count {
        out.write(1, 16);
        let partitions = bits.read(5)?;
        out.write(partitions, 5);
        let mut classes = Vec::with_capacity(partitions as usize);
        for _ in 0..partitions {
            let c = bits.read(4)?;
            classes.push(c);
        }
        for &c in &classes {
            out.write(c, 4);
        }
        let mut dimensions = Vec::new();
        let n_classes = classes.iter().max().map_or(0, |m| m + 1);
        for _ in 0..n_classes {
            let dim = bits.read(3)?;
            out.write(dim, 3);
            let subclasses = bits.read(2)?;
            out.write(subclasses, 2);
            if subclasses != 0 {
                out.write(bits.read(8)?, 8);
            }
            for _ in 0..(1u64 << subclasses) {
                out.write(bits.read(8)?, 8);
            }
            dimensions.push(dim + 1);
        }
        out.write(bits.read(2)?, 2);
        let rangebits = bits.read(4)? as u32;
        out.write(u64::from(rangebits), 4);
        for &c in &classes {
            for _ in 0..*dimensions.get(c as usize).ok_or(CRASH)? {
                out.write(bits.read(rangebits)?, rangebits);
            }
        }
    }

    let residue_count = bits.read(6)? + 1;
    out.write(residue_count - 1, 6);
    for _ in 0..residue_count {
        let residue_type = bits.read(2)?;
        if residue_type > 2 {
            return wem(format!("residue type {residue_type} is not valid"));
        }
        out.write(residue_type, 16);
        out.write(bits.read(24)?, 24);
        out.write(bits.read(24)?, 24);
        out.write(bits.read(24)?, 24);
        let classifications = bits.read(6)? + 1;
        out.write(classifications - 1, 6);
        out.write(bits.read(8)?, 8);
        let mut cascade = Vec::with_capacity(classifications as usize);
        for _ in 0..classifications {
            let low = bits.read(3)?;
            out.write(low, 3);
            let flag = bits.read(1)?;
            out.write(flag, 1);
            let mut high = 0;
            if flag == 1 {
                high = bits.read(5)?;
                out.write(high, 5);
            }
            cascade.push(low | (high << 3));
        }
        for mask in cascade {
            for bit in 0..8 {
                if mask & (1 << bit) != 0 {
                    out.write(bits.read(8)?, 8);
                }
            }
        }
    }

    let mapping_count = bits.read(6)? + 1;
    out.write(mapping_count - 1, 6);
    for _ in 0..mapping_count {
        out.write(0, 16);
        let submap_flag = bits.read(1)?;
        out.write(submap_flag, 1);
        let mut submaps = 1;
        if submap_flag == 1 {
            submaps = bits.read(4)? + 1;
            out.write(submaps - 1, 4);
        }
        let coupled = bits.read(1)?;
        out.write(coupled, 1);
        if coupled == 1 {
            let steps = bits.read(8)? + 1;
            out.write(steps - 1, 8);
            let width = ilog(channels - 1);
            for _ in 0..steps {
                out.write(bits.read(width)?, width);
                out.write(bits.read(width)?, width);
            }
        }
        if bits.read(2)? != 0 {
            return wem("mapping reserved field is not zero");
        }
        out.write(0, 2);
        if submaps > 1 {
            for _ in 0..channels {
                out.write(bits.read(4)?, 4);
            }
        }
        for _ in 0..submaps {
            out.write(bits.read(8)?, 8);
            out.write(bits.read(8)?, 8);
            out.write(bits.read(8)?, 8);
        }
    }

    let mode_count = bits.read(6)? + 1;
    out.write(mode_count - 1, 6);
    let mut blockflags = Vec::with_capacity(mode_count as usize);
    for _ in 0..mode_count {
        let blockflag = bits.read(1)?;
        out.write(blockflag, 1);
        blockflags.push(blockflag as u8);
        out.write(0, 16);
        out.write(0, 16);
        out.write(bits.read(8)?, 8);
    }
    out.write(1, 1);
    Ok(Setup { body: out.finish(), blockflags, mode_bits: ilog(mode_count as i64 - 1) })
}

// ─── ogg ──────────────────────────────────────────────────────────────────

const CRC_TABLE: [u32; 256] = {
    let mut t = [0u32; 256];
    let mut n = 0;
    while n < 256 {
        let mut c = (n as u32) << 24;
        let mut k = 0;
        while k < 8 {
            c = if c & 0x8000_0000 != 0 { (c << 1) ^ 0x04C1_1DB7 } else { c << 1 };
            k += 1;
        }
        t[n] = c;
        n += 1;
    }
    t
};

fn crc_update(mut crc: u32, data: &[u8]) -> u32 {
    for &b in data {
        crc = (crc << 8) ^ CRC_TABLE[(((crc >> 24) as u8) ^ b) as usize];
    }
    crc
}

struct Page {
    segments: Vec<u8>,
    body: Vec<u8>,
    granule: i64,
    continued: bool,
}

struct OggWriter {
    serial: u32,
    pages: Vec<Page>,
    queue: Vec<(Vec<u8>, i64)>,
}

impl OggWriter {
    fn new(serial: u32) -> Self {
        OggWriter { serial, pages: Vec::new(), queue: Vec::new() }
    }

    fn write(&mut self, packet: Vec<u8>, granule: i64) {
        self.queue.push((packet, granule));
    }

    fn flush(&mut self) {
        if self.queue.is_empty() {
            return;
        }
        let mut page = Page { segments: Vec::new(), body: Vec::new(), granule: -1, continued: false };
        for (packet, pkt_granule) in std::mem::take(&mut self.queue) {
            let full = packet.len() / 255;
            for i in 0..=full {
                let length = if i < full { 255 } else { packet.len() % 255 };
                if page.segments.len() == 255 {
                    let next = Page { segments: Vec::new(), body: Vec::new(), granule: -1, continued: i > 0 };
                    self.pages.push(std::mem::replace(&mut page, next));
                }
                page.segments.push(length as u8);
                let start = i * 255;
                page.body.extend_from_slice(&packet[start..start + length]);
            }
            page.granule = pkt_granule;
        }
        self.pages.push(page);
    }

    fn finish(mut self) -> Vec<u8> {
        self.flush();
        let total: usize = self.pages.iter().map(|p| 27 + p.segments.len() + p.body.len()).sum();
        let mut out = Vec::with_capacity(total);
        let last = self.pages.len().saturating_sub(1);
        for (number, p) in self.pages.iter().enumerate() {
            let flags = u8::from(p.continued) | if number == 0 { 2 } else { 0 } | if number == last { 4 } else { 0 };
            let start = out.len();
            out.extend_from_slice(b"OggS");
            out.push(0);
            out.push(flags);
            out.extend_from_slice(&p.granule.to_le_bytes());
            out.extend_from_slice(&self.serial.to_le_bytes());
            out.extend_from_slice(&(number as u32).to_le_bytes());
            out.extend_from_slice(&[0, 0, 0, 0]);
            out.push(p.segments.len() as u8);
            out.extend_from_slice(&p.segments);
            out.extend_from_slice(&p.body);
            let crc = crc_update(0, &out[start..]);
            out[start + 22..start + 26].copy_from_slice(&crc.to_le_bytes());
        }
        out
    }
}

// ─── wem ──────────────────────────────────────────────────────────────────

/// `wem.WemInfo`, plus where the fmt chunk sits.
#[derive(Clone, Debug, PartialEq)]
pub struct Info {
    pub codec: u16,
    pub channels: i64,
    pub rate: u32,
    pub avg: u32,
    pub align: u16,
    pub bits: u16,
    pub samples: i64,
    pub fmt_off: usize,
    pub data_off: usize,
    pub data_size: usize,
}

impl Info {
    /// `CODEC_NAMES[codec]`.
    pub fn codec_name(&self) -> &'static str {
        match self.codec {
            FMT_VORBIS => "vorbis",
            FMT_ADPCM => "adpcm",
            _ => "pcm",
        }
    }

    /// `samples / rate`.
    pub fn duration(&self) -> f64 {
        if self.rate == 0 { 0.0 } else { self.samples as f64 / f64::from(self.rate) }
    }
}

/// `wem.parse`: header facts, or the `WemError` text.
pub fn parse(raw: &[u8]) -> Result<Info, String> {
    if raw.len() < 12 || &raw[..4] != b"RIFF" || &raw[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE container".into());
    }
    let (mut fmt, mut data) = (None, None);
    let mut off = 12usize;
    while off + 8 <= raw.len() {
        let size = u32_at(raw, off + 4).unwrap_or(0) as usize;
        if off + 8 + size > raw.len() {
            break;
        }
        match &raw[off..off + 4] {
            b"fmt " => fmt = Some((off + 8, size)),
            b"data" => data = Some((off + 8, size)),
            _ => {}
        }
        off += 8 + size + (size & 1);
    }
    let (Some((fmt_off, fmt_size)), Some((data_off, data_size))) = (fmt, data) else {
        return Err("missing fmt or data chunk".into());
    };
    if fmt_size < 16 {
        return Err("fmt chunk is too small".into());
    }
    let at16 = |o| u16_at(raw, fmt_off + o).unwrap_or(0);
    let at32 = |o| u32_at(raw, fmt_off + o).unwrap_or(0);
    let (codec, channels, rate, avg, align, bits) = (at16(0), i64::from(at16(2)), at32(4), at32(8), at16(12), at16(14));
    if ![FMT_PCM, FMT_ADPCM, FMT_VORBIS].contains(&codec) {
        return Err(format!("unsupported codec 0x{codec:04x}"));
    }
    if !(1..=8).contains(&channels) || !(1000..=192_000).contains(&rate) {
        return Err("implausible channel count or sample rate".into());
    }
    let samples = match codec {
        FMT_VORBIS => {
            if fmt_size != VORBIS_FMT_SIZE {
                return Err(format!("unsupported Vorbis header layout (fmt size {fmt_size})"));
            }
            i64::from(at32(VORB_AT))
        }
        FMT_ADPCM => {
            let per_block = ((ADPCM_BLOCK - ADPCM_HEADER) * 2) as i64;
            let blocks = if align != 0 { (data_size / align as usize) as i64 } else { 0 };
            blocks * per_block
        }
        _ => {
            let frame = channels as usize * usize::from((bits / 8).max(1));
            (data_size / frame) as i64
        }
    };
    Ok(Info { codec, channels, rate, avg, align, bits, samples, fmt_off, data_off, data_size })
}

fn wav(samples: &[u8], channels: i64, rate: u32) -> R<Vec<u8>> {
    let align = (channels * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&u32::try_from(36 + samples.len()).map_err(|_| CRASH)?.to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(channels as u16).to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&rate.checked_mul(align).ok_or(CRASH)?.to_le_bytes());
    out.extend_from_slice(&(align as u16).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(samples.len() as u32).to_le_bytes());
    out.extend_from_slice(samples);
    Ok(out)
}

const STEP_TABLE: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66, 73, 80, 88, 97, 107,
    118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449, 494, 544, 598, 658, 724, 796, 876, 963,
    1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272, 2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894,
    6484, 7132, 7845, 8630, 9493, 10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794,
    32767,
];
const INDEX_TABLE: [i32; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];

fn decode_adpcm(raw: &[u8], info: &Info) -> R<Vec<u8>> {
    let channels = info.channels as usize;
    let stride = ADPCM_BLOCK * channels;
    if info.align as usize != stride {
        return wem(format!("unexpected ADPCM block size {}", info.align));
    }
    let per_block = (ADPCM_BLOCK - ADPCM_HEADER) * 2;
    let blocks = info.data_size / stride;
    if blocks * per_block * channels * 2 > MAX_DECODED_BYTES {
        return wem("decoded audio would be too large");
    }
    let data = slice(raw, info.data_off, info.data_off + info.data_size);
    let mut out = vec![0u8; blocks * per_block * channels * 2];
    for block in 0..blocks {
        let base = block * stride;
        for channel in 0..channels {
            let offset = base + channel * ADPCM_BLOCK;
            let mut predictor = i32::from(i16::from_le_bytes([data[offset], data[offset + 1]]));
            let mut index = i32::from(i16::from_le_bytes([data[offset + 2], data[offset + 3]])).clamp(0, 88);
            let mut write = (block * per_block * channels + channel) * 2;
            for i in 0..per_block {
                let byte = data[offset + ADPCM_HEADER + (i >> 1)];
                let nibble = i32::from(if i & 1 == 0 { byte & 0x0F } else { byte >> 4 });
                let step = STEP_TABLE[index as usize];
                let mut delta = step >> 3;
                if nibble & 1 != 0 {
                    delta += step >> 2;
                }
                if nibble & 2 != 0 {
                    delta += step >> 1;
                }
                if nibble & 4 != 0 {
                    delta += step;
                }
                if nibble & 8 != 0 {
                    delta = -delta;
                }
                predictor = (predictor + delta).clamp(-32768, 32767);
                index = (index + INDEX_TABLE[(nibble & 7) as usize]).clamp(0, 88);
                out[write..write + 2].copy_from_slice(&(predictor as i16).to_le_bytes());
                write += channels * 2;
            }
        }
    }
    Ok(out)
}

fn restore_packet(packet: &[u8], mode_bits: u32, blockflag: u8, previous: u8, following: u8) -> R<Vec<u8>> {
    let mut bits = BitReader::new(packet);
    let mut out = BitWriter { out: Vec::with_capacity(packet.len() + 1), ..Default::default() };
    out.write(0, 1);
    out.write(bits.read(mode_bits).map_err(|_| CRASH)?, mode_bits);
    let rest = bits.read(8 - mode_bits).map_err(|_| CRASH)?;
    if blockflag != 0 {
        out.write(u64::from(previous), 1);
        out.write(u64::from(following), 1);
    }
    out.write(rest, 8 - mode_bits);
    for &b in &packet[1..] {
        out.write(u64::from(b), 8);
    }
    Ok(out.finish())
}

fn to_ogg(raw: &[u8], info: &Info, codebooks: Result<&[u8], &str>) -> R<Vec<u8>> {
    let vorb = info.fmt_off + VORB_AT;
    let setup_at = u32_at(raw, vorb + SETUP_OFFSET_AT).ok_or(CRASH)? as usize;
    let first_at = u32_at(raw, vorb + FIRST_PACKET_AT).ok_or(CRASH)? as usize;
    let short_pow = *raw.get(vorb + BLOCKSIZE_AT).ok_or(CRASH)?;
    let long_pow = *raw.get(vorb + BLOCKSIZE_AT + 1).ok_or(CRASH)?;
    if !(6 <= short_pow && short_pow <= long_pow && long_pow <= 13) {
        return wem(format!("implausible block sizes 2^{short_pow}/2^{long_pow}"));
    }
    if !(setup_at <= first_at && first_at <= info.data_size) {
        return wem("packet offsets fall outside the data chunk");
    }
    let size = u16_at(raw, info.data_off + setup_at).ok_or(CRASH)? as usize;
    let body_at = info.data_off + setup_at + 2;
    let lib = Codebooks::new(codebooks.map_err(|e| ConvertError::Wem(e.to_owned()))?)?;
    let setup = rebuild_setup(slice(raw, body_at, body_at + size), info.channels, &lib)?;

    let mut ogg = OggWriter::new(1);
    let mut ident = b"\x01vorbis".to_vec();
    ident.extend_from_slice(&0u32.to_le_bytes());
    ident.push(info.channels as u8);
    ident.extend_from_slice(&info.rate.to_le_bytes());
    ident.extend_from_slice(&0i32.to_le_bytes());
    ident.extend_from_slice(&i32::try_from(i64::from(info.avg) * 8).map_err(|_| CRASH)?.to_le_bytes());
    ident.extend_from_slice(&0i32.to_le_bytes());
    ident.push(short_pow | (long_pow << 4));
    ident.push(1);
    ogg.write(ident, 0);
    ogg.flush();
    let mut comment = b"\x03vorbis".to_vec();
    comment.extend_from_slice(&8u32.to_le_bytes());
    comment.extend_from_slice(b"Kiwi API");
    comment.extend_from_slice(&0u32.to_le_bytes());
    comment.push(1);
    ogg.write(comment, 0);
    let mut setup_packet = b"\x05vorbis".to_vec();
    setup_packet.extend_from_slice(&setup.body);
    ogg.write(setup_packet, 0);
    ogg.flush();

    let (start, end) = (info.data_off + first_at, info.data_off + info.data_size);
    let mut packets: Vec<&[u8]> = Vec::new();
    let mut off = start;
    while off + 2 <= end {
        let size = u16_at(raw, off).ok_or(CRASH)? as usize;
        off += 2;
        if size == 0 || off + size > end {
            break;
        }
        packets.push(&raw[off..off + size]);
        off += size;
    }
    if packets.is_empty() {
        return wem("stream carries no audio packets");
    }
    let blocksizes = [1i64 << short_pow, 1i64 << long_pow];
    let mask = (1u8 << setup.mode_bits).wrapping_sub(1);
    let modes: Vec<usize> = packets.iter().map(|p| (p[0] & mask) as usize).collect();
    let flag = |m: usize| setup.blockflags.get(m).copied().ok_or(CRASH);

    let mut granules = Vec::with_capacity(packets.len());
    let (mut position, mut previous) = (0i64, 0i64);
    for (i, &mode) in modes.iter().enumerate() {
        let current = blocksizes[flag(mode)? as usize];
        if i > 0 {
            position += (previous + current).div_euclid(4);
        }
        previous = current;
        granules.push(position);
    }
    let last = granules.len() - 1;
    if info.samples != 0 && info.samples <= granules[last] {
        granules[last] = info.samples;
    }

    if setup.mode_bits == 0 {
        for (i, p) in packets.iter().enumerate() {
            ogg.write(p.to_vec(), granules[i]);
        }
        return Ok(ogg.finish());
    }
    let mut previous_flag = 0u8;
    for (i, p) in packets.iter().enumerate() {
        let blockflag = flag(modes[i])?;
        let following = if i + 1 < modes.len() { flag(modes[i + 1])? } else { 0 };
        ogg.write(restore_packet(p, setup.mode_bits, blockflag, previous_flag, following)?, granules[i]);
        previous_flag = blockflag;
    }
    Ok(ogg.finish())
}

pub struct Converted {
    pub bytes: Vec<u8>,
    pub mime: &'static str,
    pub ext: &'static str,
}

/// `wem.convert`. `codebooks` is the packed library, or the text of
/// `VorbisError("codebook library is unavailable: ...")` when it couldn't be
/// read (only Vorbis input needs it).
pub fn convert_detailed(raw: &[u8], codebooks: Result<&[u8], &str>) -> Result<Converted, ConvertError> {
    let info = parse(raw).map_err(ConvertError::Wem)?;
    match info.codec {
        FMT_VORBIS => Ok(Converted { bytes: to_ogg(raw, &info, codebooks)?, mime: "audio/ogg", ext: "ogg" }),
        FMT_ADPCM => {
            let pcm = decode_adpcm(raw, &info)?;
            Ok(Converted { bytes: wav(&pcm, info.channels, info.rate)?, mime: "audio/wav", ext: "wav" })
        }
        _ => {
            if info.bits != 16 {
                return wem(format!("unsupported PCM depth {}", info.bits));
            }
            let pcm = slice(raw, info.data_off, info.data_off + info.data_size);
            Ok(Converted { bytes: wav(pcm, info.channels, info.rate)?, mime: "audio/wav", ext: "wav" })
        }
    }
}

/// `wem.convert`. `codebooks` is the packed library; None means it couldn't be
/// loaded, so Vorbis input returns None for the caller to report.
pub fn convert(raw: &[u8], codebooks: Option<&[u8]>) -> Option<Converted> {
    convert_detailed(raw, codebooks.ok_or("codebook library is unavailable")).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantvals_matches_libvorbis() {
        assert_eq!(quantvals(81, 4), Ok(3));
        assert_eq!(quantvals(256, 2), Ok(16));
        assert_eq!(quantvals(0, 1), Ok(0));
        assert_eq!(quantvals(0, 2), Err(ConvertError::Crash("negative shift count")));
        assert_eq!(quantvals(5, 0), Err(ConvertError::Wem("codebook has no dimensions".into())));
    }

    #[test]
    fn bit_round_trip() {
        let mut w = BitWriter::default();
        w.write(0b101, 3);
        w.write(0xABCD, 16);
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes);
        assert_eq!((r.read(3), r.read(16)), (Ok(0b101), Ok(0xABCD)));
        assert_eq!(r.read(16), Err(ConvertError::Wem("ran off the end of a packet".into())));
    }

    #[test]
    fn parse_errors_carry_python_text() {
        assert_eq!(parse(b"nope").unwrap_err(), "not a RIFF/WAVE container");
        let mut w = b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0".to_vec();
        w.extend_from_slice(&[0x01, 0, 1, 0, 0x44, 0xAC, 0, 0, 0, 0, 0, 0, 2, 0, 16, 0]);
        w.extend_from_slice(b"data\0\0\0\0");
        assert_eq!(parse(&w).unwrap_err(), "unsupported codec 0x0001");
    }
}
