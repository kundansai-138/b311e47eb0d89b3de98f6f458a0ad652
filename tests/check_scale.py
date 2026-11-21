"""Exact canonical per-part digests for the seed42,60M regression dataset.
Run with NCPU=8, so both native implementations produce the same32 hash partitions.
The expected values come from the fixture/dev-validated original implementation.
"""
import argparse
import hashlib
import json
from pathlib import Path
import pyarrow.parquet as pq

def fingerprint(folder):
    result = {}
    for table_name, keys in [("user_daily", ["user_id", "day"]), ("campaign_daily", ["campaign_id", "day"])]:
        result[table_name] = {}
        for path in sorted((folder / table_name).glob("part-*.parquet")):
            table = pq.read_table(path).sort_by([(key, "ascending") for key in keys])
            digest = hashlib.sha256()
            for batch in table.to_batches(max_chunksize=16_384):
                for row in zip(*(column.to_pylist() for column in batch.columns)):
                    digest.update((json.dumps(row, ensure_ascii=True, separators=(",", ":"), default=str)+"\n").encode())
            result[table_name][path.name] = {"rows": table.num_rows, "sha256": digest.hexdigest()}
        print(f"{table_name}: {sum(x['rows'] for x in result[table_name].values())} rows fingerprinted", flush=True)
    return result

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("output", type=Path)
    parser.add_argument("--record", type=Path)
    args = parser.parse_args()
    actual = fingerprint(args.output)
    if args.record:
        args.record.write_text(json.dumps(actual, indent=2)+"\n")
    else:
        expected = json.loads((Path(__file__).parent / "scale-digests.json").read_text())
        if actual != expected:
            raise ValueError("60M output differs from the original implementation")
        print("All 60M regression output digests match")

if __name__ == "__main__":
    main()
