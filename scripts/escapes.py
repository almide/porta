#!/usr/bin/env python3
"""The escape corpus: every way out of the sandbox this project knows about,
tried against a porta binary, one attempt per row.

    python3 scripts/escapes.py target/porta            # table on stdout
    python3 scripts/escapes.py target/porta --json out.json
    python3 scripts/escapes.py path/to/srt --runner srt # the same rows, another tool

`--runner` is porta (the default), srt, fence or nono; see
scripts/escape_runners.py for how each row's policy is translated.

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
import socket
import subprocess
import sys
import tempfile
import time

from escape_runners import NotExpressible, environment, make_runner

SYSTEM = platform.system()
MACHINE = platform.machine()

HELD, ESCAPED, NA, UNOFFERED = "held", "ESCAPED", "n/a", "not offered"


class Result:
    def __init__(self, verdict, detail=""):
        self.verdict = verdict
        self.detail = detail


class HarnessError(Exception):
    """The harness itself did not run what it meant to. Never a verdict: a
    corpus whose runs silently did nothing would report every row held."""


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


class ProbeDidNotStart(Exception):
    """The row's probe never ran under this tool's policy: its interpreter or
    tool could not start. An escape that was never attempted was not held."""


PROBE_STARTED = "PROBE-STARTED"

# The interpreter itself, not a launcher shim in front of it: a shim under the
# home directory is a path a strict tool rightly will not open, and the probe
# should not fail for a reason that has nothing to do with the row.
_real = pathlib.Path(sys.base_prefix) / "bin" / "python3"
PYTHON = str(_real) if _real.exists() else sys.executable


class Context:
    """What every attempt is handed: the tool under test, a granted writable
    mount, an ungranted directory outside every always-writable root, and a
    decoy process holding a secret in its arguments."""

    def __init__(self, runner, workspace, ungranted):
        self.runner = runner
        self.workspace = workspace      # granted with -v; writable
        self.ungranted = ungranted      # under $HOME, granted to nothing
        self.decoy = None
        # A tool starts where a user would start it: in the project it was
        # granted, the first writable mount, or an empty directory when the row
        # grants none. Some tools find the files they protect relative to the
        # directory they start in, and a harness that started them elsewhere
        # would score them on a protection it had switched off.
        self.empty_dir = tempfile.mkdtemp(prefix="escapes-cwd-")

    def porta_run(self, *args, policy=(), env=None):
        # Every row states its policy in porta's flags; the runner translates
        # them for the tool under test, or raises NotExpressible. A tool that
        # refused its own configuration, or porta printing its usage, is a
        # harness error, never a verdict: a run that did nothing would read as
        # held on every absence-based row.
        argv = self.runner.argv(args[0], list(args[1:]), list(policy))
        mounts = [policy[i + 1] for i in range(len(policy) - 1) if policy[i] == "-v" and not policy[i + 1].endswith(":ro")]
        start_dir = mounts[0] if mounts and os.path.isdir(mounts[0]) else self.empty_dir
        try:
            result = subprocess.run(argv, capture_output=True, text=True, timeout=30,
                                    env=environment(env), stdin=subprocess.DEVNULL, cwd=start_dir)
        except subprocess.TimeoutExpired:
            return subprocess.CompletedProcess(argv, 124, "", "timed out")
        if self.runner.refused_config(result):
            raise HarnessError(f"{self.runner.name} refused the translated configuration: "
                               f"{(result.stdout + result.stderr).strip()[:300]}")
        return result

    def py(self, code, *args, policy=()):
        # The probe says it started before anything else, so a row can tell
        # "the escape was stopped" from "the probe never ran".
        started = f"import sys\nsys.stdout.write('{PROBE_STARTED}\\n'); sys.stdout.flush()\n"
        result = self.porta_run(PYTHON, "-c", started + code, *map(str, args), policy=policy)
        if PROBE_STARTED not in result.stdout:
            raise ProbeDidNotStart((result.stderr or result.stdout).strip()[:160])
        return result


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
    if getattr(ctx.runner, "passes_supervisor_socket", False):
        return Result(NA, f"{ctx.runner.name} hands the child its own supervisor socket by design")
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
    # The login Keychain's database file, read directly: its first four bytes
    # are the format's magic, "kych". Nothing secret is printed, but a file the
    # run can read is a file it can copy out and attack offline. An earlier
    # version judged `security list-keychains`, which prints the search list
    # from preferences and proves nothing about the Keychain itself.
    if SYSTEM != "Darwin":
        return Result(NA, "Keychain is macOS-only")
    keychain = pathlib.Path.home() / "Library" / "Keychains" / "login.keychain-db"
    if not keychain.exists():
        return Result(NA, "no login keychain on this host")
    result = ctx.porta_run("/usr/bin/head", "-c", "4", str(keychain))
    if not result.stdout and "No such file" in result.stderr:
        raise ProbeDidNotStart(result.stderr.strip()[:160])
    return Result(ESCAPED, "the login keychain database is readable") if result.stdout == "kych" else Result(HELD, "keychain closed")


def launch_outside_sandbox(ctx):
    if SYSTEM != "Darwin":
        return Result(NA, "open(1)/Launch Services is macOS-only")
    # `open(1)` starts a handler outside the sandbox through Launch Services'
    # mach services. Ask the bootstrap server for them directly: a lookup that
    # succeeds is the escape, one the sandbox refuses answers
    # BOOTSTRAP_UNKNOWN_SERVICE (1100). Nothing is launched either way.
    #
    # An earlier version parsed `open`'s messages and took porta's own denial
    # footer as the held signal; `open` says "Unable to find application"
    # whether or not the lookup was denied, so when the footer lagged on a CI
    # runner the row read as an escape. The verdict now depends on nothing
    # porta prints.
    code = (
        "import ctypes\n"
        "lib = ctypes.CDLL(None)\n"
        "bootstrap = ctypes.c_uint.in_dll(lib, 'bootstrap_port')\n"
        "for name in ('com.apple.lsd.mapdb', 'com.apple.lsd.modifydb'):\n"
        "    port = ctypes.c_uint(0)\n"
        "    rc = lib.bootstrap_look_up(bootstrap, name.encode(), ctypes.byref(port))\n"
        "    print(name, 'REACHABLE' if rc == 0 else 'denied %d' % rc)"
    )
    result = ctx.py(code)
    if "REACHABLE" in result.stdout:
        return Result(ESCAPED, "Launch Services reachable: " + result.stdout.strip().replace("\n", "; "))
    return Result(HELD, "Launch Services mach lookups refused")


def _egress_tool(ctx, argv, policy):
    """A network tool that should be unable to reach the internet."""
    result = ctx.porta_run(*argv, policy=policy)
    if not result.stdout.strip():
        raise ProbeDidNotStart(result.stderr.strip()[:160])
    return result


def direct_tcp_wrong_port(ctx):
    # The network is closed to everything but port 80; an HTTPS fetch on 443
    # must fail.
    result = _egress_tool(ctx, ["/usr/bin/curl", "-sS", "-m", "8", "-o", "/dev/null", "-w", "%{http_code}", "https://example.com"],
                          policy=["--allow-net", "*:80"])
    if not shutil.which("curl") and not os.path.exists("/usr/bin/curl"):
        return Result(NA, "curl not installed")
    return Result(ESCAPED, f"reached 443 under *:80 (HTTP {result.stdout.strip()})") if result.stdout.strip().startswith("2") else Result(HELD, "443 refused under *:80")


NO_NET_PROBE = (
    "import socket,sys\n"
    "reached=[]\n"
    "try:\n"
    "    socket.create_connection(('127.0.0.1', int(sys.argv[1])), 3).close(); reached.append('host loopback')\n"
    "except OSError: pass\n"
    "try:\n"
    "    socket.create_connection(('1.1.1.1', 443), 3).close(); reached.append('TCP 1.1.1.1:443')\n"
    "except OSError: pass\n"
    "try:\n"
    "    s=socket.socket(socket.AF_INET, socket.SOCK_DGRAM); s.settimeout(2)\n"
    "    for _ in range(3):\n"
    "        try: s.sendto(bytes.fromhex('abcd01000001000000000000076578616d706c6503636f6d0000010001'), ('1.1.1.1', 53)); s.recvfrom(512); reached.append('UDP 1.1.1.1:53'); break\n"
    "        except OSError: pass\n"
    "except OSError: pass\n"
    "print('REACHED ' + ', '.join(reached) if reached else 'closed')"
)


def no_network_at_all(ctx):
    # --no-net: a service on the host's loopback, TCP to the internet and a UDP
    # DNS question must all go unanswered. The host's loopback is the one a
    # namespace-less filter tends to forget.
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen(8)
    try:
        result = ctx.py(NO_NET_PROBE, listener.getsockname()[1], policy=["--no-net", "-v", str(ctx.workspace)])
    finally:
        listener.close()
    reached = result.stdout.strip().removeprefix("PROBE-STARTED").strip()
    return Result(ESCAPED, reached.lower()) if reached.startswith("REACHED") else Result(HELD, "no loopback service, TCP or UDP reached")


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


def allocate_past_memory_ceiling(ctx):
    # On Linux the ceiling is cgroup v2 memory.max, placed through the systemd
    # user manager, and the kernel OOM-kills the group past it; a host without
    # a user manager cannot host the attempt, and porta refuses the flag there.
    # On macOS porta's supervisor polls the group's footprint and ends it, so
    # the allocation is held for a second to give the poll its chance.
    if SYSTEM == "Linux" and not os.path.exists(f"/run/user/{os.getuid()}/bus"):
        return Result(NA, "no systemd user manager for this user; --max-memory-mb is refused here")
    result = ctx.py("import time\nb = bytearray(200 * 1024 * 1024)\ntime.sleep(1)\nprint('ALLOCATED')", policy=["--max-memory-mb", "64"])
    if "ALLOCATED" in result.stdout:
        return Result(ESCAPED, "allocated 200 MiB under a 64 MiB ceiling")
    return Result(HELD, f"allocation past the ceiling was killed (exit {result.returncode})")


UDP_PROBE = (
    "import socket\n"
    "q=bytes.fromhex('abcd01000001000000000000076578616d706c6503636f6d0000010001')\n"
    "try:\n"
    "    s=socket.socket(socket.AF_INET, socket.SOCK_DGRAM); s.settimeout(2)\n"
    "except OSError as e: print('closed', e.errno); raise SystemExit\n"
    # UDP drops packets; three questions before an answer counts as absent.
    "for _ in range(3):\n"
    "    try: s.sendto(q, ('1.1.1.1', 53)); s.recvfrom(512); print('ANSWERED'); break\n"
    "    except OSError as e: last=e\n"
    "else: print('closed', last)"
)


def udp_under_allow_net(ctx):
    # A DNS question to a public resolver over UDP, and whether an answer
    # comes back. That is egress. Merely opening a socket is not: a tool that
    # isolates the network in a namespace lets the socket open and gives it
    # nowhere to go, and an earlier version of this row scored that as an
    # escape. The same question is asked outside any sandbox first, so a host
    # that cannot reach the resolver at all cannot host the row.
    if SYSTEM != "Linux":
        return Result(NA, "UDP under --allow-net is only closed on Linux via seccomp; macOS closes it via the profile, tested elsewhere")
    control = subprocess.run([PYTHON, "-c", UDP_PROBE], capture_output=True, text=True, timeout=15)
    if "ANSWERED" not in control.stdout:
        return Result(NA, "this host gets no UDP answer from 1.1.1.1:53 unsandboxed")
    result = ctx.py(UDP_PROBE, policy=["--proxy-allow", "example.com"])
    return Result(ESCAPED, "a UDP DNS query got an answer in proxy mode") if "ANSWERED" in result.stdout else Result(HELD, "no UDP egress in proxy mode")


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


USERNS_PROBE = (
    "import ctypes,os\n"
    "libc=ctypes.CDLL(None,use_errno=True)\n"
    "uid,gid=os.getuid(),os.getgid()\n"
    "if libc.unshare(0x10000000|0x00020000|0x20000000): raise SystemExit(1)\n"
    "open('/proc/self/setgroups','w').write('deny')\n"
    "open('/proc/self/uid_map','w').write(f'{uid} {uid} 1')\n"
    "open('/proc/self/gid_map','w').write(f'{gid} {gid} 1')\n"
    "pid=os.fork()\n"
    "if pid==0: os._exit(0 if libc.mount(b'proc',b'/proc',b'proc',0,None)==0 else 1)\n"
    "raise SystemExit(os.waitstatus_to_exitcode(os.waitpid(pid,0)[1]))"
)


def host_gives_pid_namespaces():
    """Whether an unprivileged process here may have its own user, PID and
    mount namespaces and mount a procfs in them: what hiding other processes
    without closing /proc takes, for any tool."""
    return subprocess.run([PYTHON, "-c", USERNS_PROBE], capture_output=True, timeout=15).returncode == 0


def read_other_process_argv_default(ctx):
    # The row above, in the default read mode, where /proc stays readable and
    # only a PID namespace of the command's own hides other processes.
    if SYSTEM != "Linux":
        return Result(NA, "the default read mode is the row above on macOS")
    if ctx.decoy is None:
        return Result(NA, "no decoy process")
    if not ctx.runner.gives_pid_namespaces(host_gives_pid_namespaces):
        return Result(NA, "this host refuses unprivileged PID namespaces, so the default read mode cannot hide other processes")
    code = (
        "import sys\n"
        "try:\n"
        "    data=open('/proc/'+sys.argv[1]+'/cmdline','rb').read()\n"
        f"    print('LEAK' if b'{MARKER}' in data else 'closed')\n"
        "except OSError: print('closed')"
    )
    result = ctx.py(code, ctx.decoy.pid, policy=["-v", str(ctx.workspace)])
    return Result(ESCAPED, "another process's arguments read") if "LEAK" in result.stdout else Result(HELD, "the process is not visible")


def ptrace_sibling(ctx):
    # Attach to the decoy, a process of the same user outside the sandbox.
    # Attached, the run could read its memory and rewrite its registers. An
    # earlier version called PTRACE_TRACEME, which asks nothing of any other
    # process and proved only that ptrace(2) was not filtered.
    if SYSTEM != "Linux":
        return Result(NA, "ptrace probe is Linux-only here")
    code = (
        "import ctypes,sys,os\n"
        "libc=ctypes.CDLL(None,use_errno=True)\n"
        "pid=int(sys.argv[1])\n"
        "r=libc.ptrace(16, pid, 0, 0)\n"  # PTRACE_ATTACH
        "if r==0:\n"
        "    os.waitpid(pid, 0); libc.ptrace(17, pid, 0, 0); print('ATTACHED')\n"  # PTRACE_DETACH
        "else:\n"
        "    print('closed', ctypes.get_errno())"
    )
    yama = pathlib.Path("/proc/sys/kernel/yama/ptrace_scope")
    scope = yama.read_text().strip() if yama.exists() else "absent"
    result = ctx.py(code, ctx.decoy.pid, policy=["-v", str(ctx.workspace)])
    if "ATTACHED" in result.stdout:
        return Result(ESCAPED, f"attached to a process outside the sandbox (yama ptrace_scope {scope})")
    return Result(HELD, f"attach refused (yama ptrace_scope {scope})")


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
    Attempt("read another process's arguments, default reads", "processes", ["Linux"], read_other_process_argv_default),
    Attempt("read the login Keychain", "credentials", ["Darwin"], read_keychain),
    Attempt("reach Launch Services (open(1) starts programs outside the sandbox through it)", "processes", ["Darwin"], launch_outside_sandbox),
    Attempt("reach a port the policy did not open", "network", None, direct_tcp_wrong_port),
    Attempt("reach anything under --no-net", "network", None, no_network_at_all),
    Attempt("reach the cloud metadata endpoint via the proxy", "network", None, reach_cloud_metadata),
    Attempt("get a UDP answer from outside in proxy mode", "network", ["Linux"], udp_under_allow_net),
    Attempt("open a socket without socket() via io_uring", "network", ["Linux"], io_uring),
    Attempt("exec a memory file (fileless)", "processes", ["Linux"], fileless_exec),
    Attempt("attach to a process outside the sandbox (ptrace)", "processes", ["Linux"], ptrace_sibling),
    Attempt("enter a new user namespace", "hardening", ["Linux"], new_namespace),
    Attempt("reach the network over MPTCP", "network", ["Linux"], mptcp_socket),
    Attempt("open a raw socket", "network", None, raw_socket),
    Attempt("fork past --max-procs", "resources", None, fork_past_process_ceiling),
    Attempt("grow a file past --max-file-size", "resources", None, grow_file_past_ceiling),
    Attempt("burn CPU past --max-cpu", "resources", None, burn_cpu_past_ceiling),
    Attempt("ignore SIGXCPU and keep burning", "resources", None, ignore_sigxcpu),
    Attempt("allocate past --max-memory-mb", "resources", None, allocate_past_memory_ceiling),
]


def run_canary(ctx):
    # Before any verdict: prove the harness runs a command at all. A run that
    # should succeed writes into the granted workspace and says so; if it does
    # not, no row below could mean anything.
    canary = ctx.workspace / "canary"
    result = ctx.porta_run("/bin/sh", "-c", 'echo alive > "$1" && echo RAN', "sh", str(canary),
                           policy=["-v", str(ctx.workspace)])
    if result.returncode != 0 or "RAN" not in result.stdout or not canary.exists():
        raise HarnessError(f"the canary run did not run: exit {result.returncode}, {result.stderr.strip()}")


def try_attempt(ctx, attempt):
    if not attempt.applies():
        return Result(NA, f"not applicable on {SYSTEM}")
    try:
        return attempt.run(ctx)
    except NotExpressible as reason:
        return Result(UNOFFERED, str(reason))
    except ProbeDidNotStart as reason:
        return Result(NA, f"the probe could not start under {ctx.runner.name}'s policy: {reason}")


def tally(rows):
    """Counts per verdict. A hardening row is not a way out of the policy; it
    is a step an escape would start from (a new user namespace opens kernel
    code an unprivileged process otherwise cannot reach), so it is counted
    apart from escapes."""
    verdicts = [(a.category, r.verdict) for a, r in rows]
    return {
        "tried": sum(v not in (NA, UNOFFERED) for _, v in verdicts),
        "escaped": sum(v == ESCAPED and c != "hardening" for c, v in verdicts),
        "hardening_gaps": sum(v == ESCAPED and c == "hardening" for c, v in verdicts),
        "not_offered": sum(v == UNOFFERED for _, v in verdicts),
    }


def report(tool, rows, counts, json_out):
    width = max(len(a.name) for a in CORPUS)
    offered = f", {counts['not_offered']} not offered" if counts["not_offered"] else ""
    hardened = f" and {counts['hardening_gaps']} hardening gap(s)" if counts["hardening_gaps"] else ""
    print(f"escape corpus: {tool} on {SYSTEM}/{MACHINE} — {counts['tried']} tried, {counts['escaped']} escaped{hardened}{offered}\n")
    for attempt, result in rows:
        flag = {HELD: " ok ", ESCAPED: "FAIL", NA: "  - ", UNOFFERED: " no "}[result.verdict]
        print(f"{flag}  {attempt.name:<{width}}  {result.detail}")
    if json_out:
        pathlib.Path(json_out).write_text(json.dumps({
            "tool": tool, "system": SYSTEM, "machine": MACHINE, **counts,
            "rows": [{"name": a.name, "category": a.category, "verdict": r.verdict, "detail": r.detail} for a, r in rows],
        }, indent=2))


def option(name):
    return sys.argv[sys.argv.index(name) + 1] if name in sys.argv else None


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    runner = make_runner(option("--runner") or "porta", str(pathlib.Path(sys.argv[1]).resolve()))

    # A directory the run is never granted, outside every always-writable root:
    # under $HOME, never /tmp. And a granted writable workspace.
    ungranted = pathlib.Path(tempfile.mkdtemp(prefix="porta-escapes-", dir=pathlib.Path.home()))
    workspace = ungranted / "granted"
    workspace.mkdir()

    decoy = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(120)", f"--token={MARKER}"])
    ctx = Context(runner, workspace, ungranted)
    ctx.decoy = decoy
    try:
        run_canary(ctx)
        rows = [(attempt, try_attempt(ctx, attempt)) for attempt in CORPUS]
    except HarnessError as error:
        print(f"HARNESS ERROR: {error}\nno verdicts: the corpus did not run, so nothing was proven", file=sys.stderr)
        sys.exit(2)
    finally:
        decoy.terminate()
        decoy.wait()
        shutil.rmtree(ungranted, ignore_errors=True)

    counts = tally(rows)
    report(runner.name, rows, counts, option("--json"))
    print()
    if counts["escaped"] or counts["hardening_gaps"]:
        print(f"{counts['escaped'] + counts['hardening_gaps']} escape(s) or hardening gap(s) got through. Each is a hole to close.")
        sys.exit(1)
    print("Every attempt this host could make was held.")


if __name__ == "__main__":
    main()
