#!/usr/bin/env python3
"""Fuzz the parts of porta that read input a stranger controls.

    python3 scripts/fuzz.py target/porta policy [--iterations 200] [--seed N]
    python3 scripts/fuzz.py target/porta proxy  [--iterations 500] [--seed N]
    python3 scripts/fuzz.py target/porta cli    [--iterations 500] [--seed N]

Every target has two oracles. porta never crashes: no death by signal, no
Rust panic. And it never widens what it was asked for:

  policy  Directories with hostile names (quotes, backslashes, parentheses,
          newlines, fragments of the macOS profile language) are mounted and
          the run writes everywhere it can. A write must land in every
          writable mount, and in no read-only mount, no sibling, no directory
          outside, and on macOS no repository hook. A run porta refuses must
          have written nothing.
  proxy   Hostile CONNECT requests go to the proxy from inside a run. A
          tunnel may open only for a host the allow-list names, compared the
          way DNS compares names; the audit trail records each decision.
  cli     Random argument vectors. porta answers with a usage error, a
          refusal, or a run, and never with a crash.

A seed makes a run reproducible; every failure prints the seed and the
iteration that found it.
"""
import json
import os
import pathlib
import platform
import random
import shutil
import subprocess
import sys
import tempfile

SYSTEM = platform.system()
PYTHON = os.path.join(sys.base_prefix, "bin", "python3")
if not os.path.exists(PYTHON):
    PYTHON = sys.executable

# Characters that mean something to a profile, a shell, a path or a parser.
HOSTILE = ['"', "\\", "(", ")", "\n", "\t", " ", ";", "#", "|", "'", "$", "*", "?", "[", "]", "{", "}",
           ":", "=", ",", ".", "..", "é", "日", "‮", "\u0000"[:0]]
FRAGMENTS = ['")(allow file-write* (subpath "/', '(deny ', '"))', '#|', '|#', '(with message "x")',
             '\n(allow default)\n', '(literal "/")', "\\\"", ":ro", "/", "\r"]


class Violation(Exception):
    """porta did something it must never do."""


def hostile_name(rng):
    parts = [rng.choice(HOSTILE + FRAGMENTS) if rng.random() < 0.5 else rng.choice("abcxyz") for _ in range(rng.randint(1, 12))]
    name = "".join(parts).replace("/", "_").strip() or "x"
    if name in (".", ".."):
        name = "dots"
    # A name ending in ":ro" is porta's read-only marker; the harness would
    # not know which the caller meant.
    return (name[:80] + "n") if name.endswith(":ro") else name[:80]


def run(porta, *args, timeout=30):
    result = subprocess.run([porta, *args], capture_output=True, text=True, timeout=timeout, errors="replace")
    crashed = result.returncode < 0 or "panicked" in result.stderr or "RUST_BACKTRACE" in result.stderr
    if crashed:
        raise Violation(f"porta crashed (exit {result.returncode}): {result.stderr[-400:]}")
    return result


# --- policy -----------------------------------------------------------------

WRITER = 'for target in "$@"; do (printf x > "$target") 2>/dev/null; done; exit 0'


def policy_case(porta, rng, root):
    case = pathlib.Path(tempfile.mkdtemp(dir=root))
    mounts = []
    for index in range(rng.randint(1, 4)):
        directory = case / f"{index}{hostile_name(rng)}"
        directory.mkdir()
        mounts.append((directory, rng.random() < 0.25))
    outside = case / "outside"
    outside.mkdir()
    # A sibling whose name extends a mount's: a prefix match would cover it.
    sibling = pathlib.Path(str(mounts[0][0]) + rng.choice([" ", "-", "\"", "x", "\n"]))
    sibling.mkdir(exist_ok=True)
    hooks = mounts[0][0] / ".git" / "hooks"
    hooks.mkdir(parents=True)
    strict = rng.random() < 0.3
    flags = [arg for directory, ro in mounts for arg in ("-v", f"{directory}:ro" if ro else str(directory))]
    if strict:
        flags += ["--read-policy", "strict"]
    expected = {directory / "in": not ro for directory, ro in mounts}
    expected[outside / "out"] = False
    expected[sibling / "out"] = False
    if SYSTEM == "Darwin" and not mounts[0][1]:
        expected[hooks / "pre-commit"] = False
    result = run(porta, "run", "/bin/sh", *flags, "--", "-c", WRITER, "sh", *map(str, expected))
    landed = {path: path.exists() for path in expected}
    refused = result.returncode in (125, 126, 127) or result.returncode == 65
    for path, should in expected.items():
        if landed[path] and not should:
            raise Violation(f"a write landed where the policy forbids it: {path!r} (mounts {[str(m) for m, _ in mounts]!r}, strict={strict})")
        if should and not landed[path] and not refused:
            raise Violation(f"a write to a granted mount did not land, and the run was not refused: {path!r}; exit {result.returncode}: {result.stderr[-300:]}")
    return "refused" if refused else "ran"


# --- proxy ------------------------------------------------------------------

ALLOWED = ["example.com", "*.example.org"]


def dns_matches(host, patterns):
    """A list as DNS reads it: one trailing dot is the same name, case does
    not matter, and a wildcard covers the name and its subdomains."""
    name = host.lower()
    if name.endswith(".") and not name.endswith(".."):
        name = name[:-1]
    for pattern in patterns:
        if pattern.startswith("*."):
            base = pattern[2:]
            if name == base or name.endswith("." + base):
                return True
        elif name == pattern:
            return True
    return False


def dns_allows(host, mode="allow"):
    """Whether a tunnel to `host` may open: named by the allow-list, or, in
    deny mode, not named by the deny-list."""
    return dns_matches(host, ALLOWED) if mode == "allow" else not dns_matches(host, ALLOWED)


HOSTS = ["example.com", "EXAMPLE.com", "example.com.", "example.com..", "evil.test", "example.com.evil.test",
         "evilexample.com", "a.example.org", "example.org", "xexample.org", "a.example.org.evil.test",
         "example.com%00.evil.test", "example.com\t", "[::1]", "127.0.0.1", "169.254.169.254", "localhost",
         "example.com@evil.test", "evil.test#example.com", "evil.test/example.com", "0x7f.1",
         "exa mple.com", "*.example.org", "", "例え.example.org", "EXAMPLE.ORG", "b.a.example.org"]


def proxy_request(rng):
    host = rng.choice(HOSTS)
    if rng.random() < 0.3:
        host = host + rng.choice(["", ".", ":", ":443", ":80", ":0443", ":99999", "\r", " ", "%2e"])
    port = rng.choice(["443", "443", "80", "", "0", "-1", "65979", "443 extra"])
    method = rng.choice(["CONNECT", "CONNECT", "connect", "GET", "CONNECT\t"])
    authority = f"{host}:{port}" if port else host
    line = f"{method} {authority} HTTP/1.1"
    return line, host


PROXY_CLIENT = r'''
import base64, json, os, socket, sys, urllib.parse
proxy = urllib.parse.urlsplit(os.environ["HTTPS_PROXY"])
credential = base64.b64encode(f"{proxy.username}:{proxy.password}".encode()).decode()
out = []
for line in json.loads(sys.argv[1]):
    try:
        with socket.create_connection((proxy.hostname, proxy.port), 5) as stream:
            stream.settimeout(15)
            stream.sendall(line.encode("utf-8", "surrogateescape") + b"\r\nProxy-Authorization: Basic " + credential.encode() + b"\r\n\r\n")
            reply = stream.recv(200).decode("latin-1").split("\r\n")[0]
    except OSError as error:
        reply = f"error {error}"
    out.append(reply)
print(json.dumps(out))
'''


def proxy_batch(porta, rng, root, size=20):
    requests = [proxy_request(rng) for _ in range(size)]
    audit = pathlib.Path(tempfile.mkdtemp(dir=root)) / "audit.jsonl"
    # The same names as a deny-list: there a name the list misses is let
    # through, so a spelling that escapes the match is a way out.
    mode = rng.choice(["allow", "deny"])
    result = run(porta, "run", PYTHON, "-v", str(audit.parent), f"--proxy-{mode}", ",".join(ALLOWED),
                 "--proxy-audit", str(audit), "--", "-c", PROXY_CLIENT, json.dumps([line for line, _ in requests]), timeout=300)
    if result.returncode != 0:
        raise Violation(f"the proxy client did not complete: exit {result.returncode}: {result.stderr[-400:]}")
    replies = json.loads(result.stdout.strip().splitlines()[-1])
    decisions = [json.loads(line) for line in audit.read_text().splitlines()] if audit.exists() else []
    check_tunnels(mode, [line for line, _ in requests], replies, decisions)
    return f"{mode}: {sum(' 200 ' in reply for reply in replies)} tunnels of {size}"


def check_tunnels(mode, lines, replies, decisions):
    """Every tunnel that opened, and every allow the audit recorded, was one
    the list permits."""
    for line, reply in zip(lines, replies):
        if " 200 " not in reply:
            continue
        target = line.split()[1] if len(line.split()) > 1 else ""
        host = target.rsplit(":", 1)[0] if ":" in target else target
        if not dns_allows(host, mode):
            raise Violation(f"{mode} mode opened a tunnel the list forbids: {line!r} -> {reply!r}")
    for decision in decisions:
        if decision["decision"] == "allow" and not dns_allows(decision["host"], mode):
            raise Violation(f"{mode} mode: the audit records an allow the list forbids: {decision}")


# --- cli --------------------------------------------------------------------

FLAGS = ["run", "explain", "check", "serve", "up", "-v", "--allow-net", "--proxy-allow", "--proxy-deny", "--read-policy",
         "--timeout", "--max-cpu", "--max-procs", "--max-file-size", "--max-memory-mb", "--no-net", "--allow-bind",
         "--allow-unix", "--env-pass", "-e", "--json", "--save", "--", "--allow-root", "--profile"]
VALUES = ["", "0", "-1", "99999999999999999999", "strict", "open", "*:443", "*:0", "a:b:c", "/", "/nonexistent",
          ".", "..", "\n", "\"", "x=y", "=", "*", "🙂", "--", "-v", "1e9", "0x10", "/bin/echo"]


def cli_case(porta, rng, root):
    argv = []
    for _ in range(rng.randint(0, 8)):
        argv.append(rng.choice(FLAGS) if rng.random() < 0.5 else rng.choice(VALUES))
    if argv and argv[0] in ("serve", "up"):
        argv = argv[1:]
    cwd = tempfile.mkdtemp(dir=root)
    result = subprocess.run([porta, *argv], capture_output=True, text=True, timeout=30, cwd=cwd, stdin=subprocess.DEVNULL, errors="replace")
    if result.returncode < 0 or "panicked" in result.stderr:
        raise Violation(f"porta crashed on {argv!r}: exit {result.returncode}: {result.stderr[-300:]}")
    return str(result.returncode)


TARGETS = {"policy": (policy_case, 200), "proxy": (proxy_batch, 25), "cli": (cli_case, 500)}


def main():
    if len(sys.argv) < 3 or sys.argv[2] not in TARGETS:
        sys.exit(__doc__)
    porta, target = str(pathlib.Path(sys.argv[1]).resolve()), sys.argv[2]
    option = lambda name, default: int(sys.argv[sys.argv.index(name) + 1]) if name in sys.argv else default
    case, default_iterations = TARGETS[target]
    iterations, seed = option("--iterations", default_iterations), option("--seed", random.randrange(1 << 32))
    rng = random.Random(seed)
    root = pathlib.Path(tempfile.mkdtemp(prefix="porta-fuzz-", dir=pathlib.Path.home()))
    outcomes = {}
    try:
        for iteration in range(iterations):
            try:
                outcome = case(porta, rng, root)
            except Violation as violation:
                print(f"FAIL {target} seed {seed} iteration {iteration}: {violation}")
                sys.exit(1)
            outcomes[outcome] = outcomes.get(outcome, 0) + 1
    finally:
        shutil.rmtree(root, ignore_errors=True)
    print(f"fuzz {target}: {iterations} iterations, seed {seed}, no violation; outcomes {dict(sorted(outcomes.items()))}")


if __name__ == "__main__":
    main()
