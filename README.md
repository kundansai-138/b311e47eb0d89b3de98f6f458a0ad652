# Shopping Sessions

An implementation of scorer `shopping-sessions-4`, derived from the supplied Polars starter. A dependency-free Rust engine handles the input and aggregation; PyArrow converts the engine's column buffers to Parquet. The supplied 36 fixtures and scorer-style Docker wrapper are retained.

## Build and validate the submission

The repository root contains the Dockerfile. Docker builds need network access to obtain the base images and the pinned PyArrow wheel. Execution needs no network.

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

Use a fresh output folder on each run. The pipeline rejects existing output Parquet or an existing `native-output` scratch folder instead of silently combining results from separate runs. Keep generated datasets, output tables, scratch files, and build artifacts out of Git.

## Run directly without Docker

Requires Rust 1.90 or newer and Python 3.12 with `requirements.txt` installed. The Rust crate has no third-party dependencies.

```sh
cargo build --release --locked --offline
DATA_DIR=/absolute/path/to/data \
OUTPUT_DIR=/absolute/path/to/new-output \
SCRATCH_DIR=/absolute/path/to/new-scratch \
SHOPPING_ENGINE="$PWD/target/release/shopping-sessions" \
NCPU=8 python3 pipeline/pipeline.py
```

For the direct fixture runner, set `SHOPPING_ENGINE` to the absolute executable path; it assigns separate input, output, and scratch directories for each fixture:

```sh
SHOPPING_ENGINE="$PWD/target/release/shopping-sessions" \
python3 scripts/run_fixtures.py fixtures -- python3 pipeline/pipeline.py
cargo test --locked --offline
```

On Windows PowerShell, set the same environment variables with `$env:NAME = 'value'`, and use `target/release/shopping-sessions.exe`.

## Processing strategy

1. Read all link files and resolve connected components with union by size and path compression. Track the bytewise minimum ID separately, including IDs absent from events and clicks.
2. Parse records once with the exact LF-only CSV dialect and precompute validation while the fields are available. Keep invalid copies as tombstones until global earliest-ingest deduplication; only the winning copy can survive. Hash collisions always compare original bytes.
3. Validate timestamp calendars, offsets, windows, prices, event types, and lateness. Group surviving records by canonical person.
4. Parse clicks and sort each person's events/clicks. Build sessions from gaps greater than 30 minutes. The click order selects the latest eligible timestamp and the smallest bytewise click ID at that timestamp.
5. Sweep clicks with a rolling seven-day window and per-campaign counts. Each active campaign contributes one assist per session; no session-to-click Cartesian/range join is materialized.
6. Aggregate campaign days in dense counters when the manifest/catalog dimensions are small, otherwise use sparse counters. Stream binary column buffers to scratch, then release native memory. Export independent Parquet parts concurrently with explicit DATE and nullable category types, without unnecessary dictionaries, compression, or statistics.

Parsed event records occupy 64 bytes, surviving event records 40 bytes, and click records 56 bytes on x86-64. Original CSV buffers remain resident during native execution. This is an in-memory implementation, not a spill-to-disk engine; see `VALIDATION.md` for measured memory and runtime, including their limits.

## Files

- `src/main.rs`: native parser, identity resolution, aggregation, and column writer.
- `pipeline/pipeline.py`: launcher and Parquet exporter.
- `rules.md`: supplied authoritative rules.
- `fixtures/`: supplied input cases and expected answers, preserved byte-for-byte.
- `tests/oracle.py`: independent, deliberately simple Python reference for small inputs.
- `tests/generate_cases.py`: ten focused adversarial cases plus 32 deterministic randomized cases.
- `scripts/`: exact output comparison and scorer-style execution.
- `VALIDATION.md`: measured results and outstanding environment checks.

## Submission

Publish this directory as a public GitHub repository and submit its exact commit SHA on the competition page. Keep every submitted commit available until results are published. The image and hidden scorer performance must be verified in the actual Linux/scoring environment; local timings are not leaderboard scores.
