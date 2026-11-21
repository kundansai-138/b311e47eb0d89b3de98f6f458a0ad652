//! Worker-local timestamp parsing. Calendar cache entries are keyed by every
//! raw date byte and populated only after strict Gregorian validation.
#[cfg(any(test, not(feature = "cached-time")))]
use crate::timestamp;
#[cfg(feature = "cached-time")]
use crate::{civil_days, digits, pair, DAY};

#[cfg(feature = "cached-time")]
#[derive(Clone, Copy, Default)]
struct DateEntry {
    first: u64,
    last: u16,
    base: i64,
    valid: bool,
}

pub struct TimestampParser {
    #[cfg(feature = "cached-time")]
    dates: [DateEntry; 64],
}

impl TimestampParser {
    pub fn new() -> Self {
        Self {
            #[cfg(feature = "cached-time")]
            dates: [DateEntry::default(); 64],
        }
    }

    #[cfg(not(feature = "cached-time"))]
    #[inline]
    pub fn parse(&mut self, bytes: &[u8]) -> Option<i64> {
        timestamp(bytes)
    }

    #[cfg(feature = "cached-time")]
    #[inline]
    pub fn parse(&mut self, b: &[u8]) -> Option<i64> {
        if b.len() != 20 && b.len() != 25 {
            return digits(b);
        }
        if b[4] != b'-' {
            return digits(b);
        }
        if b[10] != b'T' {
            return None;
        }
        #[cfg(feature = "swar-clock")]
        let time = crate::clock_word::parse(u64::from_le_bytes(b[11..19].try_into().unwrap()))?;
        #[cfg(not(feature = "swar-clock"))]
        let time = {
        if b[13] != b':' || b[16] != b':' { return None; }
        let h = pair(b, 11)?;
        let m = pair(b, 14)?;
        let s = pair(b, 17)?;
        if h > 23 || m > 59 || s > 59 {
            return None;
        }
        (h * 3600 + m * 60 + s) * 1000
        };
        let offset = if b.len() == 20 {
            if b[19] != b'Z' {
                return None;
            }
            0
        } else {
            if (b[19] != b'+' && b[19] != b'-') || b[22] != b':' {
                return None;
            }
            let h = pair(b, 20)?;
            let m = pair(b, 23)?;
            if h > 23 || m > 59 {
                return None;
            }
            (h * 60 + m) * 60_000 * if b[19] == b'+' { 1 } else { -1 }
        };
        Some(self.date_base(b)? + time - offset)
    }

    #[cfg(not(feature = "trusted-ingest"))]
    #[inline]
    pub fn ingest(&mut self, b: &[u8]) -> Option<i64> {
        self.parse(b)
    }

    #[cfg(feature = "trusted-ingest")]
    #[inline]
    pub fn ingest(&mut self, b: &[u8]) -> Option<i64> {
        // rules.md guarantees ingest_ts is valid F1. This method is used only
        // for that column; event/click ts always uses the fully checked parse.
        // Keep a defensive fallback for unexpected lengths without a panic.
        if b.len() != 20 {
            return self.parse(b);
        }
        let h = b[11] as i64 * 10 + b[12] as i64 - 528;
        let m = b[14] as i64 * 10 + b[15] as i64 - 528;
        let s = b[17] as i64 * 10 + b[18] as i64 - 528;
        Some(self.date_base(b)? + (h * 3600 + m * 60 + s) * 1000)
    }

    #[cfg(feature = "cached-time")]
    #[inline]
    fn date_base(&mut self, b: &[u8]) -> Option<i64> {
        let first = u64::from_le_bytes(b[..8].try_into().unwrap());
        let last = u16::from_le_bytes([b[8], b[9]]);
        // All dates in two adjacent months occupy separate slots. Month
        // parity is only a selector: all10 raw prefix bytes decide a hit.
        let slot = ((b[6] as usize & 1) * 32 + b[8] as usize * 10 + b[9] as usize) & 63;
        let entry = self.dates[slot];
        if entry.valid && entry.first == first && entry.last == last {
            Some(entry.base)
        } else {
            self.date_miss(b, slot, first, last)
        }
    }

    #[cfg(feature = "cached-time")]
    #[cold]
    #[inline(never)]
    fn date_miss(&mut self, b: &[u8], slot: usize, first: u64, last: u16) -> Option<i64> {
        if b[4] != b'-' || b[7] != b'-' {
            return None;
        }
        let y = digits(&b[..4])?;
        let m = pair(b, 5)?;
        let d = pair(b, 8)?;
        if y == 0 || !(1..=12).contains(&m) {
            return None;
        }
        let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
        let max = match m {
            2 => {
                if leap {
                    29
                } else {
                    28
                }
            }
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        };
        if d < 1 || d > max {
            return None;
        }
        let base = civil_days(y, m, d) * DAY;
        self.dates[slot] = DateEntry {
            first,
            last,
            base,
            valid: true,
        };
        Some(base)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "bounded real mixed-timestamp microbenchmark"]
    fn microbench_real_timestamps() {
        use std::{hint::black_box, io::Read, time::Instant};
        let path=std::env::var("CSV_BENCH_FILE").unwrap();
        let mut bytes=Vec::new();std::fs::File::open(path).unwrap().take(16*1024*1024).read_to_end(&mut bytes).unwrap();
        let values:Vec<_>=crate::rows::<7>(&bytes).map(|f|f[2]).collect();
        let mut expected=None;
        for round in 0..7 {for candidate in [round%2==0,round%2!=0] {
            let start=Instant::now();let mut checksum=0i64;let mut count=0usize;
            if candidate {
                let mut parser=super::TimestampParser::new();
                for _ in 0..4 {for &value in &values {if let Some(ts)=parser.parse(black_box(value)){count+=1;checksum=checksum.wrapping_add(black_box(ts));}}}
            } else {
                let mut parser=crate::timestamp_reference::TimestampParser::new();
                for _ in 0..4 {for &value in &values {if let Some(ts)=parser.parse(black_box(value)){count+=1;checksum=checksum.wrapping_add(black_box(ts));}}}
            }
            if let Some(expected)=expected {assert_eq!((checksum,count),expected);}expected=Some((checksum,count));
            eprintln!("mixed_time candidate={candidate} round={round} values={} seconds={:.6} valid={count} checksum={checksum}",values.len()*4,start.elapsed().as_secs_f64());
        }}
    }
    use super::*;

    #[cfg(feature = "cached-time")]
    #[test]
    fn cache_holds_two_adjacent_full_months_without_eviction() {
        for months in [[(2026, 7), (2026, 8)], [(2026, 12), (2027, 1)]] {
            let mut parser = TimestampParser::new();
            for (y, m) in months {
                for d in 1..=31 {
                    let value = format!("{y:04}-{m:02}-{d:02}T00:00:00Z");
                    assert_eq!(parser.parse(value.as_bytes()), timestamp(value.as_bytes()));
                }
            }
            assert_eq!(parser.dates.iter().filter(|entry| entry.valid).count(), 62);
        }
    }

    #[test]
    fn strict_parse_matches_reference_for_byte_mutations_and_cache_collisions() {
        let mut parser = TimestampParser::new();
        let seeds = [
            b"2028-02-29T23:59:59Z".as_slice(),
            b"1900-03-01T00:00:00Z",
            b"2000-02-29T00:00:00-00:00",
            b"2026-10-01T23:59:59+23:59",
        ];
        for seed in seeds {
            assert_eq!(parser.parse(seed), timestamp(seed));
            for position in 0..seed.len() {
                for byte in 0..=255u8 {
                    let mut candidate = seed.to_vec();
                    candidate[position] = byte;
                    assert_eq!(
                        parser.parse(&candidate),
                        timestamp(&candidate),
                        "{candidate:?}"
                    );
                    // Re-prime an unchanged valid entry after each mutation;
                    // malformed inputs must never poison a cached valid date.
                    assert_eq!(parser.parse(seed), timestamp(seed));
                }
            }
        }
    }

    #[test]
    fn calendar_boundaries_offsets_and_epoch_forms_match_reference() {
        let mut parser = TimestampParser::new();
        for y in [
            1, 4, 99, 100, 400, 1582, 1900, 1969, 1970, 2000, 2026, 2028, 2100, 2400, 9999,
        ] {
            for m in 0..=13 {
                for d in 0..=32 {
                    for zone in [
                        "Z", "+00:00", "-00:00", "+23:59", "-23:59", "+24:00", "+05:60",
                    ] {
                        let value = format!("{y:04}-{m:02}-{d:02}T12:34:56{zone}");
                        assert_eq!(
                            parser.parse(value.as_bytes()),
                            timestamp(value.as_bytes()),
                            "{value}"
                        );
                    }
                }
            }
        }
        for value in [
            "",
            "0",
            "0001791071890000",
            "9223372036854775807",
            "9223372036854775808",
            "18446744073709551615",
            "1791071890000.0",
            "-5",
            "١٧٩١٠٧١٨٩٠٠٠٠",
            "2026-10-01t00:00:00Z",
            "2026-10-01T00:00:00Z\r",
        ] {
            assert_eq!(
                parser.parse(value.as_bytes()),
                timestamp(value.as_bytes()),
                "{value}"
            );
        }
        let mut long = vec![b'0'; 6000];
        long.extend_from_slice(b"1791071890000");
        assert_eq!(parser.parse(&long), timestamp(&long));
    }

    #[test]
    fn guaranteed_valid_ingest_fast_path_matches_reference() {
        let mut parser = TimestampParser::new();
        for y in [1, 1900, 1969, 1970, 2000, 2028, 9999] {
            for m in [1, 2, 3, 12] {
                for d in [1, 28] {
                    for h in 0..24 {
                        for (minute, second) in [(0, 0), (1, 1), (29, 30), (59, 59)] {
                            let value =
                                format!("{y:04}-{m:02}-{d:02}T{h:02}:{minute:02}:{second:02}Z");
                            assert_eq!(
                                parser.ingest(value.as_bytes()),
                                timestamp(value.as_bytes()),
                                "{value}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    #[ignore = "explicit bounded timestamp microbenchmark"]
    fn microbench_timestamp_parser() {
        use std::hint::black_box;
        use std::time::Instant;
        let values: Vec<_> = (0..4096)
            .map(|i| {
                format!(
                    "2028-02-{:02}T{:02}:{:02}:{:02}Z",
                    1 + i % 28,
                    i % 24,
                    i % 60,
                    (i * 7) % 60
                )
            })
            .collect();
        for mode in ["reference", "cached", "ingest"] {
            let mut parser = TimestampParser::new();
            let start = Instant::now();
            let mut sum = 0i64;
            for _ in 0..100 {
                for value in &values {
                    let bytes = black_box(value.as_bytes());
                    let parsed = match mode {
                        "reference" => timestamp(bytes),
                        "cached" => parser.parse(bytes),
                        _ => parser.ingest(bytes),
                    };
                    sum = sum.wrapping_add(black_box(parsed.unwrap()));
                }
            }
            eprintln!(
                "timestamp_microbench {mode}: rows={} seconds={:.6} checksum={}",
                values.len() * 100,
                start.elapsed().as_secs_f64(),
                black_box(sum)
            );
        }
    }
}
