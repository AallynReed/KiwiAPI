//! `app/trove/audio/encode.py`: 16-bit PCM in, a `.wem` the game loads out
//! (PCM `0xFFFE` or Wwise IMA ADPCM `0x0002`), headers byte for byte.

const FMT_PCM: u16 = 0xFFFE;
const FMT_ADPCM: u16 = 0x0002;
const ADPCM_BLOCK: usize = 0x24;
const ADPCM_HEADER: usize = 4;
const SAMPLES_PER_BLOCK: usize = (ADPCM_BLOCK - ADPCM_HEADER) * 2;
const MAX_SAMPLES: usize = 48000 * 60 * 10;

const STEP_TABLE: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66, 73, 80, 88, 97, 107,
    118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449, 494, 544, 598, 658, 724, 796, 876, 963,
    1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272, 2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894,
    6484, 7132, 7845, 8630, 9493, 10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794,
    32767,
];
const INDEX_TABLE: [i32; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];

fn channel_mask(channels: u16) -> Option<u32> {
    match channels {
        1 => Some(0x4),
        2 => Some(0x3),
        _ => None,
    }
}

/// `_check`; Err is the `EncodeError` text.
fn check(channels: u16, rate: u32, frames: usize) -> Result<(), String> {
    if channel_mask(channels).is_none() {
        return Err("only mono and stereo are supported".into());
    }
    if !(1000..=96000).contains(&rate) {
        return Err(format!("sample rate {rate} is out of range"));
    }
    if frames == 0 {
        return Err("the clip is empty".into());
    }
    if frames > MAX_SAMPLES {
        return Err("the clip is too long".into());
    }
    Ok(())
}

fn riff(fmt: &[u8], data: &[u8]) -> Vec<u8> {
    let mut body = b"WAVE".to_vec();
    body.extend_from_slice(b"fmt ");
    body.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
    body.extend_from_slice(fmt);
    body.extend_from_slice(b"JUNK");
    body.extend_from_slice(&4u32.to_le_bytes());
    body.extend_from_slice(&[0; 4]);
    body.extend_from_slice(b"data");
    body.extend_from_slice(&(data.len() as u32).to_le_bytes());
    body.extend_from_slice(data);
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

fn fmt(codec: u16, channels: u16, rate: u32, avg: u32, align: u16, bits: u16) -> Vec<u8> {
    let config = u32::from(channels) | (1 << 8) | (channel_mask(channels).unwrap_or(0) << 12);
    let mut f = Vec::with_capacity(24);
    f.extend_from_slice(&codec.to_le_bytes());
    f.extend_from_slice(&channels.to_le_bytes());
    f.extend_from_slice(&rate.to_le_bytes());
    f.extend_from_slice(&avg.to_le_bytes());
    f.extend_from_slice(&align.to_le_bytes());
    f.extend_from_slice(&bits.to_le_bytes());
    f.extend_from_slice(&6u16.to_le_bytes());
    f.extend_from_slice(&0u16.to_le_bytes());
    f.extend_from_slice(&config.to_le_bytes());
    f
}

pub fn pcm_wem(samples: &[u8], channels: u16, rate: u32) -> Result<Vec<u8>, String> {
    let align = usize::from(channels) * 2;
    let frames = samples.len() / align.max(1);
    check(channels, rate, frames)?;
    let f = fmt(FMT_PCM, channels, rate, rate * align as u32, align as u16, 16);
    Ok(riff(&f, &samples[..frames * align]))
}

pub fn adpcm_wem(samples: &[u8], channels: u16, rate: u32) -> Result<Vec<u8>, String> {
    let ch = usize::from(channels);
    let frames = samples.len() / (ch * 2).max(1);
    check(channels, rate, frames)?;
    let align = ADPCM_BLOCK * ch;
    let blocks = frames.div_ceil(SAMPLES_PER_BLOCK);
    let mut out = vec![0u8; blocks * align];
    for channel in 0..ch {
        let (mut predictor, mut index) = (0i32, 0i32);
        for block in 0..blocks {
            let at = block * align + channel * ADPCM_BLOCK;
            out[at..at + 2].copy_from_slice(&(predictor as i16).to_le_bytes());
            out[at + 2..at + 4].copy_from_slice(&(index as i16).to_le_bytes());
            for i in 0..SAMPLES_PER_BLOCK {
                let frame = block * SAMPLES_PER_BLOCK + i;
                let offset = (frame * ch + channel) * 2;
                let target = if offset + 2 <= samples.len() {
                    i32::from(i16::from_le_bytes([samples[offset], samples[offset + 1]]))
                } else {
                    predictor
                };
                let step = STEP_TABLE[index as usize];
                let mut diff = target - predictor;
                let mut nibble = if diff < 0 { 8 } else { 0 };
                if diff < 0 {
                    diff = -diff;
                }
                let mut delta = step >> 3;
                if diff >= step {
                    nibble |= 4;
                    diff -= step;
                    delta += step;
                }
                if diff >= step >> 1 {
                    nibble |= 2;
                    diff -= step >> 1;
                    delta += step >> 1;
                }
                if diff >= step >> 2 {
                    nibble |= 1;
                    delta += step >> 2;
                }
                predictor += if nibble & 8 != 0 { -delta } else { delta };
                predictor = predictor.clamp(-32768, 32767);
                index = (index + INDEX_TABLE[(nibble & 7) as usize]).clamp(0, 88);
                let byte_at = at + ADPCM_HEADER + (i >> 1);
                if i & 1 == 1 {
                    out[byte_at] |= ((nibble & 0x0F) << 4) as u8;
                } else {
                    out[byte_at] = (nibble & 0x0F) as u8;
                }
            }
        }
    }
    let avg = (u64::from(rate) * align as u64 / SAMPLES_PER_BLOCK as u64) as u32;
    let f = fmt(FMT_ADPCM, channels, rate, avg, align as u16, 4);
    Ok(riff(&f, &out))
}

/// `silence()`: 64 frames of mono 24 kHz ADPCM.
pub fn silence() -> Vec<u8> {
    adpcm_wem(&[0u8; SAMPLES_PER_BLOCK * 2], 1, 24000).unwrap_or_default()
}

pub fn encode(samples: &[u8], channels: u16, rate: u32, codec: &str) -> Result<Vec<u8>, String> {
    match codec {
        "pcm" => pcm_wem(samples, channels, rate),
        "adpcm" => adpcm_wem(samples, channels, rate),
        other => Err(format!("cannot write {} - only adpcm and pcm can be encoded", kiwi_core::pystr::repr(other))),
    }
}
