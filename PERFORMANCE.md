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
