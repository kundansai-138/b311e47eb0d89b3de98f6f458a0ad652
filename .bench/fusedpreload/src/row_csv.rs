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

/// The mask kernel needs precisely AVX512F and AVX512BW. Rust's feature
/// detection also checks the OS's opmask/ZMM register-state support.
#[inline]
fn avx512_available() -> bool {
    #[cfg(target_arch = "x86_64")]
    {std::is_x86_feature_detected!("avx512f") && std::is_x86_feature_detected!("avx512bw")}
    #[cfg(not(target_arch = "x86_64"))]
    {false}
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f,avx512bw")]
#[inline]
unsafe fn masks64_avx512(ptr: *const u8) -> (u64, u64, u64) {
    use std::arch::x86_64::*;
    // The caller proves that all64 bytes are inside the input slice.
    let bytes = _mm512_loadu_si512(ptr.cast());
    let lines = _mm512_cmpeq_epi8_mask(bytes, _mm512_set1_epi8(b'\n' as i8));
    let commas = _mm512_cmpeq_epi8_mask(bytes, _mm512_set1_epi8(b',' as i8));
    let quotes = _mm512_cmpeq_epi8_mask(bytes, _mm512_set1_epi8(b'"' as i8));
    (commas | lines, quotes, lines)
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
    avx512: bool,
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
            // Experimental opt-in, evaluated once per Rows, never per row.
            avx512: fast && avx512_available()
                && std::env::var("ROUND4_CSV_SIMD").as_deref() == Ok("avx512"),
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
impl<'a, const N: usize> Rows<'a, N> {
    // Called only by the guarded Iterator dispatch or a matching target-feature
    // chunk loop. Always-inline keeps extraction and its row callback together.
    #[inline(always)]
    unsafe fn next_impl<const WIDE: bool>(&mut self) -> Option<[&'a [u8]; N]> {
        if !self.fast { return self.fallback.next(); }
        #[cfg(target_arch = "x86_64")]
        while self.position < self.bytes.len() {
            let rest = &self.bytes[self.position..];
            if rest.len() < 64 { return self.slow_row(); }
            let (s,q,l) = if WIDE { unsafe { masks64_avx512(rest.as_ptr()) } } else { unsafe { masks64(rest.as_ptr()) } };
            let (mut low, high, quotes, end) = if l != 0 {
                let through = l ^ l.wrapping_sub(1);
                (s & through, 0, (q & through) as u128, l.trailing_zeros() as usize)
            } else {
                if rest.len() < 128 { return self.slow_row(); }
                let (s2,q2,l2) = if WIDE { unsafe { masks64_avx512(rest.as_ptr().add(64)) } } else { unsafe { masks64(rest.as_ptr().add(64)) } };
                if l2 == 0 { return self.slow_row(); }
                let through = l2 ^ l2.wrapping_sub(1);
                (s, s2 & through, q as u128 | (((q2 & through) as u128) << 64),
                 64 + l2.trailing_zeros() as usize)
            };
            let row_start = self.position;
            self.position += end + 1;
            if end == 0 || low.count_ones() as usize + high.count_ones() as usize != N {continue;}
            let mut quoted = 0;
            if quotes != 0 {
                let separators = low as u128 | ((high as u128) << 64);
                let starts = ((separators ^ (1u128 << end)) << 1) | 1;
                let ends = separators >> 1;
                if quotes & !(starts | ends) != 0 || quotes & starts & ends != 0 {continue;}
                let opening = unsafe {extract(quotes, starts)};
                let closing = unsafe {extract(quotes, ends)} << (low & 1);
                if opening != closing {continue;}
                quoted = opening;
            }
            // Exactly N separators through the first LF prove that every slot
            // is initialized once. Monotone separator positions bound slices;
            // quote validation proves a trimmed field has two distinct quotes.
            let mut fields = std::mem::MaybeUninit::<[&'a [u8]; N]>::uninit();
            let output = fields.as_mut_ptr().cast::<&'a [u8]>();
            let base = unsafe {self.bytes.as_ptr().add(row_start)};
            let mut start = 0;
            let mut offset = 0;
            for column in 0..N {
                if low == 0 {low = high;offset = 64;}
                let stop = offset + low.trailing_zeros() as usize;
                let trim = (quoted & 1) as usize;
                unsafe {output.add(column).write(std::slice::from_raw_parts(
                    base.add(start + trim), stop - start - 2 * trim));}
                start = stop + 1;
                low &= low - 1;
                quoted >>= 1;
            }
            return Some(unsafe {fields.assume_init()});
        }
        None
    }

    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx2,bmi2")]
    unsafe fn for_each_avx2<F>(mut self, mut callback: F)
    where F: FnMut([&'a [u8]; N]) {
        while let Some(row) = self.next_impl::<false>() { callback(row); }
    }

    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx512f,avx512bw,avx2,bmi2")]
    unsafe fn for_each_avx512<F>(mut self, mut callback: F)
    where F: FnMut([&'a [u8]; N]) {
        while let Some(row) = self.next_impl::<true>() { callback(row); }
    }
}

impl<'a, const N: usize> Iterator for Rows<'a, N> {
    type Item = [&'a [u8]; N];
    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if !self.fast { return self.fallback.next(); }
        // Both booleans are established by constructor CPUID/OS checks.
        unsafe {
            if self.avx512 { self.next_impl::<true>() }
            else { self.next_impl::<false>() }
        }
    }
    #[inline]
    fn for_each<F>(self, callback: F)
    where F: FnMut(Self::Item) {
        #[cfg(target_arch = "x86_64")]
        if self.fast {
            // Select the ISA once per complete input chunk. Each worker loop is
            // monomorphized with the caller's exact closure (no function pointer).
            unsafe {
                if self.avx512 { self.for_each_avx512(callback); }
                else { self.for_each_avx2(callback); }
            }
            return;
        }
        self.fallback.for_each(callback);
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
        let mut avx2 = Rows::<N>::new(bytes);
        avx2.avx512 = false;
        assert_eq!(avx2.collect::<Vec<_>>(), expected, "forced AVX2 N={N}");
        if avx512_available() {
            let mut avx512 = Rows::<N>::new(bytes);
            avx512.avx512 = true;
            assert_eq!(avx512.collect::<Vec<_>>(), expected, "forced AVX512 N={N}");
        }
        let mut callback_avx2 = Rows::<N>::new(bytes);
        callback_avx2.avx512 = false;
        let mut callback_rows = Vec::new();
        callback_avx2.for_each(|row| callback_rows.push(row));
        assert_eq!(callback_rows, expected, "callback AVX2 N={N}");
        if avx512_available() {
            let mut callback_avx512 = Rows::<N>::new(bytes);
            callback_avx512.avx512 = true;
            let mut callback_rows = Vec::new();
            callback_avx512.for_each(|row| callback_rows.push(row));
            assert_eq!(callback_rows, expected, "callback AVX512 N={N}");
        }
        let mut callback_scalar = Rows::<N>::new(bytes);
        callback_scalar.fast = false;
        let mut callback_rows = Vec::new();
        callback_scalar.for_each(|row| callback_rows.push(row));
        assert_eq!(callback_rows, expected, "callback fallback N={N}");
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
    fn avx512_mask_kernel_matches_avx2_and_scalar_when_supported() {
        let available = avx512_available();
        if std::env::var("ROUND4_REQUIRE_AVX512").as_deref()==Ok("1") {
            assert!(available, "this run requires an actual AVX512F/BW-capable CPU");
        }
        if !available {eprintln!("AVX512F/BW unavailable; guarded path not executed");return;}
        #[cfg(target_arch="x86_64")]
        {
            assert!(std::is_x86_feature_detected!("avx2"));
            let mut data=[0u8;192];
            let mut state=0x71e27a03514f92cbu64;
            for seed in 0..512 {
                for byte in &mut data {
                    state^=state<<13;state^=state>>7;state^=state<<17;
                    *byte=if seed<256 {seed as u8}else{state as u8};
                }
                for offset in 0..64 {
                    let bytes=&data[offset..offset+64];
                    let mut expected=(0u64,0u64,0u64);
                    for (bit,&byte) in bytes.iter().enumerate() {
                        expected.0|=u64::from(byte==b',' || byte==b'\n')<<bit;
                        expected.1|=u64::from(byte==b'"')<<bit;
                        expected.2|=u64::from(byte==b'\n')<<bit;
                    }
                    let actual=unsafe {masks64_avx512(bytes.as_ptr())};
                    assert_eq!(actual,expected,"seed={seed},offset={offset}");
                    assert_eq!(actual,unsafe {masks64(bytes.as_ptr())});
                }
            }
            eprintln!("AVX512F/BW CSV mask path executed and matched scalar/AVX2");
        }
    }

}
