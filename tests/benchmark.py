"""Controlled alternating container benchmark; no generated data enters Git."""
import argparse
import datetime as dt
import json
import os
from pathlib import Path
import statistics
import subprocess
import tempfile
import pyarrow.parquet as pq

def command(args, **kw):
    return subprocess.run(args, check=True, **kw)

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("data", type=Path)
    parser.add_argument("report", type=Path)
    parser.add_argument("--repeats", type=int, default=3)
    args = parser.parse_args()
    args.report.mkdir(parents=True, exist_ok=True)
    threads = 8
    quota = min(4, len(os.sched_getaffinity(0)))
    variants = ["combined", "vqsort", "vqsort-rust", "digits", "writer", "clicks"]
    records = []
    first_outputs = {}
    with tempfile.TemporaryDirectory(prefix="shopping-benchmark-", ignore_cleanup_errors=True) as temp:
        root = Path(temp)
        for repetition in range(args.repeats):
            offset = repetition % len(variants)
            order = variants[offset:] + variants[:offset]
            for variant in order:
                run = root / f"{variant}-{repetition}"
                output, scratch = run / "output", run / "scratch"
                output.mkdir(parents=True, mode=0o777)
                scratch.mkdir(mode=0o777)
                output.chmod(0o777)
                scratch.chmod(0o777)
                name = f"shopping-bench-{variant}-{repetition}-{os.getpid()}"
                cmd = ["docker", "run", "--name", name, "--network", "none", "--read-only",
                       "--tmpfs", "/tmp:rw,size=64m,mode=1777", "--cap-drop", "ALL",
                       "--security-opt", "no-new-privileges", "--pids-limit", "4096",
                       "--cpus", str(quota), "--memory", "12g", "--memory-swap", "12g",
                       "--user", "1000:1000", "-v", f"{args.data.resolve()}:/data:ro",
                       "-v", f"{output}:/output", "-v", f"{scratch}:/scratch",
                       "-e", "DATA_DIR=/data", "-e", "OUTPUT_DIR=/output", "-e", "SCRATCH_DIR=/scratch",
                       "-e", "HOME=/scratch", "-e", f"NCPU={threads}", "-e", "MEM_GB=12",
                       "-e", "ROUND3_SORT=" + ("rust" if variant == "vqsort-rust" else "avx2"),
                       f"shopping-sessions:{'vqsort' if variant == 'vqsort-rust' else variant}"]
                log = args.report / f"{variant}-{repetition}.log"
                try:
                    with log.open("w") as handle:
                        completed = subprocess.run(cmd, stdout=handle, stderr=subprocess.STDOUT)
                    inspect = json.loads(subprocess.check_output(["docker", "inspect", name]))[0]
                    state = inspect["State"]
                    if completed.returncode or state["ExitCode"] or state["OOMKilled"]:
                        raise RuntimeError(f"{variant}: failed container; see {log}")
                    start = dt.datetime.fromisoformat(state["StartedAt"].replace("Z", "+00:00"))
                    end = dt.datetime.fromisoformat(state["FinishedAt"].replace("Z", "+00:00"))
                    result = {"variant": variant, "repetition": repetition, "seconds": (end-start).total_seconds()}
                    records.append(result)
                    print(json.dumps(result), flush=True)
                    first_outputs.setdefault(variant, output)
                finally:
                    subprocess.run(["docker", "rm", "-f", name], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        for table, keys in [("user_daily", ["user_id", "day"]), ("campaign_daily", ["campaign_id", "day"])]:
            tables = [pq.read_table(first_outputs[v] / table).sort_by([(key, "ascending") for key in keys])
                      for v in variants]
            if any(not tables[0].equals(table.cast(tables[0].schema), check_metadata=False) for table in tables[1:]):
                raise ValueError(f"{table}: baseline and optimized output differ")
            print(f"{table}: all {tables[0].num_rows} rows exactly equal", flush=True)
        medians = {v: statistics.median(r["seconds"] for r in records if r["variant"] == v)
                   for v in variants}
        report = {"control_variant": "combined", "control_source_commit": "992b7672b54988c773f6d78cb260b3af9a370c7e", "previous_submission_commit": "d52cca7d96130444086754510f8e298d5390eb84",
                  "candidate_commit": os.environ.get("GITHUB_SHA", "local"), "threads": threads, "cpu_quota": quota, "memory_limit_bytes": 12*1024**3, "runs": records,
                  "medians_seconds": medians, "speedups": {v:medians["combined"]/medians[v] for v in variants},
                  "exact_output_match": True}
        (args.report / "results.json").write_text(json.dumps(report, indent=2)+"\n")
        print(json.dumps(report), flush=True)
        # Fix permissions only inside this script-created temporary directory.
        # Timed runs above always use the scorer's non-root UID.
        command(["docker", "run", "--rm", "--network", "none", "--user", "0:0",
                 "-v", f"{root}:/benchmark", "--entrypoint", "chmod", "shopping-sessions:combined",
                 "-R", "a+rwX", "/benchmark"])

if __name__ == "__main__":
    main()
