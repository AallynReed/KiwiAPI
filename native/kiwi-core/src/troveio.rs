//! Trove's FNV-1a-variant checksum, byte-identical to trove.dll.

const FNV_OFFSET: u32 = 2_166_136_261;
const FNV_PRIME: u32 = 16_777_619;

/// A signed `char` widened to u32, as trove.c reads the trailing bytes.
#[inline]
fn se(b: u8) -> u32 {
    b as i8 as i32 as u32
}

pub fn calculate_hash(data: &[u8]) -> u32 {
    let mut h = FNV_OFFSET;
    let mut words = data.chunks_exact(4);
    for w in &mut words {
        h = FNV_PRIME.wrapping_mul(h ^ u32::from_le_bytes([w[0], w[1], w[2], w[3]]));
    }
    let val = match *words.remainder() {
        [] => return h,
        [a] => se(a),
        [a, b] => (se(a) << 8) | se(b),
        [a, b, c] => (((se(a) << 8) | se(b)) << 8) | se(c),
        _ => unreachable!(),
    };
    FNV_PRIME.wrapping_mul(h ^ val)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_offset() {
        assert_eq!(calculate_hash(b""), FNV_OFFSET);
    }
}
