# Shopping Sessions

Public submission repository: https://github.com/kundansai-138/shopping-sessions

An implementation of scorer `shopping-sessions-4`, derived from the supplied Polars starter. A native Rust engine handles parsing, aggregation, and direct Apache Parquet output. Immutable input files are memory-mapped, and an AVX2 scanner identifies CSV delimiters and quotes in one pass. The supplied 36 fixtures and scorer-style Docker wrapper are retained.

## Build and validate the submission

The repository root contains the Dockerfile. Docker builds need network access to obtain base images and the Cargo.lock-pinned Rust dependencies. Execution needs no network or Python. Python/PyArrow are used only by the external validation scripts.

```sh
docker buildx build --platform linux/amd64 --load -t shopping-sessions:latest .
python3 -m pip install -r requirements.txt
python3 scripts/run_fixtures.py fixtures -- sh scripts/docker_run.sh
python3 tests/generate_cases.py --output scratch/adversarial
python3 scripts/run_fixtures.py scratch/adversarial -- sh scripts/docker_run.sh
```

The wrapper applies a read-only root, UID 1000, no network, a 64 MB `/tmp`, eight CPUs, 24 GB memory with no swap, and separate data/output/scratch mounts. Its resource defaults can be overridden with `CPUS` and `MEM`. The GitHub Actions workflow builds and tests the Linux container using smaller limits for the tiny fixture datasets; a successful local native run alone does not certify the container.

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

Requires Rust 1.90 or newer. Fetch dependencies once during the build; the executable runs offline and writes Parquet directly. Python 3.12 with `requirements.txt` is needed only for comparison/fixture scripts.

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

On Windows PowerShell, set the same environment variables with `$env:NAME = 'value'`, and use `target/release/shopping-sessions.exe`.

## Processing strategy

1. Map each immutable input file once and distribute newline-aligned chunks among workers. Resolve linked identities with union by size and path compression, tracking the bytewise minimum across every linked ID, including unused aliases.
2. Represent IDs of one to nine ASCII bytes exactly in a reversible 64-bit key. Length markers preserve NULs, leading zeroes, whitespace, and case. Longer/non-ASCII IDs use exact byte-keyed dictionaries and worker-to-global remaps; neither path assumes a generator-specific ID format.
3. Scan commas, LF bytes, and quotes together using AVX2 with a tested scalar fallback. Keep only the numeric fields needed downstream. Non-purchase rows skip price conversion and product lookup. Invalid event copies remain tombstones until global earliest-ingest deduplication. Compact hash slots reference worker-owned records directly, avoiding a complete raw-record copy; exact ID tokens decide equality even when hashes collide.
4. Cache strictly validated calendar dates using all ten original date bytes. Event/click timestamps retain full validation; only `ingest_ts`, guaranteed valid F1 by the rules, uses direct time decoding. Repartition survivors by canonical person, then sort by exact person key and timestamp. Sessions split at gaps greater than 30 minutes. Click ties retain the smallest original bytewise click ID.
5. Sweep clicks with a rolling seven-day window and per-campaign counts. Each active campaign contributes one assist per session; no session-to-click Cartesian/range join is materialized.
6. Aggregate campaign days in dense counters when the manifest/catalog dimensions are small, otherwise use sparse counters. Write Parquet parts directly from parallel partition workers, using bounded 131,072-row groups, explicit DATE and nullable category types, and plain uncompressed encoding without dictionaries or statistics. No intermediate column files are needed.

Parsed event records occupy 40 bytes, surviving event records 24 bytes, and click records 40 bytes on x86-64. Large inputs use up to 16 independent partitions per worker; tiny inputs use fewer. Capacity estimates affect allocation only, and all buffers can grow. Read-only CSV mappings remain valid throughout native execution. This is an in-memory implementation; see `PERFORMANCE.md` for the controlled Linux comparison and `VALIDATION.md` for correctness and resource checks.

## Files

- `src/main.rs`: native parsing, identity resolution, deduplication, sessionization, and aggregation.
- `src/fast_csv.rs`: fused SIMD/scalar strict CSV scanner and differential tests.
- `src/inline_person.rs` and `src/event_ids.rs`: reversible ID keys and exact fallback remapping.
- `src/timestamp_parser.rs`: strictly keyed calendar cache and ingest fast path.
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
