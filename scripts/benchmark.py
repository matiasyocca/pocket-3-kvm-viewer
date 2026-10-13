"""Benchmark only a fresh viewer on Hyprland; never move an existing window.

Run from the provisioned release-build environment. Raw JSONL is appended after
EVERY sample, so an interrupted comparison does not erase already collected data.
"""
import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import signal
import subprocess
import time

from sample_cpu import parse_starttime, sample

APP_ID = "io.github.pocket3.KvmViewer"
HYPR_TIMEOUT = 5


def select_new_viewer(clients, before, pgid):
    matches = []
    for client in clients:
        if client["class"] != APP_ID or client["address"] in before:
            continue
        try:
            if os.getpgid(client["pid"]) == pgid:
                matches.append(client)
        except ProcessLookupError:
            pass
    if len(matches) > 1:
        raise ValueError("more than one new viewer; refusing ambiguous target")
    return matches[0] if matches else None


def hidden_workspace(monitors, workspaces):
    used = {w["id"] for w in workspaces}
    for monitor in monitors:
        for kind in ("activeWorkspace", "specialWorkspace"):
            if kind in monitor:
                used.add(monitor[kind]["id"])
    candidate = 1000
    while candidate in used:
        candidate += 1
    return candidate


def hypr_json(command):
    return json.loads(subprocess.check_output(["hyprctl", command, "-j"], text=True,
                                             timeout=HYPR_TIMEOUT))


def dispatch(command, argument):
    subprocess.run(["hyprctl", "dispatch", command, argument], check=True,
                   stdout=subprocess.DEVNULL, timeout=HYPR_TIMEOUT)


def verify_viewer(viewer, pgid, starttime):
    current = next((c for c in hypr_json("clients") if c["address"] == viewer["address"]), None)
    if (current is None or current["class"] != APP_ID or current["pid"] != viewer["pid"]
            or os.getpgid(viewer["pid"]) != pgid
            or process_starttime(viewer["pid"]) != starttime):
        raise RuntimeError("viewer identity changed; refusing target")
    return current


def verify_workspace(viewer, pgid, starttime, target, state):
    current = verify_viewer(viewer, pgid, starttime)
    if current["workspace"]["id"] != target:
        raise RuntimeError("viewer is not on the requested workspace")
    displayed = {m[k]["id"] for m in hypr_json("monitors")
                 for k in ("activeWorkspace", "specialWorkspace") if k in m}
    if (target in displayed) != (state != "hidden"):
        raise RuntimeError(f"workspace is not {state}")
    return current


def process_starttime(pid):
    return parse_starttime(Path(f"/proc/{pid}/stat").read_text())


def process_group(pgid):
    members = {}
    for path in Path("/proc").glob("[0-9]*/stat"):
        try:
            fields = path.read_text().rsplit(")", 1)[1].split()
            if int(fields[2]) == pgid and int(fields[3]) == pgid:
                members[int(path.parent.name)] = (fields[0], int(fields[19]))
        except (FileNotFoundError, ProcessLookupError):
            pass
    return members


def stop_process_group(proc, starttime, grace=5):
    # Keep our child unreaped until all live descendants are gone. Its PID then
    # reserves the private group ID, even if the launcher exits before its children.
    def owned_members():
        members = process_group(proc.pid)
        if members and members.get(proc.pid, (None, None))[1] != starttime:
            raise RuntimeError("launcher identity changed; refusing to signal process group")
        return members

    def live_members():
        return any(state not in {"Z", "X"} for state, _ in owned_members().values())

    try:
        for sig in (signal.SIGTERM, signal.SIGKILL):
            if not live_members():
                break
            try:
                os.killpg(proc.pid, sig)
            except ProcessLookupError:
                break
            deadline = time.monotonic() + grace
            while live_members() and time.monotonic() < deadline:
                time.sleep(0.05)
        if live_members():
            raise RuntimeError("process group survived SIGKILL")
        remaining = owned_members()
    finally:
        proc.wait(timeout=grace)
    # Also wait for adopted descendants' zombies to be reaped. Never signal after
    # reaping our child; compare starttimes so a reused group/PID is not our target.
    deadline = time.monotonic() + grace
    while any(pid in remaining and identity[1] == remaining[pid][1]
              for pid, identity in process_group(proc.pid).items()):
        if time.monotonic() >= deadline:
            raise RuntimeError("process group did not disappear after cleanup")
        time.sleep(0.05)


def append_json(path, record):
    with path.open("a") as stream:
        stream.write(json.dumps(record) + "\n")
        stream.flush()
    print(json.dumps({key: record[key] for key in
                      ("variant", "label", "repeat", "cpu_one_core_pct", "status")}), flush=True)


def run(args):
    output = Path(args.output).resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    binary = Path(args.binary).resolve(strict=True)
    existing = hypr_json("clients")
    before = {c["address"] for c in existing}
    # Deliberately fail closed even if an existing viewer is manually paused.
    if any(c["class"] == APP_ID for c in existing):
        raise RuntimeError("close the existing viewer before hardware benchmarking")
    hidden = hidden_workspace(hypr_json("monitors"), hypr_json("workspaces"))
    focused = hypr_json("activewindow")
    logpath = output.with_suffix(output.suffix + f".{args.label}.log")
    with logpath.open("a") as log:
        proc = subprocess.Popen(["dbus-run-session", "--", str(binary), *args.argument],
                                stdout=log, stderr=log, start_new_session=True)
        starttime = None
        try:
            starttime = process_starttime(proc.pid)
            viewer = None
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline:
                viewer = select_new_viewer(hypr_json("clients"), before, proc.pid)
                if viewer:
                    break
                if process_group(proc.pid).get(proc.pid, ("X", None))[0] in {"Z", "X"}:
                    raise RuntimeError(f"viewer launcher exited; see {logpath}")
                time.sleep(0.1)
            if viewer is None:
                raise RuntimeError(f"no new GTK window; see {logpath}")
            pid, address = viewer["pid"], viewer["address"]
            viewer_starttime = process_starttime(pid)
            verify_viewer(viewer, proc.pid, viewer_starttime)
            workspace = viewer["workspace"]["id"]
            for command, argument in (("setfloating", "address:" + address),
                                      ("resizewindowpixel", "exact 960 540,address:" + address),
                                      ("movewindowpixel", "exact 150 120,address:" + address)):
                verify_viewer(viewer, proc.pid, viewer_starttime)
                dispatch(command, argument)
            if focused.get("address"):
                dispatch("focuswindow", "address:" + focused["address"])
            time.sleep(args.warmup)
            verify_viewer(viewer, proc.pid, viewer_starttime)
            # A real capture owner proves the hardware opened. This is not proof
            # of frames or source signal; pair CPU with caps/frame diagnostics.
            fds = [str(p.resolve()) for p in Path(f"/proc/{pid}/fd").iterdir()]
            if not args.allow_no_capture and not any(p.startswith("/dev/video") for p in fds):
                raise RuntimeError(f"viewer does not own a capture node; see {logpath}")
            for state in args.states.split(","):
                verify_viewer(viewer, proc.pid, viewer_starttime)
                target = hidden if state == "hidden" else workspace
                dispatch("movetoworkspacesilent", f"{target},address:{address}")
                time.sleep(args.warmup)
                for repeat in range(args.repeats):
                    current = verify_workspace(viewer, proc.pid, viewer_starttime, target, state)
                    result = sample(pid, args.seconds, state)
                    verify_workspace(viewer, proc.pid, viewer_starttime, target, state)
                    result.update({"variant": args.label, "repeat": repeat,
                                   "binary": str(binary), "source_revision": args.revision,
                                   "timestamp": datetime.now(timezone.utc).isoformat(),
                                   "window_size": current["size"], "workspace": current["workspace"],
                                   "log": str(logpath)})
                    append_json(output, result)
        finally:
            stop_process_group(proc, starttime)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary")
    parser.add_argument("--label", required=True)
    parser.add_argument("--revision", default="uncommitted")
    parser.add_argument("--output", required=True)
    parser.add_argument("--seconds", type=float, default=15)
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--warmup", type=float, default=3)
    parser.add_argument("--states", default="visible,hidden,returned")
    parser.add_argument("--allow-no-capture", action="store_true")
    parser.add_argument("--argument", action="append", default=[])
    args = parser.parse_args()
    if args.seconds <= 0 or args.repeats < 1 or args.warmup < 0:
        parser.error("durations/repeats must be valid")
    if any(s not in {"visible", "hidden", "returned"} for s in args.states.split(",")):
        parser.error("states must be visible,hidden,returned")
    if not args.label or any(c not in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_" for c in args.label):
        parser.error("label must contain only letters, digits, - and _")
    try:
        run(args)
    except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        parser.exit(1, f"benchmark aborted: {error}\n")


if __name__ == "__main__":
    main()
