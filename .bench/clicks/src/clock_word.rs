//! Exact HH:MM:SS decoding from one eight-byte word. This helper does not
//! handle calendars or zones; callers retain their existing strict checks.
#[inline]
pub fn parse(word: u64) -> Option<i64> {
    const SEPARATORS: u64 = 0x0000_ff00_00ff_0000;
    const COLONS: u64 = 0x0000_3a00_003a_0000;
    if word & SEPARATORS != COLONS { return None; }
    // Substitute ASCII zero at the two known colons. The two carry tests
    // together reject every non-ASCII-digit in the remaining six bytes.
    let ascii = (word & !SEPARATORS) | 0x0000_3000_0030_0000;
    if (ascii.wrapping_sub(0x3030_3030_3030_3030)
        | ascii.wrapping_add(0x4646_4646_4646_4646)) & 0x8080_8080_8080_8080 != 0 {
        return None;
    }
    let digits = word - 0x3030_3a30_303a_3030;
    let h = (digits & 255) * 10 + ((digits >> 8) & 255);
    let m = ((digits >> 24) & 255) * 10 + ((digits >> 32) & 255);
    let s = ((digits >> 48) & 255) * 10 + (digits >> 56);
    if h > 23 || m > 59 || s > 59 { return None; }
    Some((h * 3600 + m * 60 + s) as i64 * 1000)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn reference(bytes: &[u8;8]) -> Option<i64> {
        if bytes[2]!=b':' || bytes[5]!=b':' {return None;}
        let pair=|i:usize| -> Option<i64> {
            let a=bytes[i].wrapping_sub(b'0');let b=bytes[i+1].wrapping_sub(b'0');
            (a<=9&&b<=9).then_some(a as i64*10+b as i64)
        };
        let h=pair(0)?;let m=pair(3)?;let s=pair(6)?;
        if h>23||m>59||s>59 {None} else {Some((h*3600+m*60+s)*1000)}
    }
    #[test]
    fn every_time_and_single_byte_mutation_is_exact() {
        for h in 0..=25 {for m in 0..=61 {for s in 0..=61 {
            let bytes:[u8;8]=format!("{h:02}:{m:02}:{s:02}").as_bytes().try_into().unwrap();
            assert_eq!(parse(u64::from_le_bytes(bytes)),reference(&bytes));
        }}}
        for original in [*b"00:00:00",*b"23:59:59",*b"12:34:56"] {
            for pos in 0..8 {for byte in 0..=255 {
                let mut bytes=original;bytes[pos]=byte;
                assert_eq!(parse(u64::from_le_bytes(bytes)),reference(&bytes),"{bytes:?}");
            }}
        }
    }
    #[test]
    fn random_words_match_independent_reference() {
        let mut state=0x749a04ed13c42861u64;
        for _ in 0..100_000 {
            state^=state<<13;state^=state>>7;state^=state<<17;
            let bytes=state.to_le_bytes();
            assert_eq!(parse(state),reference(&bytes),"{bytes:?}");
        }
    }
    #[test]
    #[ignore="bounded clock microbenchmark"]
    fn microbench() {
        use std::{hint::black_box,time::Instant};
        let values:Vec<_>=(0..86400).map(|n|{
            let s=format!("{:02}:{:02}:{:02}",n/3600,n/60%60,n%60);
            let bytes:[u8;8]=s.as_bytes().try_into().unwrap();bytes
        }).collect();
        for round in 0..5 {for candidate in [round%2==0,round%2!=0] {
            let start=Instant::now();let mut sum=0i64;
            for _ in 0..10 {for value in &values {
                let result=if candidate{parse(u64::from_le_bytes(*black_box(value)))}else{reference(black_box(value))};
                sum+=black_box(result.unwrap());
            }}
            eprintln!("clock candidate={candidate} round={round} seconds={:.6} checksum={sum}",start.elapsed().as_secs_f64());
        }}
    }
}
