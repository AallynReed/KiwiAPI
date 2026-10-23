//! Pillow 13's `DdsImagePlugin` for the uncompressed layouts, then
//! `convert("RGB")`: the top mip level as RGB bytes. Anything else is what
//! Pillow would fail to open, which the BRDF route answers as a 404.

const DDPF_ALPHAPIXELS: u32 = 0x1;
const DDPF_ALPHA: u32 = 0x2;
const DDPF_RGB: u32 = 0x40;
const DDPF_LUMINANCE: u32 = 0x20000;

fn u32_at(b: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(off..off + 4)?.try_into().ok()?))
}

/// `DdsRgbDecoder`: each mask scaled to 8 bits the way Pillow computes it.
fn channels(data: &[u8], w: usize, h: usize, bitcount: u32, masks: &[u32]) -> Option<Vec<u8>> {
    let bytes = (bitcount / 8) as usize;
    if bytes == 0 {
        return None;
    }
    let mut offsets = Vec::with_capacity(masks.len());
    let mut totals = Vec::with_capacity(masks.len());
    for &m in masks {
        let mut off = 0u32;
        if m != 0 {
            while off < 31 && (m >> (off + 1)) << (off + 1) == m {
                off += 1;
            }
        }
        offsets.push(off);
        totals.push(m >> off);
    }
    let want = w * h * masks.len();
    let mut out = Vec::with_capacity(want);
    let mut pos = 0usize;
    while out.len() < want {
        let chunk = data.get(pos..pos + bytes)?;
        pos += bytes;
        let mut value = 0u64;
        for (i, b) in chunk.iter().enumerate() {
            value |= u64::from(*b) << (8 * i);
        }
        for (k, &m) in masks.iter().enumerate() {
            let masked = (value & u64::from(m)) as u32;
            out.push(if totals[k] != 0 {
                (f64::from(masked >> offsets[k]) / f64::from(totals[k]) * 255.0) as i64 as u8
            } else {
                0
            });
        }
    }
    Some(out)
}

/// `Image.open(dds).convert("RGB")`: `(width, height, rgb)`.
pub fn to_rgb(dds: &[u8]) -> Option<(usize, usize, Vec<u8>)> {
    if dds.get(..4)? != b"DDS " || u32_at(dds, 4)? != 124 || dds.len() < 128 {
        return None;
    }
    let (h, w) = (u32_at(dds, 12)? as usize, u32_at(dds, 16)? as usize);
    let (pfflags, bitcount) = (u32_at(dds, 80)?, u32_at(dds, 88)?);
    let pixels = &dds[128..];
    let rgb = if pfflags & DDPF_RGB != 0 {
        let n = if pfflags & DDPF_ALPHAPIXELS != 0 { 4 } else { 3 };
        let masks: Vec<u32> = (0..n).map(|i| u32_at(dds, 92 + 4 * i)).collect::<Option<_>>()?;
        let raw = channels(pixels, w, h, bitcount, &masks)?;
        raw.chunks_exact(n).flat_map(|p| [p[0], p[1], p[2]]).collect()
    } else if pfflags & DDPF_LUMINANCE != 0 && bitcount == 8 {
        pixels.get(..w * h)?.iter().flat_map(|&l| [l, l, l]).collect()
    } else if pfflags & DDPF_ALPHA != 0 && bitcount == 8 {
        pixels.get(..w * h)?.iter().flat_map(|_| [0, 0, 0]).collect()
    } else {
        return None;
    };
    Some((w, h, rgb))
}
