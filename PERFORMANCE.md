<!-- final-round3-report:begin -->
# Final checked layouts, native sorting and PGO

Runtime `bc1ffbcec5dd6620b585d0832ffb29635fa7ad24` was compared with the previous submission `d52cca7d96130444086754510f8e298d5390eb84` and the common round-three PGO reference `992b7672b54988c773f6d78cb260b3af9a370c7e` on one GitHub Actions Linux runner. Official-generator seed 818 produced 20,000,000 events and 5,000,000 clicks. Five runs per image rotated order, with a four-CPU quota, eight worker threads, a hard 12 GiB memory limit and no swap or network. Each container ran as UID 1000 with a read-only root and 64 MiB `/tmp`. Docker start-to-finish timings include processing, output writing and process exit.

| Version | Run 1 | Run 2 | Run 3 | Run 4 | Run 5 | Median |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Previous submission `d52cca7` | 3.705568s | 3.546410s | 3.731264s | 3.630974s | 3.586045s | **3.630974s** |
| Common round-three PGO reference `992b767` | 2.875585s | 2.838834s | 2.922628s | 2.849632s | 2.836242s | **2.849632s** |
| Final optimized runtime `bc1ffbc` | 2.842747s | 2.784478s | 2.845868s | 2.831043s | 2.764465s | **2.831043s** |

The final median is **22.03% lower than the previous submission (1.283x speedup)** and **0.65% lower than the common PGO reference (1.007x speedup)**. These reductions use only the same-run medians above. The small difference from the PGO reference measures the combined implementation, not each component independently. Every output row matched across all three images. [Final controlled benchmark](https://github.com/kundansai-138/b311e47eb0d89b3de98f6f458a0ad652/actions/runs/44859291731).

Checked timestamp and category domains select 32-byte raw records, 16-byte surviving events and 32-byte clicks. The check includes the full seven-day click lookback; wider domains automatically use the original 40/24/40-byte representation. Exact inline IDs, full-byte fallback dictionaries, invalid-earliest tombstones and bytewise canonical-name minima remain intact.

The implementation adds phased identity construction, whole-row CSV scanning, word-at-a-time clock and decimal parsing, reusable campaign reducers and one bounded Parquet writer per worker. A vendored C++ AVX2 sorter handles eligible packed records after runtime feature checks; the Rust sort remains the fallback. Build-time profile-guided optimization uses Rust 1.90.0, fixed generated training seeds and small boundary fixtures, with fresh profiles and separate uninstrumented unit checks. The deployed image contains no training data or Python runtime.

The production image build step took **113 seconds**; the resulting image was **28,818,074 bytes (27.48 MiB)**. These are the recorded workflow build and image measurements; the workflow enforces a 600-second build limit.

The independent seed 42 run with 60,000,000 events and 15,000,000 clicks completed in **11.086108s**, using four CPUs, eight workers and the same hard 12 GiB limit. Peak cgroup memory was **4,597,018,624 bytes (4.28 GiB)**; output plus scratch used **287,359,560 bytes**. Every output row matched the retained baseline's partition-independent digests. This is a resource and correctness regression, not a controlled 60M speedup comparison. [Final scale run](https://github.com/kundansai-138/b311e47eb0d89b3de98f6f458a0ad652/actions/runs/44859294051).

Final evidence: [controlled benchmark](validation/linux-round3-benchmark.json) and [60M regression](validation/linux-round3-scale.json). Exploratory evidence: [linux-round3-combined.json](validation/linux-round3-combined.json), [linux-round3-isolated.json](validation/linux-round3-isolated.json), [linux-round3-kernels-initial.json](validation/linux-round3-kernels-initial.json), [linux-round3-kernels-repeat.json](validation/linux-round3-kernels-repeat.json). The exploratory runs used separate runner sessions and are retained as development history; their absolute timings are not combined with the final comparison.

The user reported a 2.23-second leaderboard result for the preceding submission. The measurements here come from the four-CPU GitHub runner and do not establish a new leaderboard score.
<!-- final-round3-report:end -->

---

## Historical records

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
