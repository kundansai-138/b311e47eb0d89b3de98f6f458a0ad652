//! Runtime-checked selection of monomorphized event record layouts.

pub trait RecordLayout: Copy + Send + Sync + 'static {
    const PACKED_SORT_KEY: bool;
    type Time: Copy + Ord + Default + Send + Sync;
    type Category: Copy + Default + Send + Sync;
    fn encode(ts: i64, revenue: u32, category: u32, base: i64)
        -> (Self::Time, u32, Self::Category);
    fn epoch(ts: Self::Time, base: i64) -> i64;
    fn gap(later: Self::Time, earlier: Self::Time) -> i64;
    fn revenue(encoded: u32) -> u32;
    fn category(encoded: u32, category: Self::Category) -> u32;
}

#[derive(Clone, Copy)]
pub struct Wide;
impl RecordLayout for Wide {
    const PACKED_SORT_KEY: bool = false;
    type Time = i64;
    type Category = u32;
    #[inline]
    fn encode(ts: i64, revenue: u32, category: u32, _base: i64) -> (i64, u32, u32) {
        (ts, revenue, category)
    }
    #[inline]
    fn epoch(ts: i64, _base: i64) -> i64 {
        ts
    }
    #[inline]
    fn gap(later: i64, earlier: i64) -> i64 {
        later - earlier
    }
    #[inline]
    fn revenue(encoded: u32) -> u32 {
        encoded
    }
    #[inline]
    fn category(_encoded: u32, category: u32) -> u32 {
        category
    }
}

#[derive(Clone, Copy)]
pub struct Packed;
pub fn can_pack(start: i64, end: i64, categories: usize) -> bool {
    categories <= 256
        && end
            .checked_sub(start)
            .is_some_and(|span| span > 0 && span <= i64::from(u32::MAX))
}
impl RecordLayout for Packed {
    const PACKED_SORT_KEY: bool = true;
    type Time = u32;
    // The category occupies the high byte of revenue, so it needs no field.
    type Category = ();
    #[inline]
    fn encode(ts: i64, revenue: u32, category: u32, base: i64) -> (u32, u32, ()) {
        // Runtime dispatch checks the whole window/category domain; validation
        // establishes ts in that window and the 10,000,000 revenue ceiling.
        debug_assert!(revenue <= 10_000_000 && category < 256);
        let offset = u32::try_from(ts - base).expect("validated timestamp fits packed window");
        (offset, revenue | (category << 24), ())
    }
    #[inline]
    fn epoch(ts: u32, base: i64) -> i64 {
        base + i64::from(ts)
    }
    #[inline]
    fn gap(later: u32, earlier: u32) -> i64 {
        i64::from(later) - i64::from(earlier)
    }
    #[inline]
    fn revenue(encoded: u32) -> u32 {
        encoded & 0x00ff_ffff
    }
    #[inline]
    fn category(encoded: u32, _category: ()) -> u32 {
        encoded >> 24
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_dispatch_limits_and_checked_subtraction() {
        assert!(can_pack(0, u32::MAX as i64, 256));
        assert!(can_pack(-1_000_000, 1_000_000, 1));
        assert!(!can_pack(0, u32::MAX as i64 + 1, 256));
        assert!(!can_pack(0, 1, 257));
        assert!(!can_pack(1, 1, 1));
        assert!(!can_pack(2, 1, 1));
        assert!(!can_pack(i64::MIN, i64::MAX, 1));
    }
    #[test]
    fn packed_payload_preserves_every_category_and_price_extreme() {
        for base in [-62_135_596_800_000i64, 0, 1_791_072_000_000] {
            for offset in [0, 1, 1_800_000, 86_400_001, u32::MAX] {
                for category in 0..256 {
                    for revenue in [0, 1, 10_000_000] {
                        let (ts, encoded, cat) =
                            Packed::encode(base + i64::from(offset), revenue, category, base);
                        assert_eq!(ts, offset);
                        assert_eq!(Packed::epoch(ts, base), base + i64::from(offset));
                        assert_eq!(Packed::revenue(encoded), revenue);
                        assert_eq!(Packed::category(encoded, cat), category);
                    }
                }
            }
        }
    }
}
