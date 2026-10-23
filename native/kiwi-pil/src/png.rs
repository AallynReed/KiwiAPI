//! `img.save(fp, "PNG", optimize=True)` for an "RGB" or "RGBA" image: `ZipEncode.c`'s
//! filter choice, zlib level 9 / memLevel 9 / Z_FILTERED, IDAT chunks of
//! `ImageFile.MAXBLOCK` bytes. zlib-rs emits the same stream as zlib-ng 2.3.3
//! does in Pillow's Windows wheel.

const MAXBLOCK: usize = 65536;

fn cost(v: u8) -> i32 {
    if v < 128 { v as i32 } else { 256 - v as i32 }
}

/// The filtered scanlines (filter byte + row), as `ImagingZipEncode` with
/// `optimize` picks them.
pub fn filter_rows(rgb: &[u8], width: usize, height: usize) -> Vec<u8> {
    filter_rows_bpp(rgb, width, height, 3)
}

/// [`filter_rows`] for `bpp` bytes per pixel.
pub fn filter_rows_bpp(rgb: &[u8], width: usize, height: usize, bpp: usize) -> Vec<u8> {
    let bytes = width * bpp;
    let mut out = Vec::with_capacity((bytes + 1) * height);
    let mut previous = vec![0u8; bytes + 1];
    let mut buffer = vec![0u8; bytes + 1];
    let mut prior = vec![1u8; bytes + 1];
    let mut up = vec![2u8; bytes + 1];
    let mut average = vec![3u8; bytes + 1];
    let mut paeth = vec![4u8; bytes + 1];
    buffer[0] = 0;
    for y in 0..height {
        buffer[1..].copy_from_slice(&rgb[y * bytes..(y + 1) * bytes]);
        let mut choice = 0u8;
        let mut sum: i32 = buffer[1..].iter().map(|&v| cost(v)).sum();
        if sum > 0 {
            let mut s = 0;
            for i in 1..=bytes {
                let v = buffer[i].wrapping_sub(previous[i]);
                up[i] = v;
                s += cost(v);
            }
            if s < sum {
                choice = 2;
                sum = s;
            }
        }
        if sum > 0 {
            let mut s = 0;
            for i in 1..=bytes {
                let v = if i <= bpp { buffer[i] } else { buffer[i].wrapping_sub(buffer[i - bpp]) };
                prior[i] = v;
                s += cost(v);
            }
            if s < sum {
                choice = 1;
                sum = s;
            }
        }
        if sum > 0 {
            let mut s = 0;
            for i in 1..=bytes {
                let v = if i <= bpp {
                    buffer[i].wrapping_sub(previous[i] / 2)
                } else {
                    buffer[i].wrapping_sub(((buffer[i - bpp] as u32 + previous[i] as u32) / 2) as u8)
                };
                average[i] = v;
                s += cost(v);
            }
            if s < sum {
                choice = 3;
                sum = s;
            }
        }
        if sum > 0 {
            let mut s = 0;
            for i in 1..=bytes {
                let v = if i <= bpp {
                    buffer[i].wrapping_sub(previous[i])
                } else {
                    let a = buffer[i - bpp] as i32;
                    let b = previous[i] as i32;
                    let c = previous[i - bpp] as i32;
                    let pa = (b - c).abs();
                    let pb = (a - c).abs();
                    let pc = (a + b - 2 * c).abs();
                    let pred = if pa <= pb && pa <= pc {
                        a
                    } else if pb <= pc {
                        b
                    } else {
                        c
                    };
                    buffer[i].wrapping_sub(pred as u8)
                };
                paeth[i] = v;
                s += cost(v);
            }
            if s < sum {
                choice = 4;
            }
        }
        match choice {
            0 => out.extend_from_slice(&buffer),
            1 => out.extend_from_slice(&prior),
            2 => out.extend_from_slice(&up),
            3 => out.extend_from_slice(&average),
            _ => out.extend_from_slice(&paeth),
        }
        std::mem::swap(&mut buffer, &mut previous);
        buffer[0] = 0;
    }
    out
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut h = crc32fast::Hasher::new();
    h.update(kind);
    h.update(data);
    out.extend_from_slice(&h.finalize().to_be_bytes());
}

/// A complete PNG for 8-bit RGB pixels.
pub fn encode_rgb(rgb: &[u8], width: usize, height: usize) -> Vec<u8> {
    encode(rgb, width, height, 3)
}

/// A complete PNG for 8-bit RGBA pixels.
pub fn encode_rgba(rgba: &[u8], width: usize, height: usize) -> Vec<u8> {
    encode(rgba, width, height, 4)
}

fn encode(px: &[u8], width: usize, height: usize, bpp: usize) -> Vec<u8> {
    let raw = filter_rows_bpp(px, width, height, bpp);
    let cfg = zlib_rs::DeflateConfig {
        level: 9,
        method: zlib_rs::Method::Deflated,
        window_bits: 15,
        mem_level: 9,
        strategy: zlib_rs::Strategy::Filtered,
    };
    let mut buf = vec![0u8; zlib_rs::compress_bound(raw.len())];
    let (z, _) = zlib_rs::compress_slice(&mut buf, &raw, cfg);
    let mut out = Vec::with_capacity(z.len() + 64);
    out.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(width as u32).to_be_bytes());
    ihdr.extend_from_slice(&(height as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, if bpp == 4 { 6 } else { 2 }, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    for part in z.chunks(MAXBLOCK) {
        chunk(&mut out, b"IDAT", part);
    }
    chunk(&mut out, b"IEND", &[]);
    out
}
