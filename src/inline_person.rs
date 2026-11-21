//! Exact inline keys for nonempty strings of at most nine ASCII bytes.
//! Seven payload bits per byte plus a high length sentinel: no normalization,
//! hashing, learned prefix, or data-dependent alphabet is involved.
const ASCII_MASK: u64 = 0x7f7f_7f7f_7f7f_7f7f;

#[inline]
fn pack_scalar(word: u64) -> u64 {
    let mut value = 0;
    for i in 0..8 {
        value |= ((word >> (8 * i)) & 127) << (7 * i);
    }
    value
}
#[inline]
fn unpack_scalar(value: u64) -> u64 {
    let mut word = 0;
    for i in 0..8 {
        word |= ((value >> (7 * i)) & 127) << (8 * i);
    }
    word
}
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "bmi2")]
unsafe fn pack_bmi2(word: u64) -> u64 {
    std::arch::x86_64::_pext_u64(word, ASCII_MASK)
}
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "bmi2")]
unsafe fn unpack_bmi2(value: u64) -> u64 {
    std::arch::x86_64::_pdep_u64(value, ASCII_MASK)
}

#[inline]
fn pack(word: u64) -> u64 {
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("bmi2") {
        return unsafe { pack_bmi2(word) };
    }
    pack_scalar(word)
}
#[inline]
fn unpack(value: u64) -> u64 {
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("bmi2") {
        return unsafe { unpack_bmi2(value) };
    }
    unpack_scalar(value)
}

#[inline]
pub fn encode(name: &[u8]) -> Option<u64> {
    let len = name.len();
    if !(1..=9).contains(&len) {
        return None;
    }
    let word = if len >= 8 {
        // The length check guarantees all eight bytes belong to the slice,
        // including at the final page of a read-only mapped input file.
        u64::from_le_bytes(name[..8].try_into().unwrap())
    } else {
        // Bounded overlapping loads assemble exactly the available bytes.
        // The repeated overlap agrees byte-for-byte; OR adds no new bits.
        if len >= 4 {
            let low = u32::from_le_bytes(name[..4].try_into().unwrap()) as u64;
            let high = u32::from_le_bytes(name[len - 4..].try_into().unwrap()) as u64;
            low | (high << ((len - 4) * 8))
        } else if len >= 2 {
            let low = u16::from_le_bytes(name[..2].try_into().unwrap()) as u64;
            low | ((name[len - 1] as u64) << ((len - 1) * 8))
        } else {
            name[0] as u64
        }
    };
    if word & !ASCII_MASK != 0 {
        return None;
    }
    let tail = if len == 9 {
        let byte = name[8];
        if byte >= 128 {
            return None;
        }
        (byte as u64) << 56
    } else {
        0
    };
    Some(pack(word) | tail | (1u64 << (7 * len)))
}

#[inline]
pub fn decode<'a>(key: u64, output: &'a mut [u8; 9]) -> &'a [u8] {
    assert_ne!(key, 0, "invalid inline person key");
    let highest = 63 - key.leading_zeros();
    assert!(
        highest % 7 == 0 && (7..=63).contains(&highest),
        "invalid inline person length marker"
    );
    let len = (highest / 7) as usize;
    let payload = key ^ (1u64 << highest);
    output[..8].copy_from_slice(&unpack(payload).to_le_bytes());
    output[8] = (payload >> 56) as u8;
    &output[..len]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_ascii_byte_and_length_roundtrips_including_nul_and_del() {
        for len in 1..=9 {
            for byte in 0..128u8 {
                let name = vec![byte; len];
                let key = encode(&name).unwrap();
                let mut out = [0; 9];
                assert_eq!(decode(key, &mut out), name);
            }
        }
        assert_eq!(encode(&[127; 9]), Some(u64::MAX));
        assert_eq!(encode(&[0; 9]), Some(1u64 << 63));
    }
    #[test]
    fn length_sentinel_distinguishes_zero_bytes_and_leading_zero_text() {
        let mut seen = std::collections::HashSet::new();
        for len in 1..=9 {
            assert!(seen.insert(encode(&vec![0; len]).unwrap()));
        }
        for value in [
            b"0".as_slice(),
            b"00",
            b"000",
            b"A",
            b"a",
            b"a\0",
            b"a\r",
            b"a ",
        ] {
            assert!(seen.insert(encode(value).unwrap()));
        }
    }
    #[test]
    fn unsupported_names_fall_back_without_truncation() {
        assert_eq!(encode(b""), None);
        assert_eq!(encode(b"1234567890"), None);
        assert_eq!(encode("Ω".as_bytes()), None);
        for len in 1..=9 {
            for position in 0..len {
                let mut bytes = vec![b'a'; len];
                bytes[position] = 255;
                assert_eq!(encode(&bytes), None);
            }
        }
    }
    #[test]
    fn software_bit_packing_and_injective_mixed_strings() {
        let mut seen = std::collections::HashSet::new();
        for word in [0, 1, u64::MAX, 0x7f00_7f00_7f00_7f00, 0x0102_0304_0506_0708] {
            let ascii = word & ASCII_MASK;
            let packed = pack_scalar(ascii);
            assert_eq!(packed, pack(ascii));
            assert_eq!(unpack_scalar(packed), ascii);
            assert_eq!(unpack(packed), ascii);
        }
        for a in 0..128u8 {
            for b in 0..128u8 {
                assert!(seen.insert(encode(&[a, b]).unwrap()));
            }
        }
    }
}
