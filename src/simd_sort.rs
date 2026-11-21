//! Exact unsigned 128-bit ordering of checked, padding-free packed events.
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Backend {
    Rust,
    #[cfg(vqsort_native)]
    Avx2,
    #[cfg(all(vqsort_native, feature = "vqsort-avx512"))]
    Avx512,
}

#[cfg(vqsort_native)]
unsafe extern "C" {
    fn shopping_sort128_avx2(storage: *mut std::ffi::c_void, count: usize) -> i32;
    #[cfg(feature = "vqsort-avx512")]
    fn shopping_sort128_avx512(storage: *mut std::ffi::c_void, count: usize) -> i32;
}

#[cfg(vqsort_native)]
fn avx2_supported() -> bool {
    // Matches every enabled extension in build.rs, including Highway's SSE4
    // and AVX2 prerequisites. Rust's AVX tests also check OS register support.
    std::is_x86_feature_detected!("avx2")
        && std::is_x86_feature_detected!("bmi1")
        && std::is_x86_feature_detected!("bmi2")
        && std::is_x86_feature_detected!("lzcnt")
        && std::is_x86_feature_detected!("popcnt")
        && std::is_x86_feature_detected!("f16c")
        && std::is_x86_feature_detected!("fma")
        && std::is_x86_feature_detected!("sse4.2")
        && std::is_x86_feature_detected!("aes")
        && std::is_x86_feature_detected!("pclmulqdq")
}

#[cfg(all(vqsort_native, feature = "vqsort-avx512"))]
fn avx512_supported() -> bool {
    avx2_supported()
        && std::is_x86_feature_detected!("avx512f")
        && std::is_x86_feature_detected!("avx512cd")
        && std::is_x86_feature_detected!("avx512dq")
        && std::is_x86_feature_detected!("avx512bw")
        && std::is_x86_feature_detected!("avx512vl")
}

fn select_backend() -> Backend {
    let requested = std::env::var("SHOP_SORT_BACKEND").unwrap_or_default();
    if requested == "rust" {
        return Backend::Rust;
    }
    #[cfg(all(vqsort_native, feature = "vqsort-avx512"))]
    if requested != "avx2" && avx512_supported() {
        return Backend::Avx512;
    }
    // Unsupported wider-vector CPUs retain the checked AVX2 path.
    #[cfg(vqsort_native)]
    if avx2_supported() {
        return Backend::Avx2;
    }
    Backend::Rust
}

fn backend() -> Backend {
    static BACKEND: OnceLock<Backend> = OnceLock::new();
    *BACKEND.get_or_init(select_backend)
}

pub fn backend_name() -> &'static str {
    match backend() {
        Backend::Rust => "rust",
        #[cfg(vqsort_native)]
        Backend::Avx2 => "avx2",
        #[cfg(all(vqsort_native, feature = "vqsort-avx512"))]
        Backend::Avx512 => "avx512",
    }
}

#[cfg(vqsort_native)]
#[repr(C, align(16))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AlignedKey {
    lo: u64,
    hi: u64,
}

/// The caller must exclusively own `count * 16` initialized bytes. They must
/// encode independent two-u64-lane keys with no padding or invalid bitpatterns.
/// A successful call permutes whole records and preserves all their bytes.
pub unsafe fn try_sort_records(storage: *mut u8, count: usize) -> bool {
    unsafe { sort_with(storage, count, backend()) }
}

unsafe fn sort_with(storage: *mut u8, count: usize, selected: Backend) -> bool {
    #[cfg(not(vqsort_native))]
    {
        let _ = (storage, count, selected);
        false
    }
    #[cfg(vqsort_native)]
    {
        if selected == Backend::Rust {
            return false;
        }
        if count < 2 {
            return true;
        }
        let Some(bytes) = count.checked_mul(16).filter(|&n| n <= isize::MAX as usize) else {
            return false;
        };
        let invoke = |pointer: *mut u8| unsafe {
            let result = match selected {
                Backend::Avx2 => shopping_sort128_avx2(pointer.cast(), count),
                #[cfg(feature = "vqsort-avx512")]
                Backend::Avx512 => shopping_sort128_avx512(pointer.cast(), count),
                Backend::Rust => unreachable!(),
            };
            result == 1
        };
        if (storage as usize) & 15 == 0 {
            return invoke(storage);
        }
        // Vec<Event<Packed>> promises only eight-byte alignment. Most system
        // allocators provide 16, but a checked aligned copy handles the rest.
        let mut aligned = Vec::<AlignedKey>::with_capacity(count);
        unsafe {
            std::ptr::copy_nonoverlapping(storage, aligned.as_mut_ptr().cast::<u8>(), bytes);
            aligned.set_len(count);
        }
        if !invoke(aligned.as_mut_ptr().cast()) {
            return false;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(aligned.as_ptr().cast::<u8>(), storage, bytes);
        }
        true
    }
}

#[cfg(all(test, vqsort_native))]
mod tests {
    use super::*;

    fn available() -> Vec<Backend> {
        let mut backends = Vec::new();
        if avx2_supported() {
            backends.push(Backend::Avx2);
        }
        #[cfg(feature = "vqsort-avx512")]
        if avx512_supported() {
            backends.push(Backend::Avx512);
        }
        backends
    }

    fn keys(count: usize, mut state: u64) -> Vec<AlignedKey> {
        let mut random = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        (0..count)
            .map(|i| AlignedKey {
                lo: if i % 11 == 0 { u64::MAX } else { random() },
                hi: match i % 5 {
                    0 => 0,
                    1 => u64::MAX,
                    2 => 7,
                    _ => random(),
                },
            })
            .collect()
    }

    fn check(mut values: Vec<AlignedKey>, selected: Backend) {
        let mut expected = values.clone();
        expected.sort_unstable_by_key(|key| (key.hi, key.lo));
        assert!(unsafe { sort_with(values.as_mut_ptr().cast(), values.len(), selected) });
        assert_eq!(values, expected);
    }

    #[test]
    fn native_sort_matches_unsigned_128_order_and_preserves_payload() {
        for selected in available() {
            for count in [
                0, 1, 2, 3, 7, 15, 16, 31, 32, 63, 64, 127, 128, 129, 1023, 4097, 65537,
            ] {
                let values = keys(count, 818);
                check(values.clone(), selected);
                let mut ordered = values;
                ordered.sort_unstable_by_key(|key| (key.hi, key.lo));
                check(ordered.clone(), selected);
                ordered.reverse();
                check(ordered, selected);
                check(
                    vec![
                        AlignedKey {
                            lo: u64::MAX,
                            hi: u64::MAX
                        };
                        count
                    ],
                    selected,
                );
            }
        }
    }

    #[test]
    fn native_sort_handles_eight_byte_alignment_and_preserves_canaries() {
        for selected in available() {
            let count = 4097;
            let mut buffer = vec![
                AlignedKey {
                    lo: 0xfeed_face_dead_beef,
                    hi: 0xfeed_face_dead_beef
                };
                count + 1
            ];
            let base = buffer.as_mut_ptr().cast::<u8>();
            let unaligned = unsafe { base.add(8) };
            assert_eq!(unaligned as usize & 15, 8);
            let values = keys(count, 97331);
            let mut expected = values.clone();
            expected.sort_unstable_by_key(|key| (key.hi, key.lo));
            unsafe {
                std::ptr::copy_nonoverlapping(values.as_ptr().cast::<u8>(), unaligned, count * 16);
                assert!(sort_with(unaligned, count, selected));
                for (index, expected) in expected.iter().enumerate() {
                    assert_eq!(
                        std::ptr::read_unaligned(unaligned.add(index * 16).cast::<AlignedKey>()),
                        *expected
                    );
                }
            }
            assert_eq!(buffer[0].lo, 0xfeed_face_dead_beef);
            assert_eq!(buffer[count].hi, 0xfeed_face_dead_beef);
        }
    }

    #[test]
    fn native_sort_is_thread_local_and_rust_request_does_not_mutate() {
        for selected in available() {
            std::thread::scope(|scope| {
                for seed in 1..=8 {
                    scope.spawn(move || check(keys(4097, seed), selected));
                }
            });
        }
        let mut values = keys(1023, 17);
        let expected = values.clone();
        assert!(!unsafe { sort_with(values.as_mut_ptr().cast(), values.len(), Backend::Rust) });
        assert_eq!(values, expected);
    }
}
