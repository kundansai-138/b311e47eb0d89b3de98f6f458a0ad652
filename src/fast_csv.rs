//! Strict LF-only CSV. Commas delimit even inside malformed quote sequences.
//! Fields are assembled in a row-local array so the ordinary path needs no
//! persistent column counter, saturated counter, or second array copy.

#[inline]
fn scalar_masks(bytes: &[u8]) -> (u32, u32, u32) {
    let mut separators = 0;
    let mut quotes = 0;
    let mut lines = 0;
    for (i, &byte) in bytes.iter().enumerate() {
        if byte == b',' || byte == b'\n' {
            separators |= 1 << i;
        }
        if byte == b'"' {
            quotes |= 1 << i;
        }
        if byte == b'\n' {
            lines |= 1 << i;
        }
    }
    (separators, quotes, lines)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn vector_masks(ptr: *const u8) -> (u32, u32, u32) {
    use std::arch::x86_64::*;
    // The caller proves that a complete 32-byte block is readable.
    let bytes = _mm256_loadu_si256(ptr.cast());
    let commas = _mm256_cmpeq_epi8(bytes, _mm256_set1_epi8(b',' as i8));
    let lines = _mm256_cmpeq_epi8(bytes, _mm256_set1_epi8(b'\n' as i8));
    let quotes = _mm256_cmpeq_epi8(bytes, _mm256_set1_epi8(b'"' as i8));
    (
        _mm256_movemask_epi8(_mm256_or_si256(commas, lines)) as u32,
        _mm256_movemask_epi8(quotes) as u32,
        _mm256_movemask_epi8(lines) as u32,
    )
}

pub struct Rows<'a, const N: usize> {
    bytes: &'a [u8],
    scan: usize,
    block: usize,
    separators: u32,
    quote_bits: u32,
    line_bits: u32,
    row_start: usize,
    vector: bool,
}

impl<'a, const N: usize> Rows<'a, N> {
    #[cfg(any(test, feature = "row-csv"))]
    #[inline]
    pub fn consumed(&self) -> usize {
        self.row_start
    }
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
            line_bits: 0,
            row_start: 0,
            vector,
        }
    }

    #[inline]
    fn scan_block(&mut self) {
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
        (self.separators, self.quote_bits, self.line_bits) = masks;
    }

    #[inline]
    fn field_end(&mut self) -> (usize, usize) {
        let mut quotes = 0usize;
        while self.separators == 0 {
            quotes += self.quote_bits.count_ones() as usize;
            self.quote_bits = 0;
            if self.scan == self.bytes.len() {
                return (self.scan, quotes);
            }
            self.scan_block();
        }
        let bit = self.separators.trailing_zeros();
        // Includes the separator, which cannot itself be a quote.
        let through = self.separators ^ (self.separators - 1);
        quotes += (self.quote_bits & through).count_ones() as usize;
        self.quote_bits &= !through;
        self.separators &= self.separators - 1;
        (self.block + bit as usize, quotes)
    }

    #[inline]
    fn advance_row(&mut self, end: usize) {
        self.row_start = if end == self.bytes.len() {
            end
        } else {
            end + 1
        };
    }

    // Bad fields/counts are rare. Skip a malformed record with LF masks,
    // discarding all its remaining quotes and commas without parsing fields.
    #[cold]
    fn discard_row(&mut self, end: usize) {
        if end == self.bytes.len() || self.bytes[end] == b'\n' {
            self.advance_row(end);
            return;
        }
        loop {
            let lines = self.line_bits & self.separators;
            if lines != 0 {
                let bit = lines.trailing_zeros();
                let through = lines ^ (lines - 1);
                self.separators &= !through;
                self.quote_bits &= !through;
                self.row_start = self.block + bit as usize + 1;
                return;
            }
            if self.scan == self.bytes.len() {
                self.separators = 0;
                self.quote_bits = 0;
                self.row_start = self.scan;
                return;
            }
            self.scan_block();
        }
    }
}

impl<'a, const N: usize> Iterator for Rows<'a, N> {
    type Item = [&'a [u8]; N];
    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        'rows: while self.row_start < self.bytes.len() {
            let mut fields = [&[][..]; N];
            let mut start = self.row_start;
            // The engine uses N=2/4/7. N=0 cannot represent a valid CSV line.
            if N == 0 {
                self.row_start = self.bytes.len();
                return None;
            }
            for (column, slot) in fields.iter_mut().enumerate() {
                let (end, quotes) = self.field_end();
                let line_end = end == self.bytes.len() || self.bytes[end] == b'\n';
                let mut value = &self.bytes[start..end];
                if quotes != 0 {
                    if quotes == 2
                        && value.len() >= 2
                        && value[0] == b'"'
                        && value[value.len() - 1] == b'"'
                    {
                        value = &value[1..value.len() - 1];
                    } else {
                        self.discard_row(end);
                        continue 'rows;
                    }
                }
                *slot = value;
                if column + 1 == N {
                    if line_end {
                        let nonempty = end != self.row_start;
                        self.advance_row(end);
                        if nonempty {
                            return Some(fields);
                        }
                    } else {
                        self.discard_row(end);
                    }
                    continue 'rows;
                }
                if line_end {
                    self.advance_row(end);
                    continue 'rows;
                }
                start = end + 1;
            }
        }
        None
    }
}

#[cfg(test)]
#[path = "fast_csv_reference.rs"]
mod reference;

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
            "vector {bytes:?}"
        );
        let mut scalar = Rows::<N>::new(bytes);
        scalar.vector = false;
        assert_eq!(scalar.collect::<Vec<_>>(), expected, "scalar {bytes:?}");
        assert_eq!(
            reference::Rows::<N>::new(bytes).collect::<Vec<_>>(),
            expected
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
            b"a,b,\"bad\n\"good\",b\n",
            b",\n",
            b",",
            b"a\n",
            b"a",
            b"\"\"",
        ] {
            check::<0>(input);
            check::<1>(input);
            check::<2>(input);
            check::<4>(input);
            check::<7>(input);
        }
    }

    #[test]
    fn all_block_alignments_long_fields_and_bad_row_recovery() {
        for prefix in 0..96 {
            let mut data = vec![b'x'; prefix];
            data.extend_from_slice(b",\"with spaces \"\n\"\",plain\n\"bad\"quote,x\n");
            data.extend_from_slice(b"\"");
            data.extend(std::iter::repeat_n(b'0', 6000));
            data.extend_from_slice(b"\",last\na,b,c,d,e\nfinal,good");
            check::<1>(&data);
            check::<2>(&data);
            check::<4>(&data);
            check::<7>(&data);
        }
    }

    #[test]
    fn differential_random_bytes_and_field_counts() {
        let alphabet = b"ab,\n\"\r \0\xc3\xa9";
        let mut state = 0x12981234abcdefu64;
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
    fn exhaustive_short_delimiters_and_quotes() {
        let alphabet = b",\n\"x";
        for len in 0..8 {
            for mut word in 0..(1usize << (2 * len)) {
                let mut bytes = vec![0; len];
                for byte in &mut bytes {
                    *byte = alphabet[word & 3];
                    word >>= 2;
                }
                check::<1>(&bytes);
                check::<2>(&bytes);
                check::<4>(&bytes);
            }
        }
    }
}
