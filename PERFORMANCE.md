# Compact exact IDs and parallel reduction

Runtime commit `fca520dab97a01fdc38ad68eeb6916175c4ce738` was compared with the previously submitted `05a1cf788aa499b9c2d3fbaf2f03a437695d2749` and the earlier compact-ID candidate on one GitHub Actions Linux runner. The official generator produced seed 818 with 20,000,000 events and 5,000,000 clicks. Five runs per image rotated order; all used eight worker threads, a four-CPU quota, 12 GiB memory, no swap/network, UID 1000, a read-only root and 64 MiB `/tmp`. Docker start-to-finish times include output writing and process exit.

| Version | Run 1 | Run 2 | Run 3 | Run 4 | Run 5 | Median |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Previous submission `05a1cf7` | 5.197s | 5.231s | 5.226s | 5.279s | 5.165s | **5.226s** |
| Compact-ID reference | 3.124s | 3.211s | 3.139s | 3.208s | 3.281s | **3.208s** |
| Current production engine | 2.905s | 2.908s | 2.867s | 2.898s | 2.966s | **2.905s** |

The production median is **44.4% lower than the previous submission (1.799x speedup)**. All 2,245,254 user rows and 28,014 campaign rows match across the three images. Results: https://github.com/kundansai-138/shopping-sessions/actions/runs/44852512536

The engine uses reversible keys for short ASCII IDs and full-byte dictionaries for other IDs, 40-byte raw records, 24-byte surviving events, 40-byte clicks, and direct deduplication of worker-owned chunks. Exact token equality handles hash collisions. A validated date cache, row-local CSV fields, independent person partitions, and bounded capacity hints reduce work while retaining all required checks. Non-purchase events skip price conversion/product lookup; invalid earliest duplicates remain tombstones. Larger inputs use up to 16 partitions per worker. Each input file is mapped once. Lazy mapping and alternative hashes did not establish a useful end-to-end gain, so the existing prefaulted mapping and non-AES hash approach remain.

The full seed 42 regression with 60,000,000 events and 15,000,000 clicks completed in **12.113s** using eight workers, a four-CPU quota, and a hard 12 GiB limit. Peak cgroup memory was **3,578,023,936 bytes (3.33 GiB)**; output plus scratch used **287,424,375 bytes**. Every row matched the partition-independent original-output digests: 6,658,935 user rows and 28,014 campaign rows. This independent run measures resources and correctness, not a controlled 60M speedup. Results: https://github.com/kundansai-138/shopping-sessions/actions/runs/44852515400

Evidence is in `validation/linux-compact-*.json`. These are GitHub runner timings, not leaderboard scores. Only the competition scorer can establish the new score; small differences between runners are not comparable. The latest documentation-only commit preserves this runtime exactly.

---

## Previous optimization rounds

# Native SIMD and Parquet optimization

Runtime commit `7e9ca62d657e29c79eba9a55eb2d37f23155226d` was compared with the previously submitted `38cd60db715d1ecb9434e58578ae90fff4c8fd35`. Both ran on the same GitHub Actions Ubuntu runner against official-generator seed 818: 20,000,000 events and 5,000,000 clicks. Five runs per image alternated order, with four CPUs/workers, 12 GiB memory, no swap/network, UID 1000, read-only root, and 64 MiB `/tmp`. Docker start-to-finish durations include the complete process lifetime and output writing.

| Version | Run 1 | Run 2 | Run 3 | Run 4 | Run 5 | Median |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Previously submitted | 8.001s | 8.127s | 8.104s | 8.141s | 8.141s | **8.127s** |
| Native SIMD / mapped inputs / Parquet | 6.442s | 6.269s | 6.172s | 6.323s | 6.543s | **6.323s** |

The median runtime is **22.2% lower (1.285x speedup)**. All 2,245,254 user rows and 28,014 campaign rows match exactly after normalizing the accepted string representation. Results: https://github.com/kundansai-138/shopping-sessions/actions/runs/44844674470

The implementation retains strict CSV/calendar validation and full-byte hash-collision checks. Changes are fused AVX2 delimiter/quote scanning with a tested scalar fallback, word-at-a-time hashing, bounded integer parsing, immutable memory-mapped inputs, and direct bounded-row-group Parquet output in Rust. The image no longer needs Python or Arrow at runtime. Comparison sorts remain: compact/radix experiments did not establish a reliable end-to-end advantage, and noisy local timings were not used to select them. No dataset IDs, timestamps, row counts, or outputs are hardcoded into execution.

The full 60,000,000-event / 15,000,000-click seed42 regression completed in **19.935s**, with four CPUs, eight workers, and a hard 12 GiB memory limit. Peak cgroup memory was **5,842,042,880 bytes (5.44 GiB)**, including charged filesystem cache. Output plus scratch used **287,369,921 bytes**. Every output row matched a partition-independent canonical digest of the retained original validated output: 6,658,935 user rows and 28,014 campaign rows. This is an independent-run resource check, not a controlled speedup comparison with the historical 60M run. Results: https://github.com/kundansai-138/shopping-sessions/actions/runs/44844677475

Machine-readable evidence is in `validation/linux-strategy-benchmark.json` and `validation/linux-strategy-scale.json`. These GitHub runner timings are not leaderboard scores. The competition uses different hardware; the new score requires submitting the new commit.

---

## Historical comparisons

# Controlled Linux comparison

The optimized code at commit `ac89d3274be54a70672cb8cf4fc2d3d1f697e6f8` was compared with the original implementation and the intermediate eager-parsing version on one GitHub Actions Ubuntu runner. The generated input used seed 818 with 20,000,000 events and 5,000,000 clicks.

All images ran with a 4-CPU quota, four worker threads, a 12 GiB memory limit, no swap, no network, UID 1000, read-only root, and 64 MiB `/tmp`. The order rotated across three runs of each image. Durations use Docker's container start and finish timestamps, including native processing, Python/Arrow startup, and Parquet writing.

| Version | Run 1 | Run 2 | Run 3 | Median |
| --- | ---: | ---: | ---: | ---: |
| Original `b012c75` | 8.840s | 8.560s | 8.559s | **8.560s** |
| Eager parsing + dense counters `6072e58` | 7.615s | 7.558s | 7.395s | **7.558s** |
| Exact collision-checked sort `ac89d32` | 7.218s | 7.211s | 6.857s | **7.211s** |

The final median is 15.8% lower than the original implementation. All 2,245,254 user rows and 28,014 campaign rows matched exactly across all three implementations. The optimized image also passed 80 fixtures, the public dev answer check, and seven Rust unit tests.

Results: https://github.com/kundansai-138/shopping-sessions/actions/runs/44840582741

Machine-readable results are in `validation/linux-benchmark.json`. These are controlled comparisons on one GitHub runner, not competition scorer results. The scorer uses different CPUs and an 8-vCPU allocation, so its absolute timing can differ.
## Full-size Linux regression

The same optimized runtime completed the official generator's seed 42 dataset with 60,000,000 events and 15,000,000 clicks in **28.998 seconds**. This was one run with a four-CPU quota, eight worker threads, a hard 12 GiB memory limit, and no swap. Peak cgroup memory, including charged filesystem cache, was **11,138,543,616 bytes (10.37 GiB)**. Output plus scratch used 650,019,463 bytes (0.61 GiB).

All 6,658,935 user rows and 28,014 campaign rows matched canonical SHA-256 digests from the original fixture/dev-validated implementation. This is a full-size regression comparison, not an organizer-provided hidden reference.

The test used code at commit `4035e6ab9d8ece910f9e00acb040d180ca292cdd`; its runtime source is identical to `ac89d32`. Results: https://github.com/kundansai-138/shopping-sessions/actions/runs/44841265207

Machine-readable results are in `validation/linux-scale.json`. The competition's eight-vCPU Xeon hardware can produce different timing.
