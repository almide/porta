#!/usr/bin/env python3
"""The escape corpus: every way out of the sandbox this project knows about,
tried against a porta binary, one attempt per row.

    python3 scripts/escapes.py target/porta            # table on stdout
    python3 scripts/escapes.py target/porta --json out.json

Each attempt runs a command under `porta run` with a policy that should stop
it, and records whether it was stopped ("held"), got through ("ESCAPED"), or
could not be tried on this host ("n/a": a primitive the platform lacks, a tool
that is not installed). An attempt that gets through is a finding, and the
exit status says so. The corpus is published with its results, including any
row porta loses, because a sandbox that shows only the rows it wins is showing
a brochure.

The runner is deliberately independent of porta's own test helpers: it drives
the published binary the way a stranger would, so the same file can be pointed
at a competitor. The list is drawn from the escape tests of the tools compared
in docs/roadmap/active/04-production-sandbox.md and from what porta's own suite
found, and grows whenever either finds something new.
"""
import json
import os
import pathlib
import platform
import shutil
import subprocess
import sys
import tempfile
import time

SYSTEM = platform.system()
MACHINE = platform.machine()

HELD, ESCAPED, NA = "held", "ESCAPED", "n/a"


class Result:
    def __init__(self, verdict, detail=""):
        self.verdict = verdict
        self.detail = detail


class HarnessError(Exception):
    """The harness itself did not run what it meant to. Never a verdict: a
    corpus whose runs silently did nothing would report every row held."""


def usage_returned(result):
    return "porta run <target>" in result.stdout or "porta run <target>" in result.stderr


class Attempt:
    """One way out. `run` returns a Result; the harness supplies the porta
    binary and the two directories every attempt shares."""

    def __init__(self, name, category, platforms, run):
        self.name = name
        self.category = category
        self.platforms = platforms  # None = every platform
        self._run = run

    def applies(self):
        return self.platforms is None or SYSTEM in self.platforms

    def run(self, ctx):
        return self._run(ctx)


class Context:
    """What every attempt is handed: the binary, a granted writable mount, an
    ungranted directory outside every always-writable root, and a decoy
    process holding a secret in its arguments."""

    def __init__(self, porta, workspace, ungranted):
        self.porta = porta
        self.workspace = workspace      # granted with -v; writable
        self.ungranted = ungranted      # under $HOME, granted to nothing
        self.decoy = None

    def porta_run(self, *args, policy=(), env=None):
        # The target is the first bare word before `--`; only its own arguments
        # go after. A harness that put the target after `--` got porta's usage
        # text and exit 0 back, ran nothing, and every absence-based row passed
        # — which is why `usage_returned` below is fatal, not a verdict.
        target, rest = args[0], args[1:]
        argv = [self.porta, "run", target, *policy, "--", *rest]
        run_env = {**os.environ, **(env or {})}
        try:
            result = subprocess.run(argv, capture_output=True, text=True, timeout=30, env=run_env)
        except subprocess.TimeoutExpired:
            return subprocess.CompletedProcess(argv, 124, "", "timed out")
        if usage_returned(result):
            raise HarnessError(f"porta printed its usage instead of running: {' '.join(argv)}")
        return result

    def py(self, code, *args, policy=()):
        return self.porta_run(sys.executable, "-c", code, *map(str, args), policy=policy)


# --- attempts, grouped by what they reach for ------------------------------
# Each returns HELD when the escape was stopped, ESCAPED when it got through,
# NA when this host cannot host the attempt. The marker MUST-NOT-LEAK never
# appears in a held run's output.

MARKER = "MUST-NOT-LEAK"


def _wrote_outside(ctx, target):
    """A write to `target` (outside every mount) that must not land."""
    result = ctx.porta_run("/bin/sh", "-c", 'echo x > "$1"', "sh", str(target), policy=["-v", str(ctx.workspace)])
    if target.exists():
        target.unlink()
        return Result(ESCAPED, "the write landed")
    return Result(HELD, "write refused")


def write_outside_mount(ctx):
    return _wrote_outside(ctx, ctx.ungranted / "escape")


def rename_mount_root(ctx):
    moved = pathlib.Path(str(ctx.workspace) + ".moved")
    result = ctx.porta_run("/bin/sh", "-c", f'mv "$1" "{moved}"', "sh", str(ctx.workspace),
                           policy=["-v", str(ctx.workspace), "-v", str(ctx.workspace.parent)])
    if moved.exists():
        moved.rename(ctx.workspace)
        return Result(ESCAPED, "mount root renamed away")
    return Result(HELD, "rename refused")


def write_git_hook(ctx):
    repo = ctx.workspace / "repo"
    if not (repo / ".git").exists():
        repo.mkdir(exist_ok=True)
        if subprocess.run(["git", "init", "-q", str(repo)]).returncode != 0:
            return Result(NA, "git not available")
    hook = repo / ".git" / "hooks" / "pre-commit"
    ctx.porta_run("/bin/sh", "-c", 'echo x > "$1"', "sh", str(hook), policy=["-v", str(repo)])
    if hook.exists():
        hook.unlink()
        return Result(ESCAPED, "hook written inside a mount")
    return Result(HELD, "hook write refused")


def write_through_symlink(ctx):
    # A symlink the run makes inside its own mount, pointing outside it. The
    # kernel resolves the link to its target, so the write must be judged by the
    # target's path, not the link's: porta names the path the kernel resolved.
    target = ctx.ungranted / "via-symlink"
    ctx.porta_run("/bin/sh", "-c", 'ln -sf "$1" "$2/link" && echo x > "$2/link"',
                  "sh", str(target), str(ctx.workspace), policy=["-v", str(ctx.workspace)])
    if target.exists():
        target.unlink()
        return Result(ESCAPED, "the write reached the symlink's target outside the mount")
    return Result(HELD, "write through a symlink to outside refused")


def read_secret_through_symlink(ctx):
    # The same trick for reading: a symlink inside the mount to a file outside
    # every grant, opened under strict reads. Resolving the link must not open a
    # door the policy did not.
    secret_dir = ctx.ungranted / "creds"
    secret_dir.mkdir(parents=True, exist_ok=True)
    (secret_dir / "token").write_text(f"token={MARKER}\n")
    result = ctx.porta_run("/bin/sh", "-c", 'ln -sf "$1" "$2/k" && cat "$2/k" 2>/dev/null',
                           "sh", str(secret_dir / "token"), str(ctx.workspace),
                           policy=["--read-policy", "strict", "-v", str(ctx.workspace)])
    return Result(ESCAPED, "read the secret through a symlink") if MARKER in result.stdout else Result(HELD, "read through a symlink refused")


def inherit_porta_fd(ctx):
    # In proxy mode porta holds a listening socket, and with --proxy-audit an
    # open log, while it supervises the child. None of porta's descriptors may
    # cross into the child: an fd to the proxy, the audit log or the journal
    # would be a capability the policy never granted. std opens them O_CLOEXEC;
    # this confirms nothing beyond stdio survives the exec.
    code = (
        "import os\n"
        "leaked=[]\n"
        "for fd in range(3, 64):\n"
        "    try:\n"
        "        os.fstat(fd); leaked.append(fd)\n"
        "    except OSError:\n"
        "        pass\n"
        "print('LEAK' if leaked else 'clean')"
    )
    result = ctx.py(code, policy=["--proxy-allow", "example.com",
                                  "--proxy-audit", str(ctx.workspace / "audit.jsonl"),
                                  "-v", str(ctx.workspace)])
    return Result(ESCAPED, "a porta descriptor crossed into the child") if "LEAK" in result.stdout else Result(HELD, "no descriptor beyond stdio inherited")


def _read_secret(ctx, secret_dir, name, policy):
    """A read of a file the sandbox should not open. The file holds MARKER;
    the probe prints whatever it managed to read."""
    secret = secret_dir / name
    secret_dir.mkdir(parents=True, exist_ok=True)
    secret.write_text(f"token={MARKER}\n")
    result = ctx.porta_run("/bin/cat", str(secret), policy=policy,
                           env={"HOME": str(secret_dir.parent)} if ".ssh" in str(secret) else None)
    return Result(ESCAPED, "secret read") if MARKER in result.stdout else Result(HELD, "read refused")


def read_ssh_key(ctx):
    home = ctx.ungranted / "home"
    # On macOS ~/.ssh is closed in every mode. On Linux the home directory is
    # closed under strict, so the key sits outside the strict read set; the
    # policies differ but the promise — an SSH key the run was not granted
    # stays unread — is the same, so the row runs on both.
    policy = ["-v", str(home)]
    if SYSTEM == "Linux":
        policy = ["--read-policy", "strict", "-v", str(ctx.workspace),
                  "-v", str(pathlib.Path(sys.base_prefix).resolve()) + ":ro"]
        home = ctx.ungranted / "elsewhere"
    return _read_secret(ctx, home / ".ssh", "id_ed25519", policy=policy)


def read_etc_shadow_strict(ctx):
    if SYSTEM != "Linux":
        return Result(NA, "the /etc file-allowlist is Linux-only; macOS grants /private/etc whole")
    result = ctx.porta_run("/bin/sh", "-c", "cat /etc/shadow 2>/dev/null && echo OPENED || echo closed",
                           policy=["--read-policy", "strict", "-v", str(ctx.workspace)])
    return Result(ESCAPED, "/etc/shadow readable under strict") if "OPENED" in result.stdout else Result(HELD, "/etc/shadow closed")


def read_other_process_argv(ctx):
    if ctx.decoy is None:
        return Result(NA, "no decoy process")
    if SYSTEM == "Darwin":
        code = (
            "import ctypes,sys\n"
            "libc=ctypes.CDLL(None,use_errno=True)\n"
            "mib=(ctypes.c_int*3)(1,49,int(sys.argv[1]))\n"
            "size=ctypes.c_size_t(1<<20); buf=ctypes.create_string_buffer(size.value)\n"
            "rc=libc.sysctl(mib,3,buf,ctypes.byref(size),None,0)\n"
            f"print('LEAK' if not rc and b'{MARKER}' in buf.raw[:size.value] else 'closed')"
        )
    else:
        code = (
            "import sys\n"
            "try:\n"
            "    data=open('/proc/'+sys.argv[1]+'/cmdline','rb').read()\n"
            f"    print('LEAK' if b'{MARKER}' in data else 'closed')\n"
            "except OSError: print('closed')"
        )
    policy = ["-v", str(ctx.workspace)]
    if SYSTEM == "Linux":
        policy = ["--read-policy", "strict", "-v", str(ctx.workspace),
                  "-v", str(pathlib.Path(sys.base_prefix).resolve()) + ":ro"]
    result = ctx.py(code, ctx.decoy.pid, policy=policy)
    return Result(ESCAPED, "another process's arguments read") if "LEAK" in result.stdout else Result(HELD, "process arguments closed")


def read_keychain(ctx):
    if SYSTEM != "Darwin":
        return Result(NA, "Keychain is macOS-only")
    result = ctx.porta_run("/usr/bin/security", "list-keychains")
    return Result(ESCAPED, "keychain reachable") if "login.keychain" in result.stdout else Result(HELD, "keychain closed")


def launch_outside_sandbox(ctx):
    if SYSTEM != "Darwin":
        return Result(NA, "open(1)/Launch Services is macOS-only")
    # `open` with a bogus app fails either way; what matters is whether lsopen
    # was permitted. A denied lsopen surfaces as a Launch Services error, an
    # allowed one as "application not found".
    result = ctx.porta_run("/usr/bin/open", "-a", "PortaNoSuchApp")
    text = (result.stdout + result.stderr).lower()
    if "sandbox" in text or "not permitted" in text or "operation not" in text:
        return Result(HELD, "lsopen refused")
    return Result(ESCAPED, "Launch Services reachable") if "unable to find" in text or "cannot be found" in text else Result(HELD, "lsopen refused")


def _egress_tool(ctx, argv, policy):
    """A network tool that should be unable to reach the internet."""
    result = ctx.porta_run(*argv, policy=policy)
    return result


def direct_tcp_wrong_port(ctx):
    # The network is closed to everything but port 80; an HTTPS fetch on 443
    # must fail.
    result = _egress_tool(ctx, ["/usr/bin/curl", "-sS", "-m", "8", "-o", "/dev/null", "-w", "%{http_code}", "https://example.com"],
                          policy=["--allow-net", "*:80"])
    if not shutil.which("curl") and not os.path.exists("/usr/bin/curl"):
        return Result(NA, "curl not installed")
    return Result(ESCAPED, f"reached 443 under *:80 (HTTP {result.stdout.strip()})") if result.stdout.strip().startswith("2") else Result(HELD, "443 refused under *:80")


def reach_cloud_metadata(ctx):
    # The cloud metadata endpoint (169.254.169.254) is the classic agent
    # exfil/credential target. Even placed on the proxy allow-list it must be
    # refused: the proxy resolves the target and a link-local / metadata address
    # is never reached by name, whatever the list says.
    if not shutil.which("curl") and not os.path.exists("/usr/bin/curl"):
        return Result(NA, "curl not installed")
    result = _egress_tool(ctx, ["/usr/bin/curl", "-sS", "-m", "6", "-o", "/dev/null",
                                "-w", "%{http_code}", "https://169.254.169.254/"],
                          policy=["--proxy-allow", "169.254.169.254", "-v", str(ctx.workspace)])
    reached = result.stdout.strip().startswith("2")
    return Result(ESCAPED, f"reached cloud metadata (HTTP {result.stdout.strip()})") if reached else Result(HELD, "cloud metadata refused even when allow-listed")


def fork_past_process_ceiling(ctx):
    # A fork bomb is the cheapest way to take a host down from inside a file
    # policy. --max-procs is RLIMIT_NPROC, set between fork and exec, so the
    # kernel refuses the fork itself; a ceiling of one leaves nothing to spawn.
    # Twenty attempts, each child exiting at once, so a failure is still bounded.
    code = (
        "import os\n"
        "spawned = 0\n"
        "for _ in range(20):\n"
        "    try:\n"
        "        pid = os.fork()\n"
        "    except OSError:\n"
        "        continue\n"
        "    if pid == 0:\n"
        "        os._exit(0)\n"
        "    os.waitpid(pid, 0); spawned += 1\n"
        "print('SPAWNED', spawned)"
    )
    result = ctx.py(code, policy=["--max-procs", "1"])
    if "SPAWNED 0" in result.stdout:
        return Result(HELD, "every fork refused under --max-procs 1")
    return Result(ESCAPED, f"forked past the process ceiling: {result.stdout.strip() or result.stderr.strip()}")


def grow_file_past_ceiling(ctx):
    # Filling the disk is the other cheap denial of service a writable mount
    # allows. --max-file-size is RLIMIT_FSIZE: the write past the ceiling ends
    # the process with SIGXFSZ and the file stops there, measured from the host.
    target = ctx.workspace / "grow.bin"
    code = (
        "import sys\n"
        "with open(sys.argv[1], 'wb') as f:\n"
        "    for _ in range(3):\n"
        "        f.write(b'x' * 1024 * 1024)\n"
        "print('WROTE')"
    )
    ctx.py(code, target, policy=["-v", str(ctx.workspace), "--max-file-size", "1"])
    size = target.stat().st_size if target.exists() else 0
    if size <= 1024 * 1024:
        return Result(HELD, f"file stopped at {size} bytes under a 1 MiB ceiling")
    return Result(ESCAPED, f"file grew to {size} bytes past a 1 MiB ceiling")


def burn_cpu_past_ceiling(ctx):
    # A looping agent under --max-cpu is ended by the kernel with SIGXCPU once
    # its CPU seconds are spent, whatever it is doing; the harness's own 30 s
    # guard is the escape signal.
    started = time.monotonic()
    result = ctx.porta_run("/bin/sh", "-c", "while :; do :; done", policy=["--max-cpu", "1"])
    elapsed = time.monotonic() - started
    if result.returncode == 124 or elapsed > 10:
        return Result(ESCAPED, f"CPU burn ran {elapsed:.0f}s past a 1 s ceiling")
    return Result(HELD, f"CPU burn ended by the kernel after {elapsed:.1f}s (exit {result.returncode})")


def ignore_sigxcpu(ctx):
    # The obvious way around a CPU rlimit is to ignore the signal it sends.
    # Linux follows with SIGKILL at the hard limit; macOS never does, so there
    # porta's supervisor measures the process group's CPU and kills it itself.
    started = time.monotonic()
    result = ctx.porta_run("/bin/sh", "-c", 'trap "" XCPU; while :; do :; done', policy=["--max-cpu", "1"])
    elapsed = time.monotonic() - started
    if result.returncode == 124 or elapsed > 10:
        return Result(ESCAPED, f"ignoring SIGXCPU kept the CPU burn alive for {elapsed:.0f}s")
    return Result(HELD, f"killed after {elapsed:.1f}s despite ignoring SIGXCPU (exit {result.returncode})")


def udp_under_allow_net(ctx):
    if SYSTEM != "Linux":
        return Result(NA, "UDP under --allow-net is only closed on Linux via seccomp; macOS closes it via the profile, tested elsewhere")
    code = (
        "import socket\n"
        "try:\n"
        "    socket.socket(socket.AF_INET, socket.SOCK_DGRAM); print('OPENED')\n"
        "except OSError as e: print('closed', e.errno)"
    )
    # proxy mode is where UDP is closed; --allow-net leaves it open by design.
    result = ctx.py(code, policy=["--proxy-allow", "example.com"])
    return Result(ESCAPED, "UDP socket opened in proxy mode") if "OPENED" in result.stdout else Result(HELD, "UDP closed in proxy mode")


def io_uring(ctx):
    if SYSTEM != "Linux":
        return Result(NA, "io_uring is Linux-only")
    code = (
        "import ctypes\n"
        "libc=ctypes.CDLL(None,use_errno=True)\n"
        "r=libc.syscall(425,1,0)\n"
        "print('OPENED' if r>=0 else 'closed')"
    )
    result = ctx.py(code, policy=["--proxy-allow", "example.com"])
    return Result(ESCAPED, "io_uring_setup succeeded") if "OPENED" in result.stdout else Result(HELD, "io_uring refused")


def fileless_exec(ctx):
    if SYSTEM != "Linux":
        return Result(NA, "execveat(AT_EMPTY_PATH) fileless exec is Linux-only")
    code = (
        "import ctypes,os\n"
        "libc=ctypes.CDLL(None,use_errno=True)\n"
        "fd=libc.memfd_create(b'x',0)\n"
        "if fd<0: print('closed'); raise SystemExit\n"
        "os.write(fd, open('/bin/true','rb').read())\n"
        "r=libc.syscall(322 if __import__('platform').machine()=='x86_64' else 281, fd, b'', 0, 0, 0x1000)\n"
        "print('closed' if r<0 else 'EXECVED')"
    )
    result = ctx.py(code, policy=["-v", str(ctx.workspace)])
    return Result(ESCAPED, "fileless execveat ran") if "EXECVED" in result.stdout else Result(HELD, "fileless exec refused")


def ptrace_sibling(ctx):
    if SYSTEM != "Linux":
        return Result(NA, "ptrace probe is Linux-only here")
    code = (
        "import ctypes\n"
        "libc=ctypes.CDLL(None,use_errno=True)\n"
        "import ctypes as c\n"
        "r=libc.ptrace(0, 1, 0, 0)\n"  # PTRACE_TRACEME=0 is harmless; a denied ptrace(2) returns EPERM
        "print('closed' if r<0 else 'TRACED')"
    )
    result = ctx.py(code, policy=["-v", str(ctx.workspace)])
    return Result(ESCAPED, "ptrace permitted") if "TRACED" in result.stdout else Result(HELD, "ptrace refused")


def new_namespace(ctx):
    if SYSTEM != "Linux":
        return Result(NA, "unshare is Linux-only")
    code = (
        "import ctypes\n"
        "libc=ctypes.CDLL(None,use_errno=True)\n"
        "r=libc.unshare(0x10000000)\n"  # CLONE_NEWUSER
        "print('closed' if r<0 else 'UNSHARED')"
    )
    result = ctx.py(code, policy=["-v", str(ctx.workspace)])
    return Result(ESCAPED, "unshare(CLONE_NEWUSER) permitted") if "UNSHARED" in result.stdout else Result(HELD, "unshare refused")


def mptcp_socket(ctx):
    if SYSTEM != "Linux":
        return Result(NA, "MPTCP is a Linux concern")
    code = (
        "import socket\n"
        "try:\n"
        "    socket.socket(socket.AF_INET, socket.SOCK_STREAM, 262); print('OPENED')\n"
        "except OSError as e: print('closed', e.errno)"
    )
    result = ctx.py(code, policy=["--proxy-allow", "example.com"])
    return Result(ESCAPED, "MPTCP socket opened") if "OPENED" in result.stdout else Result(HELD, "MPTCP refused")


def raw_socket(ctx):
    code = (
        "import socket\n"
        "try:\n"
        "    socket.socket(socket.AF_INET, socket.SOCK_RAW, 1); print('OPENED')\n"
        "except OSError as e: print('closed', e.errno)"
    )
    result = ctx.py(code, policy=["--proxy-allow", "example.com"] if SYSTEM == "Linux" else ["--allow-net", "*:443"])
    if SYSTEM not in ("Linux", "Darwin"):
        return Result(NA, "no backend")
    return Result(ESCAPED, "raw socket opened") if "OPENED" in result.stdout else Result(HELD, "raw socket refused")


CORPUS = [
    Attempt("write outside every mount", "filesystem", None, write_outside_mount),
    Attempt("rename the mount root away", "filesystem", ["Darwin"], rename_mount_root),
    Attempt("write a git hook inside a mount", "filesystem", ["Darwin"], write_git_hook),
    Attempt("write through a symlink pointing outside the mount", "filesystem", None, write_through_symlink),
    Attempt("read a secret through a symlink under strict", "credentials", None, read_secret_through_symlink),
    Attempt("inherit an open file descriptor from porta", "processes", None, inherit_porta_fd),
    Attempt("read an SSH private key", "credentials", None, read_ssh_key),
    Attempt("read /etc/shadow under strict", "credentials", ["Linux"], read_etc_shadow_strict),
    Attempt("read another process's arguments", "processes", None, read_other_process_argv),
    Attempt("read the login Keychain", "credentials", ["Darwin"], read_keychain),
    Attempt("start a program outside the sandbox", "processes", ["Darwin"], launch_outside_sandbox),
    Attempt("reach a port the policy did not open", "network", None, direct_tcp_wrong_port),
    Attempt("reach the cloud metadata endpoint via the proxy", "network", None, reach_cloud_metadata),
    Attempt("open a UDP socket in proxy mode", "network", ["Linux"], udp_under_allow_net),
    Attempt("open a socket without socket() via io_uring", "network", ["Linux"], io_uring),
    Attempt("exec a memory file (fileless)", "processes", ["Linux"], fileless_exec),
    Attempt("attach to another process (ptrace)", "processes", ["Linux"], ptrace_sibling),
    Attempt("enter a new user namespace", "processes", ["Linux"], new_namespace),
    Attempt("reach the network over MPTCP", "network", ["Linux"], mptcp_socket),
    Attempt("open a raw socket", "network", None, raw_socket),
    Attempt("fork past --max-procs", "resources", None, fork_past_process_ceiling),
    Attempt("grow a file past --max-file-size", "resources", None, grow_file_past_ceiling),
    Attempt("burn CPU past --max-cpu", "resources", None, burn_cpu_past_ceiling),
    Attempt("ignore SIGXCPU and keep burning", "resources", None, ignore_sigxcpu),
]


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    porta = str(pathlib.Path(sys.argv[1]).resolve())
    json_out = None
    if "--json" in sys.argv:
        json_out = sys.argv[sys.argv.index("--json") + 1]

    # A directory the run is never granted, outside every always-writable root:
    # under $HOME, never /tmp. And a granted writable workspace.
    ungranted = pathlib.Path(tempfile.mkdtemp(prefix="porta-escapes-", dir=pathlib.Path.home()))
    workspace = ungranted / "granted"
    workspace.mkdir()

    decoy = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(120)", f"--token={MARKER}"])
    ctx = Context(porta, workspace, ungranted)
    ctx.decoy = decoy

    rows, findings, tried = [], 0, 0
    try:
        # Before any verdict: prove the harness runs a command at all. A run
        # that should succeed writes into the granted workspace and says so;
        # if it does not, no row below could mean anything.
        canary = workspace / "canary"
        result = ctx.porta_run("/bin/sh", "-c", 'echo alive > "$1" && echo RAN', "sh", str(canary),
                               policy=["-v", str(workspace)])
        if result.returncode != 0 or "RAN" not in result.stdout or not canary.exists():
            raise HarnessError(f"the canary run did not run: exit {result.returncode}, {result.stderr.strip()}")
        for attempt in CORPUS:
            if not attempt.applies():
                rows.append((attempt, Result(NA, f"not applicable on {SYSTEM}")))
                continue
            result = attempt.run(ctx)
            rows.append((attempt, result))
            if result.verdict == ESCAPED:
                findings += 1
            if result.verdict != NA:
                tried += 1
    except HarnessError as error:
        print(f"HARNESS ERROR: {error}\nno verdicts: the corpus did not run, so nothing was proven", file=sys.stderr)
        sys.exit(2)
    finally:
        decoy.terminate()
        decoy.wait()
        shutil.rmtree(ungranted, ignore_errors=True)

    width = max(len(a.name) for a in CORPUS)
    print(f"escape corpus on {SYSTEM}/{MACHINE} — {tried} tried, {findings} escaped\n")
    for attempt, result in rows:
        flag = {HELD: " ok ", ESCAPED: "FAIL", NA: "  - "}[result.verdict]
        print(f"{flag}  {attempt.name:<{width}}  {result.detail}")

    if json_out:
        pathlib.Path(json_out).write_text(json.dumps({
            "system": SYSTEM, "machine": MACHINE, "tried": tried, "escaped": findings,
            "rows": [{"name": a.name, "category": a.category, "verdict": r.verdict, "detail": r.detail} for a, r in rows],
        }, indent=2))

    print()
    if findings:
        print(f"{findings} escape(s) got through. Each is a hole to close.")
        sys.exit(1)
    print("Every attempt this host could make was held.")


if __name__ == "__main__":
    main()
