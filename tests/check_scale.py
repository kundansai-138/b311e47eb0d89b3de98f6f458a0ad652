"""Partition-independent digest of the original validated 60M output.

Expected digests were computed from the original implementation's retained
seed42 output, before introducing the new hash or native Parquet writer.
"""
import argparse
import hashlib
import json
from pathlib import Path

import pyarrow.parquet as pq


SPECS = [
    ("user_daily", ["user_id", "day", "sessions", "events", "revenue_cents", "top_category"],
     ["user_id", "day"]),
    ("campaign_daily", ["campaign_id", "channel", "day", "sessions", "purchasing_sessions",
                        "revenue_cents", "assisted_sessions"], ["campaign_id", "day"]),
]


def fingerprint(folder):
    result = {}
    for table_name, columns, keys in SPECS:
        table = pq.read_table(folder / table_name, columns=columns).sort_by(
            [(key, "ascending") for key in keys])
        digest = hashlib.sha256()
        for batch in table.to_batches(max_chunksize=16_384):
            for row in zip(*(column.to_pylist() for column in batch.columns)):
                digest.update((json.dumps(row, ensure_ascii=True, separators=(",", ":"),
                                          default=str) + "\n").encode())
        result[table_name] = {"rows": table.num_rows, "sha256": digest.hexdigest()}
        print(f"{table_name}: {table.num_rows} rows fingerprinted", flush=True)
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("output", type=Path)
    parser.add_argument("--record", type=Path)
    args = parser.parse_args()
    actual = fingerprint(args.output)
    if args.record:
        args.record.write_text(json.dumps(actual, indent=2) + "\n")
    else:
        expected = json.loads((Path(__file__).parent / "scale-global-digests.json").read_text())
        if actual != expected:
            raise ValueError("60M output differs from original validated implementation")
        print("All 60M regression output digests match")


if __name__ == "__main__":
    main()
