# Shopping Sessions

<!-- final-round4-report:begin -->
## Latest validated runtime

Runtime `25f02ed9dc93fc021d340917d05c40785d7a1fe1`: controlled 20M median **2.652034s**, versus **2.752569s** for `55c31f8`; **3.65% lower than the previous submission on the same runner**.

The selected Rust 1.100.0 PGO runtime uses compact CSV separator masks, bounded short-ID loads, and exact integer-key catalog dictionaries. Packed-event sorting selects the C++ AVX512 kernel when every required CPU/OS feature is available, with checked AVX2 and Rust fallbacks. The simpler AVX2 CSV iterator was retained after the fused/wider parser performed similarly in production comparisons. Independent workers, global earliest-ingest deduplication, strict validation, and exact fallback identifiers remain intact.

The user reported 1.68 seconds for the preceding submission on the leaderboard. These GitHub runner timings do not establish a new leaderboard score.

Evidence: [linux-round4-benchmark.json](validation/linux-round4-benchmark.json) and [linux-round4-scale.json](validation/linux-round4-scale.json). Exploratory and rejected experiments: [linux-round4-benchmark-amd.json](validation/linux-round4-benchmark-amd.json), [round4-experiments.json](validation/round4-experiments.json), [round4-screen-analysis.json](validation/round4-screen-analysis.json). Workflow records: [benchmark](https://github.com/kundansai-138/b311e47eb0d89b3de98f6f458a0ad652/actions/runs/44867782921), [scale](https://github.com/kundansai-138/b311e47eb0d89b3de98f6f458a0ad652/actions/runs/44867804242), [CI](https://github.com/kundansai-138/b311e47eb0d89b3de98f6f458a0ad652/actions/runs/44867781484).
<!-- final-round4-report:end -->

---

## Existing build and runtime documentation


Public submission repository: https://github.com/kundansai-138/b311e47eb0d89b3de98f6f458a0ad652

An implementation of scorer `shopping-sessions-4`, derived from the supplied Polars starter. A Rust engine handles parsing, aggregation, and direct Apache Parquet output; checked C++ AVX2/AVX512 kernels accelerate packed-event sorting. Immutable input files are memory-mapped, and a runtime-selected vector scanner identifies CSV delimiters and quotes in one pass. The supplied 36 fixtures and scorer-style Docker wrapper are retained.

## Build and validate the submission

The repository root contains the Dockerfile. The default image uses Rust 1.100.0 profile-guided compilation: an instrumented build processes three independently generated training datasets and small correctness fixtures, then a fresh optimized executable is compiled with the merged profile. Training inputs, profiles, compilers and test outputs remain in builder stages. The runtime contains only the executable and launcher on Debian slim.

Builds need network access for base images, pinned Cargo dependencies and the matching LLVM profile tool. The vendored C++ kernel needs no download. Execution needs no network or Python. Python/PyArrow are used only by external validation scripts. `docker build --target normal` produces an ordinary release control from the same source.

```sh
docker buildx build --platform linux/amd64 --load -t shopping-sessions:latest .
python3 -m pip install -r requirements.txt
python3 scripts/run_fixtures.py fixtures -- sh scripts/docker_run.sh
python3 tests/generate_cases.py --output scratch/adversarial
python3 scripts/run_fixtures.py scratch/adversarial -- sh scripts/docker_run.sh
python3 tests/generate_layout_cases.py --output scratch/layout-cases --writer
CPUS=1 python3 scripts/run_fixtures.py scratch/layout-cases -- sh scripts/docker_run.sh
```

The wrapper applies a read-only root, UID 1000, no network, a 64 MB `/tmp`, eight CPUs, 24 GB memory with no swap, and separate data/output/scratch mounts. Override the CPU quota with `CPUS`, memory with `MEM`, and worker count with `WORKERS` (defaults to the integer CPU quota). The GitHub Actions workflow builds and tests the Linux container using smaller quotas; a successful local native run alone does not certify the container.

Generate the public development input using the bundled copy of the supplied generator, then compare with the separately supplied answer:

```sh
cargo build --release --locked --offline --manifest-path tests/generator/Cargo.toml
tests/generator/target/release/generate --seed 1 --events 1000000 --clicks 250000 --output dev
sh scripts/docker_run.sh dev out
python3 scripts/compare.py out dev-answer
# Alternatively, verify canonical SHA-256 digests of the public answer:
python3 tests/check_dev.py out
```

Use a fresh output folder on each run. The engine rejects existing output Parquet. Keep input files unchanged for the entire run: memory-mapped slices borrow their contents, and the scorer supplies an immutable read-only `/data` mount. Keep generated datasets, output tables, scratch files, and build artifacts out of Git.

## Run directly without Docker

Requires Rust 1.90 or newer, plus g++ with C++20 support and ar on Linux x86-64. Fetch dependencies once during the build; the executable runs offline and writes Parquet directly. Python 3.12 with `requirements.txt` is needed only for comparison/fixture scripts.

```sh
cargo build --release --locked
DATA_DIR=/absolute/path/to/data \
OUTPUT_DIR=/absolute/path/to/new-output \
SCRATCH_DIR=/absolute/path/to/new-scratch \
NCPU=8 ./target/release/shopping-sessions

python3 scripts/run_fixtures.py fixtures -- ./target/release/shopping-sessions
cargo test --release --locked --offline
cargo run --release --locked --offline --example writer_smoke -- scratch/writer-smoke
python3 tests/check_native_writer.py scratch/writer-smoke
```

The optional `pipeline/pipeline.py` convenience launcher executes the native binary from `target/release`, or from the absolute path in `SHOPPING_ENGINE`. It does not import Arrow or export intermediate files.

Windows uses the validated Rust sort fallback. On Windows PowerShell, set the same environment variables with `$env:NAME = 'value'`, and use `target/release/shopping-sessions.exe`.

## Processing strategy

1. Map each immutable input file once and distribute newline-aligned chunks among workers. Resolve linked identities in separate interning, union and canonicalization passes. Checked compact union metadata, path compression and cached canonical keys preserve the bytewise minimum across every linked ID, including unused aliases.
2. Represent IDs of one to nine ASCII bytes exactly in a reversible 64-bit key. Length markers preserve NULs, leading zeroes, whitespace, and case. Catalog keys use the same exact codec, with bounded short loads instead of variable-size copies. Longer/non-ASCII IDs use exact byte-keyed dictionaries and worker-to-global remaps; neither path assumes a generator-specific ID format.
3. Scan commas, LF bytes, and quotes with runtime-checked vector masks, retaining the exact streaming/scalar parser for long rows, tails and unsupported CPUs. Keep only the numeric fields needed downstream. Non-purchase rows skip price conversion and product lookup. Invalid event copies remain tombstones until global earliest-ingest deduplication. Compact hash slots reference worker-owned records directly, avoiding a complete raw-record copy; exact ID tokens decide equality even when hashes collide.
4. Cache strictly validated calendar dates using all ten original date bytes. Decode decimal and clock digits in bounded machine-word groups, preserving arbitrary leading zeroes and overflow checks. Event/click timestamps retain full validation; only `ingest_ts`, guaranteed valid F1 by the rules, uses direct time decoding. Repartition survivors by canonical person, then sort by exact person key and timestamp. Packed events use the pinned Google Highway C++ AVX2/AVX512 sorter after CPU, layout and alignment checks; Wide records and unsupported platforms use Rust sorting. Sessions split at gaps greater than 30 minutes. Click ties retain the smallest original bytewise click ID.
5. Sweep clicks with a rolling seven-day window and per-campaign counts. Each active campaign contributes one assist per session; no session-to-click Cartesian/range join is materialized.
6. Reuse one campaign accumulator and one Parquet writer per worker across its partitions. Campaign counters are dense when the manifest/catalog dimensions are small and sparse otherwise. Write Parquet directly, using bounded 131,072-row groups, explicit DATE and nullable category types, and plain uncompressed encoding without dictionaries or statistics. No intermediate column files are needed.

When the complete event-plus-click-lookback domain fits u32 milliseconds and the catalog has at most 256 categories including unknown, parsed events occupy 32 bytes, surviving events 16 bytes, and clicks 32 bytes on x86-64. Wider domains or larger catalogs automatically use the exact 40/24/40-byte layout. Ingest timestamps remain i64 in both paths; the compact layout never changes deduplication or validation semantics. Large inputs use up to 16 independent partitions per worker; tiny inputs use fewer. Capacity estimates affect allocation only, and all buffers can grow. Read-only CSV mappings remain valid throughout native execution. This is an in-memory implementation; see `PERFORMANCE.md` for the controlled Linux comparison and `VALIDATION.md` for correctness and resource checks.

## Files

- `src/main.rs`: native parsing, identity resolution, deduplication, sessionization, and aggregation.
- `src/row_csv.rs` and `src/fast_csv.rs`: whole-row masks and exact streaming/scalar CSV parsing with differential tests.
- `src/inline_person.rs` and `src/event_ids.rs`: reversible ID keys and exact fallback remapping.
- `src/timestamp_parser.rs`, `src/clock_word.rs`, `src/decimal_word.rs`: exact calendar, clock and decimal decoding.
- `src/record_layout.rs`: checked compact/full-width record layouts.
- `src/simd_sort.rs`, `native/`, `build.rs`: checked Rust/C++ sorting boundary.
- `vendor/highway/`: pinned, unmodified Highway headers and license.
- `build/build.sh`: reproducible unit checks, profile training and optimized compilation.
- `src/parquet_output.rs`: bounded-memory direct Parquet writers.
- `pipeline/pipeline.py`: optional native executable launcher.
- `rules.md`: supplied authoritative rules.
- `fixtures/`: supplied input cases and expected answers, preserved byte-for-byte.
- `tests/oracle.py`: independent, deliberately simple Python reference for small inputs.
- `tests/generate_cases.py`: twelve focused adversarial cases plus 32 deterministic randomized cases.
- `tests/generate_id_cases.py`: exact ID boundary cases and a cross-file, cross-worker fallback stress test.
- `scripts/`: exact output comparison and scorer-style execution.
- `VALIDATION.md`: correctness and resource verification.
- `PERFORMANCE.md`: controlled Linux comparisons and full-scale measurements.

## Submission

Publish this directory as a public GitHub repository and submit its exact commit SHA on the competition page. Keep every submitted commit available until results are published. The image and hidden scorer performance must be verified in the actual Linux/scoring environment; local timings are not leaderboard scores.

For diagnostic comparisons, `SHOP_SORT_BACKEND=avx2` selects the AVX2 native sorter. The sorter otherwise selects a supported wider path automatically; the scanner uses AVX2 and retains its exact scalar fallback. The Docker wrapper forwards these optional variables.
