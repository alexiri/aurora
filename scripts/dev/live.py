#!/usr/bin/env python3
"""Live-reload dev loop (OPS-018, docs/ops/live-reload.md): watch -> incremental rebuild -> restart.

Run through `just dev` (which sources deploy/dev/env.sh, migrates, and mints the dev CA first).
Python 3 standard library only, so the owner installs nothing.

* Watches the source of each service crate and of every workspace crate it depends on
  (`cargo metadata`), by polling mtimes every 150 ms (a few hundred files: ~1 ms per scan).
* On a change: one `cargo build -p <affected binaries>` (only the affected services), then
  stop the old process and start the new one. The old service keeps serving while compiling; a
  failed build leaves it running and prints the compiler output.
* Restarts are idempotent: migrations are applied by `just migrate` and re-running is a no-op,
  and every service re-subscribes its LISTEN/NOTIFY channels at start.
* Logs of all processes are multiplexed with a per-service prefix; each restart prints the measured
  save -> listening time. Ctrl-C (or SIGTERM) stops every process group and exits.

`--kind` (just kind-live): no host processes; instead of cargo + restart, each change runs
`deploy/dev/kind-dev.sh dev <affected services>` (builder container -> thin image -> kind load ->
rollout restart; OPS-017, ~35 s). Needs the cluster from `just kind-up`.

Env: AURORA_DEV_PROFILE (cargo profile, default `dev`), AURORA_LIVE_SERVICES (default
`api,gateway,hookhost,runner,dev-idp`), AURORA_LIVE_WEB=0 to skip `next dev`.
"""
import json
import os
import signal
import socket
import subprocess
import sys
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PROFILE = os.environ.get("AURORA_DEV_PROFILE", "dev")
POLL = 0.15
DEBOUNCE = 0.15
STOP_GRACE = 0.5  # dev: in-flight long polls must not hold a restart up

# name -> (package, readiness port, binary, argv). `dev-idp` (P1-005, RFE-004) is the development
# OIDC provider every sign-in goes through: the `aurora-dev-idp` binary of the aurora-testkit package.
SERVICES = {
    "api": ("aurora-api", 4101, "aurora-api", ["serve"]),
    "gateway": ("aurora-gateway", 4102, "aurora-gateway", ["serve"]),
    "hookhost": ("aurora-hookhost", 4104, "aurora-hookhost", ["serve"]),
    "runner": ("aurora-runner", 4106, "aurora-runner", ["serve"]),
    "dev-idp": ("aurora-testkit", 4107, "aurora-dev-idp", []),
}
COLORS = {"api": 36, "gateway": 35, "hookhost": 33, "runner": 32, "dev-idp": 31, "web": 34, "live": 90}
WATCHED_DIRS = ("src", "migrations", "assets", "templates")
WATCHED_FILES = ("Cargo.toml", "build.rs")


def log(tag, msg):
    color = COLORS.get(tag, 37)
    prefix = f"\033[{color}m{tag:>8} |\033[0m " if sys.stdout.isatty() else f"{tag:>8} | "
    sys.stdout.write(prefix + msg.rstrip("\n") + "\n")
    sys.stdout.flush()


def die(msg):
    log("live", f"error: {msg}")
    sys.exit(1)


def metadata():
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked", "--offline"],
        cwd=ROOT, capture_output=True, text=True,
    )
    if out.returncode != 0:
        die("cargo metadata failed:\n" + out.stderr)
    return json.loads(out.stdout)


def closure(meta, package):
    """Workspace crates `package` depends on (itself included), as {name: manifest dir}."""
    members = set(meta["workspace_members"])
    pkgs = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    start = next(i for i in members if pkgs[i]["name"] == package)
    seen, todo = set(), [start]
    while todo:
        i = todo.pop()
        if i in seen:
            continue
        seen.add(i)
        # Normal + build deps only: a dev-dependency edit cannot change the built binary.
        for d in nodes[i]["deps"]:
            if d["pkg"] in members and any(
                k["kind"] in (None, "build") for k in d["dep_kinds"]
            ):
                todo.append(d["pkg"])
    return {pkgs[i]["name"]: Path(pkgs[i]["manifest_path"]).parent for i in seen}


def snapshot(dirs):
    snap = {}
    for d in dirs:
        for f in WATCHED_FILES:
            p = d / f
            try:
                snap[p] = p.stat().st_mtime_ns
            except OSError:
                pass
        for sub in WATCHED_DIRS:
            for dirpath, _, files in os.walk(d / sub):
                for name in files:
                    p = Path(dirpath) / name
                    try:
                        snap[p] = p.stat().st_mtime_ns
                    except OSError:
                        pass
    try:
        p = ROOT / "Cargo.lock"
        snap[p] = p.stat().st_mtime_ns
    except OSError:
        pass
    return snap


class Proc:
    def __init__(self, tag, argv, cwd=ROOT):
        self.tag, self.argv, self.cwd, self.p = tag, argv, cwd, None

    def start(self):
        self.p = subprocess.Popen(
            self.argv, cwd=self.cwd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
            text=True, bufsize=1, start_new_session=True,
        )
        threading.Thread(target=self._pump, args=(self.p,), daemon=True).start()

    def _pump(self, p):
        for line in p.stdout:
            log(self.tag, line)

    def stop(self):
        p = self.p
        if p is None or p.poll() is not None:
            return
        try:
            os.killpg(p.pid, signal.SIGTERM)
        except ProcessLookupError:
            return
        try:
            p.wait(STOP_GRACE)
        except subprocess.TimeoutExpired:
            os.killpg(p.pid, signal.SIGKILL)
            p.wait()


def wait_listening(port, timeout=30.0):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.2):
                return True
        except OSError:
            time.sleep(0.02)
    return False


def main():
    kind = "--kind" in sys.argv[1:]
    wanted = [s for s in os.environ.get("AURORA_LIVE_SERVICES", "api,gateway,hookhost,runner,dev-idp").split(",") if s]
    for s in wanted:
        if s not in SERVICES:
            die(f"unknown service `{s}` (known: {', '.join(SERVICES)})")
    for var in () if kind else ("AURORA_CONFIG", "AURORA_DATABASE_URL"):
        if not os.environ.get(var):
            die(f"{var} is not set; run through `just dev` (it sources deploy/dev/env.sh)")

    meta = metadata()
    target = Path(meta["target_directory"]) / ("debug" if PROFILE == "dev" else PROFILE)
    watch = {s: closure(meta, SERVICES[s][0]) for s in wanted}
    snap_dirs = sorted({d for c in watch.values() for d in c.values()})
    log("live", f"{'kind' if kind else 'profile=' + PROFILE}; watching {len(snap_dirs)} crates for {', '.join(wanted)}")

    procs = {} if kind else {s: Proc(s, [str(target / SERVICES[s][2]), *SERVICES[s][3]]) for s in wanted}
    web = None
    if not kind and os.environ.get("AURORA_LIVE_WEB", "1") != "0":
        web = Proc("web", ["pnpm", "--dir", "web", "dev"])

    stopping = threading.Event()

    def shutdown(*_):
        stopping.set()

    signal.signal(signal.SIGINT, shutdown)
    signal.signal(signal.SIGTERM, shutdown)

    def build(services):
        if kind:
            argv = ["bash", "deploy/dev/kind-dev.sh", "dev", *services]
        else:
            argv = ["cargo", "build", "--profile", PROFILE]
            for s in services:
                argv += ["-p", SERVICES[s][0]]
        t = time.monotonic()
        r = subprocess.run(argv, cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        dt = time.monotonic() - t
        if r.returncode != 0:
            for line in r.stdout.splitlines():
                log("build", line)
            log("live", f"build FAILED after {dt:.1f}s; keeping the running services ({', '.join(services)})")
            return None
        return dt

    def restart(services, saved_at):
        for s in services:
            procs[s].stop()
        for s in services:
            procs[s].start()
        for s in services:
            ok = wait_listening(SERVICES[s][1])
            total = time.monotonic() - saved_at
            log("live", f"{s}: serving on :{SERVICES[s][1]} {total:.2f}s after save" if ok
                else f"{s}: not listening on :{SERVICES[s][1]} after 30s (see its log)")

    try:
        if kind:
            log("kind", "watching only; run `just kind-up` first if the cluster does not exist")
        else:
            log("live", "initial build")
            if build(wanted) is None:
                die("initial build failed")
            if web:
                web.start()
            restart(wanted, time.monotonic())
        snap = snapshot(snap_dirs)
        log("live", "ready; save a file to reload (Ctrl-C stops everything)")
        while not stopping.is_set():
            time.sleep(POLL)
            now = snapshot(snap_dirs)
            if now == snap:
                continue
            saved_at = time.monotonic() - POLL / 2  # mean detection latency
            # Debounce: wait until the tree is quiet.
            while not stopping.is_set():
                time.sleep(DEBOUNCE)
                nxt = snapshot(snap_dirs)
                if nxt == now:
                    break
                now = nxt
            changed = {p for p in set(now) | set(snap) if now.get(p) != snap.get(p)}
            snap = now
            hit = [
                s for s in wanted
                if any(d in p.parents for p in changed for d in watch[s].values())
                or ROOT / "Cargo.lock" in changed
            ]
            if not hit:
                continue
            log("live", f"change in {', '.join(sorted(p.name for p in changed)[:3])}; rebuilding {', '.join(hit)}")
            took = build(hit)
            if took is None:
                snap = snapshot(snap_dirs)
                continue
            log("live", f"built in {took:.2f}s")
            if kind:
                log("live", f"{', '.join(hit)}: rolled out {time.monotonic() - saved_at:.1f}s after save")
            else:
                restart(hit, saved_at)
    finally:
        log("live", "stopping")
        threads = [threading.Thread(target=p.stop) for p in list(procs.values()) + ([web] if web else [])]
        for t in threads:
            t.start()
        for t in threads:
            t.join()


if __name__ == "__main__":
    main()
