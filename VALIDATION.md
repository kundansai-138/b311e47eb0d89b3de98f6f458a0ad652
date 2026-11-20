# Validation performed on 2026-11-21

## Correctness

- All 36 supplied fixtures pass with exact row and schema comparison.
- All 42 additional fixtures pass: ten focused cases and 32 deterministic randomized cases.
- Public dev input regenerated with the supplied generator, seed 1, 1,000,000 events and 250,000 clicks. Both tables exactly match the supplied `dev-answer.zip`: 117,038 user rows and 27,400 campaign rows. DATE types and unique keys checked.
- Four independent generated datasets, seeds 2, 17, 2028, and 97331, each with 50,000 events and 18,000 clicks, exactly match the independent Python oracle for both tables.
- Three Rust parser tests pass, covering strict CSV/CR handling, calendars/offsets, and numeric limits.
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

## Checks still required in the target environment

The initial Linux/amd64 image built successfully in 34 seconds on GitHub Actions using Rust 1.90. Commit `8e5f263e969d7070f8baf5c9d1e55a15579f5c37` subsequently passed both fixture suites and the public dev answer digest check under the requested GitHub account. Both fixture suites (78 cases) passed inside the image with a read-only root, non-root UID, no network, and a 64 MB `/tmp`. The full workflow completed successfully: https://github.com/kundansai-138/shopping-sessions/actions/runs/44839106789. These tiny fixtures used two CPUs and 5 GB RAM; they do not validate full-size cgroup memory behavior. Local native compilation used Rust 1.99.

The 20M and 60M runs are resource/invariant checks, not full comparisons against a hidden reference. Local Windows timings are not predictions or guarantees of Google Cloud scorer timing, hidden-data correctness, image size/build time, or leaderboard position. The public repository is https://github.com/kundansai-138/shopping-sessions. Submitting the chosen commit to the competition scorer remains an external step.
