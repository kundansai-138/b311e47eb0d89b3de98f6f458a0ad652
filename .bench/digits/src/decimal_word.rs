//! Exact decimal conversion with eight-byte chunks. No assumption about
//! timestamp length, digit prefixes, or the number of leading zeros.

#[inline]
fn eight(ascii: u64) -> Option<u32> {
    let digits = ascii.wrapping_sub(0x3030_3030_3030_3030);
    if (digits | ascii.wrapping_add(0x4646_4646_4646_4646)) & 0x8080_8080_8080_8080 != 0 {
        return None;
    }
    let pairs = (digits * 10 + (digits >> 8)) & 0x00ff_00ff_00ff_00ff;
    let quads = (pairs * 100 + (pairs >> 16)) & 0x0000_ffff_0000_ffff;
    Some((quads * 10_000 + (quads >> 32)) as u32)
}

#[inline]
pub fn parse(mut bytes: &[u8]) -> Option<i64> {
    if bytes.is_empty() { return None; }
    while bytes.first() == Some(&b'0') { bytes = &bytes[1..]; }
    if bytes.len() > 19 { return None; }
    let mut value = 0u64;
    if bytes.len() < 8 {
        for &byte in bytes {
            let digit = byte.wrapping_sub(b'0');
            if digit > 9 { return None; }
            value = value * 10 + u64::from(digit);
        }
    } else {
        let head = bytes.len() % 8;
        if head != 0 {
            // At least eight bytes remain. Shift away bytes beyond the short
            // prefix and fill its left side with ASCII zero; no overread.
            let word = u64::from_le_bytes(bytes[..8].try_into().unwrap());
            let padded = (word << ((8-head)*8)) | (0x3030_3030_3030_3030u64 >> (head*8));
            value = u64::from(eight(padded)?);
            bytes = &bytes[head..];
        }
        for chunk in bytes.chunks_exact(8) {
            let word = u64::from_le_bytes(chunk.try_into().unwrap());
            // At most 19 significant digits means this cannot overflow u64.
            value = value * 100_000_000 + u64::from(eight(word)?);
        }
    }
    (value <= i64::MAX as u64).then_some(value as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn baseline(mut bytes: &[u8]) -> Option<i64> {
        if bytes.is_empty() { return None; }
        while bytes.first() == Some(&b'0') {bytes=&bytes[1..];}
        if bytes.len()>19 {return None;}
        let mut n=0u64;
        for &byte in bytes {
            let digit=byte.wrapping_sub(b'0');if digit>9 {return None;}
            n=n*10+u64::from(digit);
        }
        (n<=i64::MAX as u64).then_some(n as i64)
    }
    fn reference(bytes: &[u8]) -> Option<i64> {
        if bytes.is_empty() { return None; }
        let mut value=0i64;
        for &byte in bytes {
            if !byte.is_ascii_digit() { return None; }
            value=value.checked_mul(10)?.checked_add(i64::from(byte-b'0'))?;
        }
        Some(value)
    }
    #[test]
    fn decimal_boundaries_and_arbitrary_zero_prefixes() {
        for number in [0u64,1,9,10,99,100,9_999_999,10_000_000,99_999_999,100_000_000,
            9_999_999_999_999_999,10_000_000_000_000_000,i64::MAX as u64,i64::MAX as u64+1,u64::MAX] {
            for zeros in [0,1,7,8,9,63,4096] {
                let value=format!("{}{number}","0".repeat(zeros));
                assert_eq!(parse(value.as_bytes()),reference(value.as_bytes()),"{number},zeros={zeros}");
            }
        }
        for value in [b"".as_slice(),b"00",b"-5",b"1.79e12",b"1791071890000.0",b"250\r",b" 250",b"250 ","٢٥٠".as_bytes(),"２５０".as_bytes()] {
            assert_eq!(parse(value),reference(value),"{value:?}");
        }
    }
    #[test]
    fn every_byte_mutation_of_each_decimal_length() {
        for len in 1..=25 {
            for digit in [b'0',b'1',b'9'] {
                let mut bytes=vec![digit;len];
                for position in 0..len {for byte in 0..=255 {
                    bytes[position]=byte;
                    assert_eq!(parse(&bytes),reference(&bytes),"{bytes:?}");
                }bytes[position]=digit;}
            }
        }
    }
    #[test]
    fn random_values_and_random_bytes_match_reference() {
        let mut state=0x173054ac81e95122u64;
        for _ in 0..100_000 {
            state^=state<<13;state^=state>>7;state^=state<<17;
            let text=state.to_string();assert_eq!(parse(text.as_bytes()),reference(text.as_bytes()));
            let bytes=state.to_le_bytes();assert_eq!(eight(state).map(i64::from),reference(&bytes));
        }
    }
    #[test]
    #[ignore="bounded decimal microbenchmark"]
    fn microbench() {
        use std::{hint::black_box,time::Instant};
        let values:Vec<_>=(0..100_000u64).map(|n|(1_791_071_890_000+n*371).to_string().into_bytes()).collect();
        for round in 0..5 {for candidate in [round%2==0,round%2!=0] {
            let start=Instant::now();let mut sum=0i64;
            for _ in 0..5 {for value in &values {
                let parsed=if candidate{parse(black_box(value))}else{baseline(black_box(value))};
                sum=sum.wrapping_add(black_box(parsed.unwrap()));
            }}
            eprintln!("digits candidate={candidate} round={round} seconds={:.6} checksum={sum}",start.elapsed().as_secs_f64());
        }}
    }
}
