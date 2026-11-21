<!-- final-round3-report:begin -->
# Final optimized runtime validation

Runtime `bc1ffbcec5dd6620b585d0832ffb29635fa7ad24` passed the [final Linux container CI](https://github.com/kundansai-138/b311e47eb0d89b3de98f6f458a0ad652/actions/runs/44859271562), [controlled benchmark](https://github.com/kundansai-138/b311e47eb0d89b3de98f6f458a0ad652/actions/runs/44859291731) and [60M regression](https://github.com/kundansai-138/b311e47eb0d89b3de98f6f458a0ad652/actions/runs/44859294051).

- Linux native tests: 52 default-feature and 31 no-default-feature correctness tests. The Windows build passed 49 default-feature tests.
- The assembled native candidate passed 101 exact checks, including fixture repetitions and the public development answer comparison.
- Container CI covered 94 unique fixtures: 36 supplied, 44 general adversarial, five small exact-ID cases, one large cross-worker ID case, and eight layout/writer boundary cases. The six ID cases were also checked with eight workers after their one-worker runs.
- The boundary cases cover category counts, exact 32-bit timestamp limits, negative epochs, the complete seven-day click lookback and automatic wide-layout fallback. The 131,077-row writer case matches the independent oracle and, with one worker, produces row groups of 131,072 and 5 rows in one user file.
- Public development output matched the supplied answer: 117,038 user rows and 27,400 campaign rows, with the required schemas and unique keys.
- Five rotated 20M timing runs per image; the complete output from the first run of each image matched across the previous submission, common PGO reference and final runtime.
- The full 60M/15M run completed in 11.086108s under a hard 12 GiB limit. Every output row matched the retained baseline digests; peak cgroup memory was 4,597,018,624 bytes.

The exact-ID regressions include long and Unicode identifiers, NUL/DEL and 9/10-byte boundaries, shared fallback identifiers across files and workers, unused byte-minimum aliases and invalid earliest duplicates. Hash collisions never establish identity. SIMD/scalar parser checks and native-sort comparison tests preserve strict CSV, calendar, numeric and ordering behavior. Packed representations are selected only after checked domain bounds, and click eligibility still includes the full attribution window.

The PGO Docker build runs default and no-default unit tests before instrumentation, trains only on fresh build-local data, and checks the final executable's shared-library dependencies. The runtime runs offline as UID 1000 with a read-only root and 64 MiB `/tmp`; dependency downloads and native compiler tools are confined to image construction. The 290 original supplied fixture files remain unchanged. [Native correctness report](validation/native-round3-tests.json).

Final evidence: [controlled benchmark](validation/linux-round3-benchmark.json) and [60M regression](validation/linux-round3-scale.json). Exploratory evidence: [linux-round3-combined.json](validation/linux-round3-combined.json), [linux-round3-isolated.json](validation/linux-round3-isolated.json), [linux-round3-kernels-initial.json](validation/linux-round3-kernels-initial.json), [linux-round3-kernels-repeat.json](validation/linux-round3-kernels-repeat.json). The exploratory runs used separate runner sessions and are retained as development history; their absolute timings are not combined with the final comparison.

See [PERFORMANCE.md](PERFORMANCE.md) for the same-run timings and resource conditions. These checks do not replace the organizer's hidden scorer or establish a new leaderboard score.
<!-- final-round3-report:end -->

---

## Historical records

# Current compact pipeline validation

Runtime `fca520dab97a01fdc38ad68eeb6916175c4ce738` passed:

- 32 default-feature Rust tests and 24 with no default features; explicit microbenchmarks are excluded.
- All 36 supplied fixtures, 44 general adversarial fixtures, and 5 exact-ID boundary fixtures.
- A 131,072-row independent-oracle stress case with four approximately 7 MiB event files, 65,536 distinct event IDs, 2,259 invalid earliest copies, long/Unicode IDs, NUL/DEL boundaries, and unused canonical aliases. Both 1-worker and 8-worker runs matched all 1,358 user rows and 42 campaign rows.
- Exact public answer comparison: 117,038 user rows and 27,400 campaign rows, valid schemas and unique keys.
- The Linux/amd64 image build and size check, all 86 unique fixtures (the six ID/stress cases are repeated at 1 and 8 workers), and public-dev digests under scorer filesystem restrictions: https://github.com/kundansai-138/shopping-sessions/actions/runs/44852825095
- Five rotated 20M runs of each comparison image with full-table equality.
- The complete 60M regression under 12 GiB with no swap, including exact partition-independent digests for every output row.

The tests force complete event-hash collisions, cover both fallback-remapping domains, check reference-width limits without huge allocations, and compare SIMD/scalar CSV parsing against an independent parser. Length sentinels preserve zero bytes and all nine DEL bytes; hash equality never decides identity. Only guaranteed-valid F1 ingest timestamps use unchecked time-digit arithmetic; event/click timestamps remain strictly validated.

The supplied fixture files are unchanged. See `PERFORMANCE.md` and `validation/linux-compact-*.json` for measurement conditions and results. These checks do not replace the organizer's hidden scorer.

---

## Earlier validation records

# Validation performed on 2026-11-21

## Current native SIMD / Parquet version

The runtime at `7e9ca62d657e29c79eba9a55eb2d37f23155226d` passed:

- 12 Rust correctness tests (the explicit parser microbenchmark is excluded), including SIMD/scalar differential CSV checks, every 32-byte block alignment, long fields, numeric boundaries, and forced event-ID/person hash collisions.
- All 36 supplied fixtures plus the current 44 generated adversarial cases.
- Exact public-development answer comparison: 117,038 user rows and 27,400 campaign rows, correct schemas and unique keys.
- A direct-writer 131,077-row regression across two row groups, covering all-null and mixed-null categories, empty tables, DATE values, and exact Unicode/CR/tab/NUL strings.
- The Linux container build, image-size check, all 80 fixtures under scorer filesystem restrictions, and public-dev digest check: https://github.com/kundansai-138/shopping-sessions/actions/runs/44844658232
- Five alternating 20M runs per implementation: median 8.127s to 6.323s, with exact full-table equality.
- The 60M regression: 19.935s, 5.44 GiB peak cgroup memory under a hard 12 GiB limit, and exact partition-independent original-output digests.

The new scale digests were generated from the retained original validated output before running the new implementation. Their global key order permits a changed hash function or Parquet partition count without changing the reference values. The earlier per-part digests remain available for historical verification.

See `PERFORMANCE.md` and `validation/linux-strategy-*.json` for benchmark conditions and evidence. Later documentation-only commits preserve this runtime.

## Earlier correctness checks

- All 36 supplied fixtures pass with exact row and schema comparison.
- All 44 additional fixtures pass: twelve focused cases and 32 deterministic randomized cases. This includes partial-day manifests and the dense-to-sparse aggregation fallback.
- Public dev input regenerated with the supplied generator, seed 1, 1,000,000 events and 250,000 clicks. Both tables exactly match the supplied `dev-answer.zip`: 117,038 user rows and 27,400 campaign rows. DATE types and unique keys checked.
- Four independent generated datasets, seeds 2, 17, 2028, and 97331, each with 50,000 events and 18,000 clicks, exactly match the independent Python oracle for both tables.
- Seven Rust tests pass, covering strict CSV/CR handling, calendars/offsets, numeric limits, forced person-hash collisions, click-ID tie order, and empty inputs.
- The 20M dataset output is exactly unchanged after parallelizing Parquet export, compared across every row.
- Both large runs have equal user/campaign session and revenue totals; organic assists are zero; total assists are at least credited non-organic sessions.

The additional cases cover a 700-link identity chain, an unused minimum ID, UTF-8 ordering, NUL/tab/CR in valid IDs, 6,000 leading zeroes, oversized epoch values, dedup-before-validation, malformed quoting, exact 24-hour/30-minute/seven-day boundaries, click ties, leap days, midnight sessions, and empty outputs.

## Initial implementation local scale measurements

These measurements describe baseline commit `b012c755514a34b5c4949b9bc315a733bf28e261`, before the later eager-parsing and dense-counter optimizations. Each measurement below is one end-to-end local process run, including Python startup, native processing, and Parquet export. Eight workers; Windows host with 20 logical CPUs and 31.7 GiB RAM; Rust 1.99.0; Python 3.12; PyArrow 24.0.0. Workers were not CPU-affinity pinned. Input was recently generated and may have benefited from the filesystem cache.

| Generated events / clicks | Seed | Input bytes | Wall time | Peak sampled process-tree RSS | Scratch + output |
| --- | --- | ---: | ---: | ---: | ---: |
| 20,000,000 / 5,000,000 | 818 | 1,881,102,028 | 8.483 s | 3,087,056,896 bytes (2.88 GiB) | 135,815,803 bytes |
| 60,000,000 / 15,000,000 | 42 | 5,741,389,006 | 23.932 s | 9,200,410,624 bytes (8.57 GiB) | 398,131,744 bytes |

Generator counts exclude extra dirty/duplicate rows. The 60M run parsed 64,487,770 structurally valid event rows and retained 58,694,027 valid unique events. It wrote 6,658,935 user rows and 28,014 campaign rows. Session totals were 10,067,851 and revenue totals were 20,418,435,163 cents in each table.

RSS was sampled about every 20 ms using psutil, summing the launcher and its children. It excludes kernel file-cache accounting and is not a cgroup memory-limit measurement. Scratch/output totals include small logs and measurement files. Machine-readable measurements are in `validation-results.json`.

## Linux container validation

The optimized Linux/amd64 image passed its build-time seven Rust tests, all 80 fixture cases, the public dev answer digest check, and the image-size check under the requested GitHub account. CI for the scale-tested commit: https://github.com/kundansai-138/shopping-sessions/actions/runs/44841262843

The optimized code also completed an official-generator 60M-event/15M-click dataset under a hard **12 GiB** Linux cgroup memory limit with swap disabled. Container time was **28.998 seconds** with a four-CPU quota and eight workers. Peak cgroup memory was **10.37 GiB**, including charged filesystem cache. Canonical digests covering every output row matched the original implementation. Results: https://github.com/kundansai-138/shopping-sessions/actions/runs/44841265207

The controlled 20M comparison used the same runner for all versions, rotated three runs per image, and compared every output value. Median container time improved from **8.560 s to 7.211 s**, a **15.8% reduction**. See `PERFORMANCE.md` and the JSON files in `validation/`.

The original Windows scale runs above were resource/invariant checks. The later Linux runs added complete baseline comparisons and a constrained memory test. These results are not organizer-provided hidden references or guarantees of Google Cloud scorer timing or leaderboard position. The public repository is https://github.com/kundansai-138/shopping-sessions. Submitting the chosen commit to the competition scorer remains an external step.
