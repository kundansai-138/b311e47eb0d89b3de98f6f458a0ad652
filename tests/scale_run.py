"""Run the60M Linux regression with a hard12GiB container limit and no swap."""
import argparse
import datetime as dt
import json
import os
from pathlib import Path
import subprocess

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("data", type=Path)
    parser.add_argument("report", type=Path)
    args = parser.parse_args()
    args.report.mkdir(parents=True, exist_ok=True)
    output = args.report / "output"
    scratch = args.report / "scratch"
    for path in [output, scratch]:
        path.mkdir(exist_ok=True)
        path.chmod(0o777)
    name = f"shopping-scale-{os.getpid()}"
    cmd = ["docker", "run", "--name", name, "--network", "none", "--read-only",
           "--tmpfs", "/tmp:rw,size=64m,mode=1777", "--cap-drop", "ALL",
           "--security-opt", "no-new-privileges", "--pids-limit", "4096",
           "--cpus", "4", "--memory", "12g", "--memory-swap", "12g", "--user", "1000:1000",
           "-v", f"{args.data.resolve()}:/data:ro", "-v", f"{output.resolve()}:/output",
           "-v", f"{scratch.resolve()}:/scratch", "-e", "DATA_DIR=/data", "-e", "OUTPUT_DIR=/output",
           "-e", "SCRATCH_DIR=/scratch", "-e", "HOME=/scratch", "-e", "NCPU=8", "-e", "MEM_GB=12",
           "--entrypoint", "/bin/sh", "shopping-sessions:optimized", "-c",
           '/app/run.sh; result=$?; printf "cgroup_memory_peak_bytes="; cat /sys/fs/cgroup/memory.peak; exit "$result"']
    log = args.report / "container.log"
    try:
        with log.open("w") as stream:
            p = subprocess.run(cmd, stdout=stream, stderr=subprocess.STDOUT)
        state = json.loads(subprocess.check_output(["docker", "inspect", name]))[0]["State"]
        print(log.read_text(), flush=True)
        if p.returncode or state["ExitCode"] or state["OOMKilled"]:
            raise RuntimeError(f"scale container failed: {state}")
        start = dt.datetime.fromisoformat(state["StartedAt"].replace("Z", "+00:00"))
        end = dt.datetime.fromisoformat(state["FinishedAt"].replace("Z", "+00:00"))
        prefix = "cgroup_memory_peak_bytes="
        peak = next(int(line[len(prefix):]) for line in log.read_text().splitlines() if line.startswith(prefix))
        subprocess.run(["python", "tests/check_scale.py", str(output)], check=True)
        record = {"seconds": (end-start).total_seconds(), "cpu_quota": 4, "worker_threads": 8,
                  "memory_limit_bytes": 12*1024**3, "cgroup_memory_peak_bytes": peak,
                  "baseline_digest_match": True, "generated_events": 60_000_000, "generated_clicks": 15_000_000,
                  "seed": 42, "out_plus_scratch_bytes": sum(p.stat().st_size for d in [output,scratch] for p in d.rglob("*") if p.is_file())}
        (args.report / "results.json").write_text(json.dumps(record, indent=2)+"\n")
        print(json.dumps(record), flush=True)
    finally:
        subprocess.run(["docker", "rm", "-f", name], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

if __name__ == "__main__":
    main()
