#!/usr/bin/env python3
"""Run the native engine, then wrap its column buffers in Parquet tables."""
from __future__ import annotations
import os
from pathlib import Path
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor

SCHEMAS = {
    "user_daily": {
        "user_id": "string", "day": "date", "sessions": "integer",
        "events": "integer", "revenue_cents": "integer", "top_category": "nullable_string",
    },
    "campaign_daily": {
        "campaign_id": "string", "channel": "string", "day": "date",
        "sessions": "integer", "purchasing_sessions": "integer",
        "revenue_cents": "integer", "assisted_sessions": "integer",
    },
}

def export_parquet(native: Path, output: Path, threads: int) -> None:
    import pyarrow as pa
    import pyarrow.parquet as pq
    pa.set_cpu_count(threads)
    pa.set_io_thread_count(min(threads, 4))

    def buffer(path: Path):
        if path.stat().st_size == 0:
            return pa.py_buffer(b"")
        with pa.memory_map(str(path), "r") as source:
            return source.read_buffer()

    for table_name, columns in SCHEMAS.items():
        destination = output / table_name
        destination.mkdir(parents=True, exist_ok=True)
        parts = sorted((native / table_name).glob("part-*"))
        if not parts:
            raise RuntimeError(f"native engine produced no parts for {table_name}")
        def convert_part(item):
            i, part = item
            count = int((part / "count.txt").read_text(encoding="ascii"))
            arrays = []
            for name, kind in columns.items():
                if kind in ("string", "nullable_string"):
                    validity = buffer(part / f"{name}.valid") if kind == "nullable_string" else None
                    arrays.append(pa.Array.from_buffers(
                        pa.large_string(), count,
                        [validity, buffer(part / f"{name}.offsets"), buffer(part / f"{name}.data")],
                    ))
                else:
                    typ, extension = (pa.date32(), "i32") if kind == "date" else (pa.int64(), "i64")
                    arrays.append(pa.Array.from_buffers(typ, count, [None, buffer(part / f"{name}.{extension}")]))
            table = pa.Table.from_arrays(arrays, names=list(columns))
            pq.write_table(table, destination / f"part-{i:04d}.parquet",
                           compression="none", use_dictionary=False, write_statistics=False,
                           row_group_size=262_144)
            return count
        if len(parts) == 1:
            rows = convert_part((0, parts[0]))
        else:
            with ThreadPoolExecutor(max_workers=min(threads, len(parts))) as pool:
                rows = sum(pool.map(convert_part, enumerate(parts)))
        print(f"{table_name}: {rows} rows in {len(parts)} parts", file=sys.stderr)

def main() -> None:
    start = time.monotonic()
    root = Path(__file__).resolve().parents[1]
    scratch = Path(os.environ.get("SCRATCH_DIR", "/scratch")).resolve()
    output = Path(os.environ.get("OUTPUT_DIR", "/output")).resolve()
    scratch.mkdir(parents=True, exist_ok=True)
    if (scratch / "native-output").exists():
        raise FileExistsError("Use a fresh SCRATCH_DIR: native-output already exists")
    for table in SCHEMAS:
        if any((output / table).glob("*.parquet")):
            raise FileExistsError(f"Use a fresh OUTPUT_DIR: {table} already contains Parquet files")
    threads = max(1, int(os.environ.get("NCPU", "8")))
    executable = os.environ.get("SHOPPING_ENGINE", str(root / "bin" / "shopping-sessions"))
    engine = subprocess.Popen([executable])
    try:
        # Import Arrow while the native engine starts; table buffers are opened
        # only after native memory has been released.
        import pyarrow  # noqa: F401
        import pyarrow.parquet  # noqa: F401
        returncode = engine.wait()
        if returncode:
            raise subprocess.CalledProcessError(returncode, [executable])
    except BaseException:
        if engine.poll() is None:
            engine.terminate()
            engine.wait()
        raise
    export_parquet(scratch / "native-output", output, threads)
    print(f"pipeline including Parquet: {time.monotonic() - start:.3f}s", file=sys.stderr)

if __name__ == "__main__":
    main()
