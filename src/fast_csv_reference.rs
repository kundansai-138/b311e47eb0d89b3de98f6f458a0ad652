//! Strict LF-only CSV rows. Commas always delimit fields in this dialect.
//! One vector pass identifies commas, line feeds and quotes together; quote
//! masks validate whole-field wrapping without rescanning field contents.

#[inline]
fn scalar_masks(bytes: &[u8]) -> (u32, u32) {
    let mut separators = 0u32;
    let mut quotes = 0u32;
    for (i, &byte) in bytes.iter().enumerate() {
        if byte == b',' || byte == b'\n' {
            separators |= 1 << i;
        }
        if byte == b'"' {
            quotes |= 1 << i;
        }
    }
    (separators, quotes)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn vector_masks(ptr: *const u8) -> (u32, u32) {
    use std::arch::x86_64::*;
    // Caller checks AVX2 availability and that 32 input bytes remain. No read
    // crosses the provided slice, including the last page of a mapped file.
    let bytes = _mm256_loadu_si256(ptr.cast());
    let commas = _mm256_cmpeq_epi8(bytes, _mm256_set1_epi8(b',' as i8));
    let lines = _mm256_cmpeq_epi8(bytes, _mm256_set1_epi8(b'\n' as i8));
    let quotes = _mm256_cmpeq_epi8(bytes, _mm256_set1_epi8(b'"' as i8));
    (
        _mm256_movemask_epi8(_mm256_or_si256(commas, lines)) as u32,
        _mm256_movemask_epi8(quotes) as u32,
    )
}

pub struct Rows<'a, const N: usize> {
    bytes: &'a [u8],
    scan: usize,
    block: usize,
    separators: u32,
    quote_bits: u32,
    field_start: usize,
    row_start: usize,
    column: usize,
    quotes: u8,
    valid: bool,
    done: bool,
    vector: bool,
    fields: [&'a [u8]; N],
}

impl<'a, const N: usize> Rows<'a, N> {
    pub fn new(bytes: &'a [u8]) -> Self {
        #[cfg(target_arch = "x86_64")]
        let vector = std::is_x86_feature_detected!("avx2");
        #[cfg(not(target_arch = "x86_64"))]
        let vector = false;
        Self {
            bytes,
            scan: 0,
            block: 0,
            separators: 0,
            quote_bits: 0,
            field_start: 0,
            row_start: 0,
            column: 0,
            quotes: 0,
            valid: true,
            done: false,
            vector,
            fields: [&[]; N],
        }
    }

    #[inline]
    fn separator(&mut self) -> Option<usize> {
        while self.separators == 0 {
            self.quotes = (self.quotes as u32 + self.quote_bits.count_ones()).min(3) as u8;
            self.quote_bits = 0;
            if self.scan == self.bytes.len() {
                return None;
            }
            self.block = self.scan;
            let size = (self.bytes.len() - self.scan).min(32);
            #[cfg(target_arch = "x86_64")]
            let masks = if self.vector && size == 32 {
                unsafe { vector_masks(self.bytes.as_ptr().add(self.scan)) }
            } else {
                scalar_masks(&self.bytes[self.scan..self.scan + size])
            };
            #[cfg(not(target_arch = "x86_64"))]
            let masks = scalar_masks(&self.bytes[self.scan..self.scan + size]);
            self.scan += size;
            self.separators = masks.0;
            self.quote_bits = masks.1;
        }
        let bit = self.separators.trailing_zeros();
        let before = (1u32 << bit).wrapping_sub(1);
        self.quotes = (self.quotes as u32 + (self.quote_bits & before).count_ones()).min(3) as u8;
        self.quote_bits &= !before;
        self.separators &= self.separators - 1;
        Some(self.block + bit as usize)
    }

    #[inline]
    fn finish_field(&mut self, end: usize) {
        let mut value = &self.bytes[self.field_start..end];
        if self.quotes != 0 {
            if self.quotes == 2
                && value.len() >= 2
                && value[0] == b'"'
                && value[value.len() - 1] == b'"'
            {
                value = &value[1..value.len() - 1];
            } else {
                self.valid = false;
            }
        }
        if self.column < N {
            self.fields[self.column] = value;
        }
        self.column = (self.column + 1).min(N + 1);
        self.quotes = 0;
        self.field_start = end + 1;
    }
}

impl<'a, const N: usize> Iterator for Rows<'a, N> {
    type Item = [&'a [u8]; N];
    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        loop {
            let Some(end) = self.separator() else {
                self.done = true;
                if self.row_start == self.bytes.len() {
                    return None;
                }
                self.finish_field(self.bytes.len());
                return (self.valid && self.column == N).then_some(self.fields);
            };
            self.finish_field(end);
            if self.bytes[end] == b'\n' {
                let valid = self.valid && self.column == N && end != self.row_start;
                self.row_start = end + 1;
                self.column = 0;
                self.valid = true;
                if valid {
                    return Some(self.fields);
                }
            }
        }
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
            "vector input={bytes:?}"
        );
        let mut scalar = Rows::<N>::new(bytes);
        scalar.vector = false;
        assert_eq!(
            scalar.collect::<Vec<_>>(),
            expected,
            "scalar input={bytes:?}"
        );
    }

    #[test]
    fn strict_quotes_cr_empty_and_final_record() {
        for input in [
            b"a,b\n\"a\",\"b\"\n".as_slice(),
            b"a,b\r\n",
            b"a,\"b\"\r\n",
            b"a,\"\"\n",
            b"\"a\",\"b\"",
            b"a,\"b\n\"x\",y\n",
            b"\n\n",
            b"",
            b"a,b,c\na,b\n",
            b"a,\"b\"\"\nc,d",
            b"a,b\na,",
            b" ,\0\r",
        ] {
            check::<2>(input);
        }
    }

    #[test]
    fn all_block_alignments_and_long_fields() {
        for prefix in 0..96 {
            let mut data = vec![b'x'; prefix];
            data.extend_from_slice(b",\"with spaces \"\n\"\",plain\n\"bad\"quote,x\n");
            data.extend_from_slice(b"\"");
            data.extend(std::iter::repeat_n(b'0', 6000));
            data.extend_from_slice(b"\",last\n");
            check::<2>(&data);
        }
    }

    #[test]
    fn differential_random_bytes_and_field_counts() {
        let alphabet = b"ab,\n\"\r \0\xc3\xa9";
        let mut state = 0x12981234abcdefu64;
        for len in 0..512 {
            let mut bytes = Vec::with_capacity(len);
            for _ in 0..len {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                bytes.push(alphabet[state as usize % alphabet.len()]);
            }
            check::<2>(&bytes);
            check::<4>(&bytes);
            check::<7>(&bytes);
        }
    }
}
