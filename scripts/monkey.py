#!/usr/bin/env python3
"""Drive porta the way a careless user and a hostile agent would, at random,
and check after every step that what it promises still holds.

    python3 scripts/monkey.py target/porta [--steps 200] [--seed N]

Where fuzz.py throws one hostile input at one part, this strings real
operations together — runs with random grants and random misbehaviour,
snapshots and rollbacks, explain, init and up, runs killed part way — so
that state carried from one step to the next gets exercised too. After each
step:

  - porta did not crash: no Rust panic, no death by signal it was not sent;
  - nothing outside the grants changed: a sentinel tree beside the mounts,
    the read-only mount, and (where porta protects inside mounts) the
    repository's hooks, unless the step dropped the preset;
  - no planted credential reached the command's output unless the step
    dropped the preset;
  - explain --json is JSON; a timed-out or killed run left no process;
  - a snapshot put back with rollback --yes restores the mounts exactly.

The fake home holds the planted credentials and the snapshots, so the real
home is never read from or written to by a step.
"""
import hashlib
import json
import os
import pathlib
import platform
import random
import shutil
import signal
import subprocess
import sys
import tempfile
import time

SYSTEM = platform.system()
MARKER = "MONKEY-SECRET-7f3a"
CREDENTIALS = [".aws/credentials", ".ssh/id_ed25519", ".config/gh/hosts.yml"]


class Violation(Exception):
    pass


def tree(root):
    """Every entry under root: kind, bytes or link target, and mode."""
    found = {}
    for path in sorted(root.rglob("*")):
        info = path.lstat()
        if path.is_symlink():
            found[str(path.relative_to(root))] = ("link", os.readlink(path))
        elif path.is_dir():
            found[str(path.relative_to(root))] = ("dir", info.st_mode & 0o7777)
        else:
            found[str(path.relative_to(root))] = ("file", hashlib.sha256(path.read_bytes()).hexdigest(), info.st_mode & 0o7777)
    return found


class World:
    """The directories one monkey run plays in, and what each may become."""

    def __init__(self, porta, rng):
        self.porta, self.rng = porta, rng
        self.root = pathlib.Path(tempfile.mkdtemp(prefix="porta-monkey-", dir=pathlib.Path.home())).resolve()
        self.home = self.root / "home"
        for name in CREDENTIALS:
            (self.home / name).parent.mkdir(parents=True, exist_ok=True)
            (self.home / name).write_text(f"token={MARKER}\n")
        self.outside = self.root / "outside"
        (self.outside / "keep").mkdir(parents=True)
        (self.outside / "keep" / "note.txt").write_text("untouched\n")
        self.a, self.b, self.ro = self.root / "a", self.root / "b", self.root / "ro"
        for mount in (self.a, self.b, self.ro):
            (mount / "src").mkdir(parents=True)
            (mount / "src" / "main.txt").write_text("hello\n")
        subprocess.run(["git", "init", "-q", str(self.a)], check=True)
        (self.a / ".git" / "hooks" / "pre-commit").write_text("#!/bin/sh\nexit 0\n")
        report = json.loads(self.porta_run(["check", "--json"]).stdout)
        self.protects_inside = SYSTEM == "Darwin" or any("namespace" in p["name"] and p["present"] for p in report["primitives"])
        self.baseline = self.fixed()

    def fixed(self):
        """What no step may change while the preset is in force."""
        return {"outside": tree(self.outside), "ro": tree(self.ro), "hooks": tree(self.a / ".git" / "hooks")}

    def env(self):
        return {**os.environ, "HOME": str(self.home), "PORTA_DENIALS": "never"}

    def porta_run(self, args, timeout=60, stdin=subprocess.DEVNULL):
        return subprocess.run([self.porta, *args], text=True, capture_output=True, timeout=timeout, env=self.env(), stdin=stdin)


# --- what a command does inside a run --------------------------------------

def moves(world, rng):
    """A shell script of random misbehaviour, and the token its processes carry:
    a sleep of a length no other process on the host has."""
    token = f"sleep 31.{rng.randrange(10**6):06d}"
    targets = [world.a / "src", world.b, world.ro / "src", world.outside / "keep", world.home, world.a / ".git" / "hooks", world.root]
    steps = []
    for _ in range(rng.randint(1, 6)):
        pick = rng.randrange(9)
        where = rng.choice(targets)
        if pick == 0:
            steps.append(f'echo x > "{where}/w{rng.randrange(99)}.txt"')
        elif pick == 1:
            steps.append(f'cat "{world.home}/{rng.choice(CREDENTIALS)}"')
        elif pick == 2:
            steps.append(f'rm -rf "{where}"/*')
        elif pick == 3:
            steps.append(f'mv "{world.a}" "{world.a}.moved"')
        elif pick == 4:
            steps.append(f'ln -sf "{world.outside}/keep/note.txt" "{world.a}/src/link" && echo y > "{world.a}/src/link"')
        elif pick == 5:
            steps.append(f'echo x > "{where}/pre-commit"')
        elif pick == 6:
            steps.append(f'{token} &')
        elif pick == 7:
            steps.append(f'head -c {rng.choice([0, 1, 65536, 1 << 20])} /dev/zero | tr "\\\\0" z')
        else:
            steps.append(f'mkdir -p "{world.b}/d{rng.randrange(9)}/e" && touch "{world.b}/d{rng.randrange(9)}/e/f"')
    steps.append(f"exit {rng.choice([0, 0, 1, 2, 42])}")
    return "exec 2>/dev/null\n" + "\n".join(steps), token


def grants(world, rng):
    """Random flags; returns them and whether they drop the preset."""
    flags, dropped = [], False
    for mount in rng.sample([str(world.a), str(world.b)], rng.randint(0, 2)):
        flags += ["-v", mount]
    if rng.random() < 0.5:
        flags += ["-v", f"{world.ro}:ro"]
    if rng.random() < 0.1:
        flags += ["--preset", "none"]
        dropped = True
    options = [["--read-policy", "strict"], ["--no-net"], ["--allow-net", "*:443"], ["--protect", "src/main.txt"],
               ["--deny-read", "~/notes"], ["--max-procs", "400"], ["--max-file-size", "4"], ["-e", "MONKEY=1"]]
    for option in rng.sample(options, rng.randint(0, 3)):
        flags += option
    return flags, dropped


# --- checks ----------------------------------------------------------------

def no_crash(result, what):
    if result.returncode < 0 or result.returncode == 101 or "panicked at" in result.stderr:
        raise Violation(f"{what}: porta crashed (exit {result.returncode}): {result.stderr[-400:]}")


def no_leftover(token):
    time.sleep(0.3)
    found = subprocess.run(["pgrep", "-f", token], capture_output=True, text=True).stdout.split()
    if found:
        for pid in found:
            os.kill(int(pid), signal.SIGKILL)
        raise Violation(f"process {token} outlived its run: {found}")


def fixed_held(world, dropped):
    now = world.fixed()
    for part, before in world.baseline.items():
        allowed = dropped or (part == "hooks" and not world.protects_inside)
        if now[part] != before and not allowed:
            raise Violation(f"{part} changed under the preset: {set(now[part].items()) ^ set(before.items())}")
    restore(world)


def restore(world):
    """Puts the world back for the next step: mounts renamed away return, and
    whatever a preset-less step was allowed to change is reset."""
    moved = pathlib.Path(f"{world.a}.moved")
    if moved.exists() and not world.a.exists():
        moved.rename(world.a)
    shutil.rmtree(moved, ignore_errors=True)
    if world.fixed() != world.baseline:
        for part, path in (("outside", world.outside), ("ro", world.ro)):
            shutil.rmtree(path, ignore_errors=True)
        (world.outside / "keep").mkdir(parents=True, exist_ok=True)
        (world.outside / "keep" / "note.txt").write_text("untouched\n")
        (world.ro / "src").mkdir(parents=True, exist_ok=True)
        (world.ro / "src" / "main.txt").write_text("hello\n")
        hooks = world.a / ".git" / "hooks"
        shutil.rmtree(hooks, ignore_errors=True)
        hooks.mkdir(parents=True, exist_ok=True)
        (hooks / "pre-commit").write_text("#!/bin/sh\nexit 0\n")
        world.baseline = world.fixed()


# --- steps -----------------------------------------------------------------

def step_run(world, rng):
    flags, dropped = grants(world, rng)
    script, token = moves(world, rng)
    snapshot = rng.random() < 0.3 and any(f in flags for f in (str(world.a), str(world.b)))
    timeout = rng.random() < 0.15
    extra = (["--snapshot"] if snapshot else []) + (["--timeout", "1"] if timeout else [])
    before = {m: tree(pathlib.Path(m)) for m in (str(world.a), str(world.b))}
    result = world.porta_run(["run", "/bin/sh", *flags, *extra, "--", "-c", script])
    no_crash(result, "run")
    if MARKER in result.stdout and not dropped:
        raise Violation(f"a planted credential reached the output under the preset: flags {flags}")
    if timeout or token in script:
        # A run that ended by --timeout kills its group; a background sleep
        # after a normal exit is the command's own, and is ended here.
        if timeout and result.returncode == 124:
            no_leftover(token)
        subprocess.run(["pkill", "-9", "-f", token], capture_output=True)
    if snapshot and result.returncode not in (125, 126, 127) and rng.random() < 0.6:
        back = world.porta_run(["rollback", "--yes"])
        no_crash(back, "rollback")
        for mount, was in before.items():
            if pathlib.Path(mount).exists() and tree(pathlib.Path(mount)) != was:
                raise Violation(f"rollback --yes left {mount} different from before the run")
    fixed_held(world, dropped)
    return f"run exit {result.returncode}"


def step_explain(world, rng):
    flags, _ = grants(world, rng)
    result = world.porta_run(["explain", "/bin/echo", *flags, "--json"])
    no_crash(result, "explain")
    try:
        json.loads(result.stdout)
    except ValueError:
        raise Violation(f"explain --json printed no JSON for {flags}: {result.stdout[:200]!r}")
    return "explain"


def step_rollback(world, rng):
    before = {m: tree(m) for m in (world.a, world.b)}
    result = world.porta_run(["rollback", rng.choice(["", "no-such-id", "../../etc", "1-1"])])
    no_crash(result, "rollback")
    if {m: tree(m) for m in (world.a, world.b)} != before:
        raise Violation("rollback without --yes changed a mount")
    return "rollback (dry)"


def step_init_up(world, rng):
    place = pathlib.Path(tempfile.mkdtemp(dir=world.b))
    recipe = rng.choice(["claude", "codex", "native", "wasm", "nonsense"])
    result = subprocess.run([world.porta, "init", recipe, *(["/bin/echo"] if recipe in ("native", "wasm") else [])],
                            cwd=place, text=True, capture_output=True, env=world.env(), timeout=30)
    no_crash(result, "init")
    if (place / "porta.toml").exists():
        up = subprocess.run([world.porta, "up", "--", "--version"], cwd=place, text=True, capture_output=True, env=world.env(), timeout=60)
        no_crash(up, "up")
    shutil.rmtree(place, ignore_errors=True)
    return f"init {recipe}"


def step_kill(world, rng):
    token = f"sleep 32.{rng.randrange(10**6):06d}"
    child = subprocess.Popen([world.porta, "run", "/bin/sh", "-v", str(world.b), "--", "-c", f"{token}; true"],
                             env=world.env(), stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True, start_new_session=True)
    time.sleep(rng.uniform(0.1, 1.0))
    sent = rng.choice([signal.SIGINT, signal.SIGTERM, signal.SIGHUP])
    os.killpg(child.pid, sent)
    try:
        child.wait(timeout=20)
    except subprocess.TimeoutExpired:
        child.kill()
        raise Violation(f"porta did not end within 20s of {sent.name}")
    if "panicked at" in child.stderr.read():
        raise Violation(f"porta panicked on {sent.name}")
    no_leftover(token)
    return f"kill {sent.name}"


STEPS = [(step_run, 6), (step_explain, 2), (step_rollback, 1), (step_init_up, 1), (step_kill, 1)]


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    porta = str(pathlib.Path(sys.argv[1]).resolve())
    argv = sys.argv[2:]
    steps = int(argv[argv.index("--steps") + 1]) if "--steps" in argv else 200
    seed = int(argv[argv.index("--seed") + 1]) if "--seed" in argv else random.randrange(1 << 32)
    rng = random.Random(seed)
    world = World(porta, rng)
    done = {}
    try:
        for index in range(steps):
            step = rng.choices([s for s, _ in STEPS], weights=[w for _, w in STEPS])[0]
            try:
                outcome = step(world, rng)
            except Violation as violation:
                sys.exit(f"monkey: VIOLATION at step {index} ({step.__name__}), seed {seed}: {violation}")
            done[outcome.split()[0]] = done.get(outcome.split()[0], 0) + 1
    finally:
        shutil.rmtree(world.root, ignore_errors=True)
    print(f"monkey: {steps} steps, seed {seed}, no violation; {done}")


if __name__ == "__main__":
    main()
