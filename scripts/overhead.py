#!/usr/bin/env python3
"""What a sandbox costs a command: wall time under each mode, against bare.

    python3 scripts/overhead.py target/porta [--runs 30] [--json out.json]
                                [--tool srt=/path/to/srt ...]

Each command runs `--runs` times after three warm-up runs, bare and then under
every porta mode, alternating so that a slow minute on the host lands on all
of them alike. Reported: the median and 90th percentile wall time, and the
median minus bare. A run that exits non-zero is not a measurement of the
command, so it stops the script rather than being averaged in.

`--tool name=binary` adds another sandbox, run with the same policy through
the translations in escape_runners.py.
"""
import json
import os
import pathlib
import platform
import statistics
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, str(pathlib.Path(__file__).parent))
from escape_runners import NotExpressible, make_runner  # noqa: E402

PYTHON = os.path.join(sys.base_prefix, "bin", "python3")
if not os.path.exists(PYTHON):
    PYTHON = sys.executable

MODES = [
    ("default", []),
    ("strict reads", ["--read-policy", "strict"]),
    ("--no-net", ["--no-net"]),
    ("--allow-net *:443", ["--allow-net", "*:443"]),
    ("--proxy-allow", ["--proxy-allow", "example.com"]),
]


def commands(workspace):
    git = subprocess.run(["which", "git"], capture_output=True, text=True).stdout.strip() or "/usr/bin/git"
    return [
        ("true", "/usr/bin/true", []),
        ("python3 -c pass", PYTHON, ["-c", "pass"]),
        ("git status", git, ["-C", str(workspace), "status", "--short"]),
    ]


class Failed(Exception):
    """The command did not succeed under this variant, so there is nothing to time."""


def timed(argv, cwd):
    started = time.perf_counter()
    result = subprocess.run(argv, cwd=cwd, stdin=subprocess.DEVNULL, capture_output=True)
    elapsed = (time.perf_counter() - started) * 1000
    if result.returncode != 0:
        raise Failed(f"exit {result.returncode}: {result.stderr.decode().strip().splitlines()[-1][:160] if result.stderr.strip() else ''}")
    return elapsed


def summary(samples):
    ordered = sorted(samples)
    return {"median_ms": round(statistics.median(ordered), 2), "p90_ms": round(ordered[int(len(ordered) * 0.9) - 1], 2)}


def prepare_workspace():
    workspace = pathlib.Path(tempfile.mkdtemp(prefix="porta-overhead-", dir=pathlib.Path.home()))
    subprocess.run(["git", "init", "-q", str(workspace)], check=True)
    for index in range(50):
        (workspace / f"file{index}.txt").write_text("x\n")
    subprocess.run(["git", "-C", str(workspace), "add", "."], check=True)
    subprocess.run(["git", "-C", str(workspace), "-c", "user.email=o@x", "-c", "user.name=o", "commit", "-qm", "x"], check=True)
    return workspace


def skipped(cmd, flags):
    """git reads the user's configuration under the home, which strict reads
    close unless the whole home is granted, and granting it would not measure
    strict reads at all."""
    return "strict" in flags and pathlib.Path(cmd).name == "git"


def install_grant(cmd, flags):
    """Under strict reads, the command's own install, read-only, as a user
    would have to grant it: a tool from Nix or an interpreter under the home
    lives outside the system directories strict reads leave open."""
    if "strict" not in flags:
        return []
    root = pathlib.Path(cmd).resolve().parent.parent
    if str(root).startswith("/nix/store/"):
        # A Nix package loads libraries from other store paths.
        return ["-v", "/nix/store:ro"]
    system = ("/usr", "/bin", "/sbin", "/System", "/lib")
    return [] if str(root).startswith(system) or str(root) == "/" else ["-v", f"{root}:ro"]


def variants(porta, tools, workspace):
    """Every (label, argv builder) to time, bare first."""
    grant = ["-v", str(workspace)]
    found = [("bare", lambda cmd, args: [cmd, *args])]
    runner = make_runner("porta", porta)
    for label, flags in MODES:
        found.append((f"porta {label}", lambda cmd, args, flags=flags: None if skipped(cmd, flags) else runner.argv(cmd, args, [*grant, *flags, *install_grant(cmd, flags)])))
    for name, binary in tools:
        other = make_runner(name, binary)
        try:
            other.argv("/usr/bin/true", [], grant)
        except NotExpressible:
            continue
        found.append((name, lambda cmd, args, other=other: other.argv(cmd, args, grant)))
    return found


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    porta = str(pathlib.Path(sys.argv[1]).resolve())
    option = lambda name, default=None: sys.argv[sys.argv.index(name) + 1] if name in sys.argv else default
    runs = int(option("--runs", "30"))
    tools = [tuple(sys.argv[i + 1].split("=", 1)) for i, arg in enumerate(sys.argv) if arg == "--tool"]
    workspace = prepare_workspace()
    try:
        table = measure(porta, tools, workspace, runs)
    finally:
        subprocess.run(["rm", "-rf", str(workspace)])
    report(porta, table, runs, option("--json"))


def measure(porta, tools, workspace, runs):
    plans = variants(porta, tools, workspace)
    table = {}
    for name, cmd, args in commands(workspace):
        samples, failures = sample(plans, cmd, args, workspace, runs)
        bare = statistics.median(samples["bare"])
        table[name] = {label: tabulate(values, bare, failures.get(label)) for label, values in samples.items()}
    return table


def sample(plans, cmd, args, workspace, runs):
    """Timings per variant, and why a variant produced none."""
    samples = {label: [] for label, _ in plans}
    failures = {}
    for round_ in range(runs + 3):
        for label, build in plans:
            argv = build(cmd, args)
            if argv is None or label in failures:
                continue
            try:
                elapsed = timed(argv, cwd=workspace)
            except Failed as reason:
                if label == "bare":
                    raise SystemExit(f"{cmd} fails with no sandbox at all: {reason}")
                # A command the sandbox's policy stops is not a timing; it is
                # reported, and the row left empty.
                failures[label] = str(reason)
                continue
            if round_ >= 3:
                samples[label].append(elapsed)
    return samples, failures


def tabulate(values, bare, failure):
    if not values:
        return {"not_measured": failure or "needs the whole home granted under strict reads"}
    return {**summary(values), "over_bare_ms": round(statistics.median(values) - bare, 2)}


def report(porta, table, runs, json_out):
    version = subprocess.run([porta, "--version"], capture_output=True, text=True).stdout.strip()
    host = f"{platform.system()} {platform.release()} {platform.machine()}"
    namespaces = ""
    if platform.system() == "Linux":
        check = json.loads(subprocess.run([porta, "check", "--json"], capture_output=True, text=True).stdout)
        present = any("namespace" in p["name"] and p["present"] for p in check["primitives"])
        namespaces = "with its own PID/mount namespace" if present else "without namespaces (the host refuses them)"
    print(f"overhead: {version} on {host} {namespaces}, {runs} runs each, milliseconds\n")
    labels = list(next(iter(table.values())).keys())
    print(f"{'':24}" + "".join(f"{name:>22}" for name in table))
    for label in labels:
        cells = "".join(
            f"{table[name][label]['median_ms']:>10.1f} (+{max(table[name][label]['over_bare_ms'], 0):>6.1f})     " if "median_ms" in table[name][label] else f"{'—':>22}"
            for name in table
        )
        print(f"{label:24}{cells}")
    if json_out:
        pathlib.Path(json_out).write_text(json.dumps({"porta": version, "host": host, "namespaces": namespaces, "runs": runs, "results": table}, indent=2))


if __name__ == "__main__":
    main()
