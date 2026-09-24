# What Porta enforces, per platform

What porta defends against, what it does not, and the test behind each claim
are in [the threat model](threat-model.md). The escape corpus that backs it —
every way out this project knows about, run against the binary — is
`scripts/escapes.py`; it runs in CI and you can run it yourself:

```bash
python3 scripts/escapes.py "$(command -v porta)"
```

Published results are in [benchmarks/escapes.md](benchmarks/escapes.md), losing
rows included.

## Two layers

1. **OS layer** — `sandbox-exec` on macOS, Landlock plus a seccomp filter on
   Linux. Filesystem and network restrictions the child process cannot lift,
   because they are applied to it before it starts.
2. **MCP layer** — application-level host+port URL filtering and capability
   checks on the `porta.exec` and `porta.http` built-in tools.

## Native restrictions

One table, because the two platforms differ and reading two near-identical ones
does not show you where.

| Control | macOS (`sandbox-exec`) | Linux (Landlock + seccomp) |
|---|---|---|
| **Write** | denied outside `-v` mounts, `/tmp`, `/dev` | denied outside `-v` mounts, `/tmp`, `/dev` |
| **Inside a writable mount** | the existing repository's `.git/hooks` and `.git/config`, and the names the preset protects (shell rc files, `.gitconfig`, `.mcp.json`, `.envrc`, `.claude/commands`, `.vscode`, `porta.toml`, …) stay unwritable; the mount root and those paths cannot be renamed away | same, each bind-mounted read-only onto itself in the command's mount namespace; where the host refuses one, not protected (the run says so) |
| **Read, default** | what the preset closes denied — by default credential stores: `~/.ssh`, `~/.gnupg`, `~/.aws`, `~/.config/gh`, `~/.config/gcloud`, `~/.docker`, `~/.kube`, `~/.netrc`, Keychains, browser profiles, …; everything else readable | same, each covered by an empty mount in the command's mount namespace; where the host refuses one, Landlock grants reads everywhere else, and a closed path inside a grant (`-v ~`, `/tmp`) refuses the run |
| **Environment** | empty, plus `PATH` `HOME` `USER` `LOGNAME` `SHELL` `TERM` `COLORTERM` `LANG` `LANGUAGE` `LC_*` `TZ`, `-e` and `--env-pass` | same |
| **Other processes** | their arguments and environment unreadable (`procargs`, `proc_pidinfo`); signals to them not restricted | invisible: the command runs in PID, mount and user namespaces of its own with a fresh `/proc`, where the host allows unprivileged user namespaces (otherwise porta says so and only `strict` closes `/proc`); signals and abstract sockets scoped to the sandbox on Landlock ABI 6 |
| **Host facilities** | Keychain, `open(1)`/Launch Services, mounting, disk and packet devices, Apple Events, network-share agents closed | `ptrace`, `process_vm_*`, `pidfd_getfd`, `mount*`, `unshare`/`setns`/`clone(CLONE_NEW*)`, `bpf`, `perf_event_open`, `userfaultfd`, `keyctl`, `io_uring`, `clone3`, `execveat(AT_EMPTY_PATH)`, kernel modules, `TIOCSTI` refused by seccomp in every mode |
| **Read, `--read-policy strict`** | your mounts plus `/usr`, `/System`, `/bin`, `/sbin`, `/etc`, `/tmp`, `/dev` | your mounts plus `/usr`, `/lib`, `/bin`, `/sbin`, `/tmp`, `/dev`, and under `/etc` only the files a command needs to start (loader cache, resolver, trust store, `passwd`, `localtime`…) — never `shadow`, `sudoers` or the host keys, and not the listing |
| **Read-only mount** | `-v ./data:ro` → read yes, write no | same |
| **No network (`--no-net`)** | every outbound, bind and inbound denied, the resolver included | a network namespace of its own with only a loopback interface, where the host allows unprivileged user namespaces; otherwise Landlock closes every TCP port and seccomp every other family |
| **Network by port** | `--allow-net '*:443'`; UDP closed | same, needs Landlock ABI 4; UDP, SCTP and ICMP closed by seccomp, and names resolve over TCP 53 (`RES_OPTIONS=use-vc`) |
| **Credential sockets** | the preset's patterns refused at `connect` unless `--allow-unix` names the path | the matching sockets bound at the start covered with `/dev/null` in the command's mount namespace; where the host refuses one, reachable (the run names them) |
| **Network by host** | `--proxy-allow` only, never `--allow-net` | same |
| **Proxy mode** | enforced by the profile | enforced by Landlock (the TCP port) plus seccomp (everything else) |

What the table calls the preset is data, not code: `native/presets/default.toml`,
built into the binary. It lists the paths closed to reads, the names protected
inside every writable mount and the Unix sockets refused, and `porta explain`
prints what a run resolved it to. `--deny-read`, `--protect` and `--deny-unix`
add to it; `--preset <file>` starts from another document and `--preset none`
from nothing. The mechanisms are porta's; which paths deserve them is yours.

Under `strict`, every home directory is closed — and so is the command itself if
it lives outside those directories. A toolchain under `/opt` needs a mount on
its own installation, and it wants the read-only form: `-v /opt/toolchain:ro`
leaves the interpreter runnable while `-v /opt/toolchain` would also let the
agent rewrite it. porta says which grant is missing rather than failing with a
bare `Permission denied`.

Linux uses Landlock unprivileged, without namespaces and without an external
runtime. A rule the running kernel cannot express refuses the run rather than
widening it: partial enforcement is never silently accepted.

## Host-filtered HTTPS

```bash
porta run claude -v . --proxy-allow "api.anthropic.com,*.anthropic.com"
```

Or add `[proxy]` with `allow = ["api.anthropic.com"]` to `porta.toml`. Both
`run` and `up` apply the same policy. The child can reach only the local CONNECT
proxy; direct TCP, UDP and Unix-socket egress are denied, on Linux by a seccomp
filter that also refuses `io_uring`, because a ring can open a socket without
ever asking for one.

Only HTTPS CONNECT on port 443 is supported, and clients must respect
`HTTPS_PROXY` (Node's `fetch` does once `NODE_USE_ENV_PROXY=1` is set, which
porta sets). The proxy serves this run's client alone: `HTTPS_PROXY` carries a
per-run credential, and a CONNECT without it is answered 407, so another
process on the machine cannot use porta's proxy as a relay carrying this run's
allow-list. An allowed name that resolves to loopback, a link-local or cloud
metadata address, or a multicast group is refused: a hostname on the list is a
promise about a public service, not a route to this machine. Private ranges
stay reachable. Every decision is written and synced to the audit file before
the connection proceeds. Deny lists are weaker than explicit allow lists, and
this is not a credential broker.

## WASM sandbox

Deny-by-default capability system. Every WASI import is validated against the
capability set before execution.

| Capability | Controls |
|------------|----------|
| `io` | stdin/stdout/stderr |
| `fs` | File read (path_open, stat, readdir) |
| `fs.write` | File write (create, rename, delete) |
| `process` | Process lifecycle, args |
| `env` | Environment variables |
| `clock` | Time/clock |
| `random` | Random bytes |
| `net` | Network access |
| `exec` | Command execution |

Built-in profiles: `ai-agent` (IO + Process), `worker` (+Clock +Random), `full`
(all). Manifest capabilities are respected in both `serve` and `run` modes.

A WASI 0.2 component (`almide build --target wasm --component`, or anything
that targets `wasi:cli/command`) runs under the same check. Its imports are
interfaces rather than functions, and each maps to the capabilities above:
`wasi:cli/std*` and `wasi:io/*` to `io`, `wasi:cli/exit` to `process`,
`wasi:cli/environment` to `process` and `env`, `wasi:clocks/*` to `clock`,
`wasi:random/*` to `random`, `wasi:filesystem/*` to `fs` and `fs.write` (one
interface carries both), `wasi:sockets/*` and `wasi:http/*` to `net`. An
interface not in that list is refused. Fuel, memory and preopens apply as to a
module; the entry is `wasi:cli/run`, so `--entry` does not apply.

A WASI 0.3 component (Almide: `ALMIDE_COMPONENT_P3=1` with `--component`) is
recognised by its `@0.3` imports and run through the async linker and the
store's concurrent executor, under the same check and budgets. Its world
imports the filesystem interfaces whether or not the program touches a file,
so it needs `fs` and `fs.write` — the `full` profile, or a manifest that
grants them. wasmtime's 0.3 support is marked experimental upstream, and
porta says the same of it.
Direct `porta.exec_command` / `porta.http_request` WASM imports are rejected;
host execution and HTTP requests go through the checked MCP built-in tools.

## What Porta does not do

- **It is not a container or a VM.** Restrictions are applied to a process on
  your kernel. A kernel that can be exploited is a kernel both sides share.
- **Read access is broader than write access** unless you pass
  `--read-policy strict`, and even then the system directories a command needs
  to start stay readable. Credential stores under your home are closed in every
  mode; the rest of what your user can read, a command can read too.
- **Proxy filtering controls connection targets, not TLS contents.** Listening
  is closed once `--allow-net` is in force and opened per port with
  `--allow-bind`; with the network open, so is listening.
- **Namespaces need the host's consent.** The command's own PID, mount and
  network namespaces need unprivileged user namespaces. Ubuntu from 23.10
  restricts them through AppArmor, and most container runtimes refuse them.
  There porta runs without them and says so, `porta check` shows it, and
  `--no-net` falls back to Landlock and seccomp. On Ubuntu,
  `sudo bash scripts/apparmor-userns.sh "$(command -v porta)"` loads the
  profile Ubuntu documents for a program that needs them, for porta alone.
- **Linux protects inside a mount, and closes credential sockets, only in a
  mount namespace.** Landlock grants a directory whole and has no rule over a
  Unix socket's `connect`, so both need the namespace; where the host refuses
  one they are not applied, and the run says so. A credential socket bound
  after the run starts is not covered on Linux.
- **Under `--allow-net`, names resolve over TCP on Linux.** UDP is closed, so a
  resolver that does not follow `RES_OPTIONS=use-vc` (musl, a static Go binary)
  cannot resolve; use `--proxy-allow`, where the proxy resolves.
- **Resource ceilings are what an unprivileged process can place.**
  `--timeout`, `--max-cpu`, `--max-procs` and `--max-file-size` are a process
  group kill and rlimits the kernel enforces on every descendant, per process.
  `--max-memory-mb` is the one per-run ceiling. On Linux it is cgroup v2
  `memory.max` with swap closed, placed by asking the systemd user manager for
  a transient scope around the command before it runs its first instruction;
  it needs a user manager (a logind session, or `loginctl enable-linger`) and
  the flag refuses the run where there is none. On macOS, which has no
  cgroup, porta's supervisor reads the group's physical footprint every
  quarter second and ends the group at the ceiling: a bound, not a kernel
  limit, so a burst can pass it briefly before the kill. The WASM runtime caps
  memory on every platform.
- **macOS and Linux only**, and not identically. Anywhere else, native
  execution fails closed rather than running unrestricted.
- **Nothing is mounted implicitly.** `porta run` and `porta serve` see no
  directory until you pass `-v`.
