"""Fixed-interval Linux process CPU/RSS sampler; percentages use one core.

Thread rows cover only matching TID/starttime pairs at both snapshots; started,
exited, or reused threads are excluded, so rows need not sum to process CPU.
"""
import argparse
import json
import os
from pathlib import Path
import time


def parse_stat(text):
    fields = text.rsplit(")", 1)[1].split()
    return int(fields[11]) + int(fields[12])


def parse_starttime(text):
    return int(text.rsplit(")", 1)[1].split()[19])


def cpu_percent(before, after, ticks_per_second, elapsed):
    if ticks_per_second <= 0 or elapsed <= 0:
        raise ValueError("clock tick rate and interval must be positive")
    return (after - before) / ticks_per_second / elapsed * 100.0


def thread_ticks(root):
    result = {}
    for path in (root / "task").glob("*/stat"):
        try:
            text = path.read_text()
            result[path.parent.name] = (
                text.split("(", 1)[1].rsplit(")", 1)[0], parse_stat(text), parse_starttime(text)
            )
        except (FileNotFoundError, ProcessLookupError):
            pass
    return result


def snapshot(pid):
    root = Path("/proc") / str(pid)
    status = {}
    for line in (root / "status").read_text().splitlines():
        if line.startswith(("VmRSS:", "VmHWM:", "Threads:")):
            key, value = line.split(":", 1)
            status[key] = value.strip()
    stat = (root / "stat").read_text()
    return {"ticks": parse_stat(stat), "starttime": parse_starttime(stat),
            "threads": thread_ticks(root), "status": status,
            "time": time.monotonic()}


def summarize(before, after, pid, label):
    if before["starttime"] != after["starttime"]:
        raise RuntimeError("process identity changed during sample")
    elapsed = after["time"] - before["time"]
    hz = os.sysconf("SC_CLK_TCK")
    cores = os.cpu_count() or 1
    total = cpu_percent(before["ticks"], after["ticks"], hz, elapsed)
    hot = []
    for tid, (name, ticks, starttime) in after["threads"].items():
        if tid in before["threads"] and before["threads"][tid][2] == starttime:
            hot.append({"tid": tid, "name": name,
                        "cpu_one_core_pct": cpu_percent(before["threads"][tid][1], ticks, hz, elapsed)})
    return {"pid": pid, "label": label, "elapsed_seconds": elapsed,
            "cpu_one_core_pct": total, "cpu_machine_pct": total / cores,
            "logical_cores": cores, "status": after["status"],
            "threads": sorted(hot, key=lambda row: row["cpu_one_core_pct"], reverse=True)}


def sample(pid, seconds, label):
    if seconds <= 0:
        raise ValueError("sample duration must be positive")
    before = snapshot(pid)
    time.sleep(seconds)
    return summarize(before, snapshot(pid), pid, label)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("pid", type=int)
    parser.add_argument("--seconds", type=float, default=30.0)
    parser.add_argument("--label", default="unspecified")
    args = parser.parse_args()
    if args.seconds <= 0:
        parser.error("--seconds must be positive")
    try:
        print(json.dumps(sample(args.pid, args.seconds, args.label)))
    except (FileNotFoundError, ProcessLookupError, PermissionError, RuntimeError) as error:
        parser.exit(1, f"cannot sample requested process: {error}\n")
