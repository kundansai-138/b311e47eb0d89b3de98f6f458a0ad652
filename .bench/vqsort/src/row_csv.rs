//! Whole-row masks for short records, with the exact streaming parser for
//! arbitrary long records, the bounded final tail, and unsupported CPUs.
use crate::fast_csv;

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn masks64(ptr: *const u8) -> (u64, u64, u64) {
    use std::arch::x86_64::*;
    let a = _mm256_loadu_si256(ptr.cast());
    let b = _mm256_loadu_si256(ptr.add(32).cast());
    let comma = _mm256_set1_epi8(b',' as i8);
    let quote = _mm256_set1_epi8(b'"' as i8);
    let line = _mm256_set1_epi8(b'\n' as i8);
    let al = _mm256_cmpeq_epi8(a, line);
    let bl = _mm256_cmpeq_epi8(b, line);
    let ac = _mm256_or_si256(_mm256_cmpeq_epi8(a, comma), al);
    let bc = _mm256_or_si256(_mm256_cmpeq_epi8(b, comma), bl);
    let aq = _mm256_cmpeq_epi8(a, quote);
    let bq = _mm256_cmpeq_epi8(b, quote);
    (
        _mm256_movemask_epi8(ac) as u32 as u64 | ((_mm256_movemask_epi8(bc) as u32 as u64) << 32),
        _mm256_movemask_epi8(aq) as u32 as u64 | ((_mm256_movemask_epi8(bq) as u32 as u64) << 32),
        _mm256_movemask_epi8(al) as u32 as u64 | ((_mm256_movemask_epi8(bl) as u32 as u64) << 32),
    )
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "bmi2")]
unsafe fn extract(bits: u128, mask: u128) -> u64 {
    use std::arch::x86_64::_pext_u64;
    let low = _pext_u64(bits as u64, mask as u64);
    if mask >> 64 == 0 {
        return low;
    }
    low | (_pext_u64((bits >> 64) as u64, (mask >> 64) as u64) << (mask as u64).count_ones())
}

pub struct Rows<'a, const N: usize> {
    bytes: &'a [u8],
    position: usize,
    fast: bool,
    fallback: fast_csv::Rows<'a, N>,
}
impl<'a, const N: usize> Rows<'a, N> {
    pub fn new(bytes: &'a [u8]) -> Self {
        #[cfg(target_arch = "x86_64")]
        let fast = std::is_x86_feature_detected!("avx2")
            && std::is_x86_feature_detected!("bmi2")
            && N <= 32;
        #[cfg(not(target_arch = "x86_64"))]
        let fast = false;
        Self {
            bytes,
            position: 0,
            fast,
            fallback: fast_csv::Rows::new(bytes),
        }
    }
    #[inline]
    fn slow_row(&mut self) -> Option<[&'a [u8]; N]> {
        self.fallback = fast_csv::Rows::new(&self.bytes[self.position..]);
        let row = self.fallback.next();
        self.position += self.fallback.consumed();
        row
    }
}
impl<'a, const N: usize> Iterator for Rows<'a, N> {
    type Item = [&'a [u8]; N];
    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if !self.fast {
            return self.fallback.next();
        }
        #[cfg(target_arch = "x86_64")]
        while self.position < self.bytes.len() {
            let rest = &self.bytes[self.position..];
            if rest.len() < 64 {
                return self.slow_row();
            }
            // Both vector loads stay inside the supplied slice. A row is
            // complete here only when an actual LF was found in those bytes.
            let (s, q, l) = unsafe { masks64(rest.as_ptr()) };
            let (mut separators, quotes, lines) = if l != 0 {
                (s as u128, q as u128, l as u128)
            } else {
                if rest.len() < 128 {
                    return self.slow_row();
                }
                let (s2, q2, l2) = unsafe { masks64(rest.as_ptr().add(64)) };
                if l2 == 0 {
                    return self.slow_row();
                }
                (
                    s as u128 | ((s2 as u128) << 64),
                    q as u128 | ((q2 as u128) << 64),
                    (l2 as u128) << 64,
                )
            };
            let end = lines.trailing_zeros() as usize;
            let through_line = lines ^ (lines - 1);
            separators &= through_line;
            let quotes = quotes & through_line;
            let row_start = self.position;
            self.position += end + 1;
            if end == 0 || separators.count_ones() as usize != N {
                continue;
            }

            let mut quoted = 0;
            if quotes != 0 {
                let starts = ((separators ^ (1u128 << end)) << 1) | 1;
                let ends = separators >> 1;
                // Every quote must be at a field boundary. A one-character
                // quote field is invalid even though it is both boundaries.
                if quotes & !(starts | ends) != 0 || quotes & starts & ends != 0 {
                    continue;
                }
                let opening = unsafe { extract(quotes, starts) };
                // An empty first field has no preceding byte for its closing
                // position. Insert its necessarily-zero flag explicitly.
                let closing = unsafe { extract(quotes, ends) } << (separators as u64 & 1);
                if opening != closing {
                    continue;
                }
                quoted = opening;
            }
            let mut fields = [&[][..]; N];
            let mut start = 0;
            for slot in &mut fields {
                let stop = separators.trailing_zeros() as usize;
                let trim = (quoted & 1) as usize;
                *slot = &self.bytes[row_start + start + trim..row_start + stop - trim];
                start = stop + 1;
                separators &= separators - 1;
                quoted >>= 1;
            }
            return Some(fields);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn check<const N: usize>(bytes: &[u8]) {
        let expected: Vec<_> = bytes
            .split(|&b| b == b'\n')
            .filter_map(crate::fields::<N>)
            .collect();
        assert_eq!(
            Rows::<N>::new(bytes).collect::<Vec<_>>(),
            expected,
            "N={N}, input={bytes:?}"
        );
        let mut scalar = Rows::<N>::new(bytes);
        scalar.fast = false;
        assert_eq!(scalar.collect::<Vec<_>>(), expected);
    }
    #[test]
    fn exhaustive_short_quotes_and_empty_fields_with_fast_path_padding() {
        let alphabet = b",\n\"x";
        for len in 0..8 {
            for mut word in 0..(1usize << (2 * len)) {
                let mut bytes = vec![0; len];
                for byte in &mut bytes {
                    *byte = alphabet[word & 3];
                    word >>= 2;
                }
                bytes.extend_from_slice(b"\n");
                bytes.extend_from_slice(&[b'x'; 160]);
                check::<1>(&bytes);
                check::<2>(&bytes);
                check::<4>(&bytes);
                check::<7>(&bytes);
            }
        }
    }
    #[test]
    fn mask_half_and_tail_boundaries() {
        for len in 0..192 {
            for quote in [false, true] {
                let mut bytes = Vec::new();
                for field in 0..7 {
                    if quote {
                        bytes.push(b'"');
                    }
                    bytes.extend(std::iter::repeat_n(
                        b'x',
                        if field == 2 { len } else { field },
                    ));
                    if quote {
                        bytes.push(b'"');
                    }
                    bytes.push(if field == 6 { b'\n' } else { b',' });
                }
                let row = bytes.clone();
                bytes.extend_from_slice(&row);
                bytes.extend_from_slice(&row);
                check::<7>(&bytes);
                for cut in 0..bytes.len().min(160) {
                    check::<7>(&bytes[..bytes.len() - cut]);
                }
            }
        }
    }
    #[test]
    fn long_arbitrary_fields_and_malformed_quote_recovery() {
        let mut bytes = b"a,b,\"oops\nc,d\n".to_vec();
        bytes.extend(std::iter::repeat_n(b'0', 6000));
        bytes.extend_from_slice(b",\"long\"\n");
        bytes.extend_from_slice(b"\",x\"y\n\"\",\"\"\n ,\0\r\nlast,good");
        check::<1>(&bytes);
        check::<2>(&bytes);
        check::<4>(&bytes);
        check::<7>(&bytes);
    }
    #[test]
    fn every_quote_pattern_and_illegal_quote_position() {
        for prefix in [0, 1, 7, 19, 31, 55, 63, 64, 65, 87, 119, 127, 128] {
            for quote_flags in 0..128 {
                let mut row = Vec::new();
                for column in 0..7 {
                    let quote = quote_flags & (1 << column) != 0;
                    if quote {
                        row.push(b'"');
                    }
                    row.extend(std::iter::repeat_n(
                        b'x',
                        if column == 0 { prefix } else { column },
                    ));
                    if quote {
                        row.push(b'"');
                    }
                    row.push(if column == 6 { b'\n' } else { b',' });
                }
                let mut bytes = row.clone();
                bytes.extend_from_slice(&row);
                bytes.extend_from_slice(&[b'x'; 160]);
                check::<7>(&bytes);
                for position in 0..row.len() {
                    if row[position] == b'x' {
                        bytes[position] = b'"';
                        check::<7>(&bytes);
                        bytes[position] = b'x';
                    }
                }
            }
        }
    }
    #[test]
    fn random_differential_all_byte_values() {
        let alphabet = b"ab,\n\"\r \0\xc3\xa9";
        let mut state = 0x81b49ba24a122113u64;
        for len in 0..4096 {
            let mut bytes = Vec::with_capacity(len);
            for _ in 0..len {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                bytes.push(alphabet[state as usize % alphabet.len()]);
            }
            check::<1>(&bytes);
            check::<2>(&bytes);
            check::<4>(&bytes);
            check::<7>(&bytes);
        }
    }
    #[test]
    #[ignore = "bounded real-input parser microbenchmark"]
    fn microbench() {
        use std::{hint::black_box, io::Read, time::Instant};
        let path = std::env::var("CSV_BENCH_FILE").unwrap();
        #[cfg(target_arch = "x86_64")]
        eprintln!(
            "CPU AVX512BW={} AVX512VL={}",
            std::is_x86_feature_detected!("avx512bw"),
            std::is_x86_feature_detected!("avx512vl")
        );
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .unwrap()
            .take(64 * 1024 * 1024)
            .read_to_end(&mut bytes)
            .unwrap();
        let mut expected = None;
        for round in 0..5 {
            for candidate in [round % 2 == 0, round % 2 != 0] {
                let start = Instant::now();
                let mut checksum = 0usize;
                let mut count = 0;
                if candidate {
                    for row in Rows::<7>::new(black_box(&bytes)) {
                        let row = black_box(row);
                        count += 1;
                        checksum += row.iter().map(|f| f.len()).sum::<usize>();
                    }
                } else {
                    for row in fast_csv::Rows::<7>::new(black_box(&bytes)) {
                        let row = black_box(row);
                        count += 1;
                        checksum += row.iter().map(|f| f.len()).sum::<usize>();
                    }
                }
                if let Some(expected) = expected {
                    assert_eq!((count, checksum), expected);
                }
                expected = Some((count, checksum));
                eprintln!("whole_row candidate={candidate} round={round} bytes={} seconds={:.6} rows={count} checksum={checksum}",bytes.len(),start.elapsed().as_secs_f64());
            }
        }
    }
}
