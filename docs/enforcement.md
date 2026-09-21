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
| **Inside a writable mount** | the existing repository's `.git/hooks` and `.git/config`, and the root's shell rc files, `.gitconfig`, `.mcp.json`, `.npmrc`, `.claude/commands`, `.claude/agents`, `.vscode`, `.idea`, `porta.toml` stay unwritable; the mount root and those paths cannot be renamed away | not yet (Landlock grants a directory whole) |
| **Read, default** | credential stores denied — `~/.ssh`, `~/.gnupg`, `~/.aws`, `~/.config/gcloud`, `~/.docker`, `~/.kube`, `~/.netrc`, `~/.npmrc`, `~/.pypirc`, Keychains, browser profiles; everything else readable | not confined |
| **Environment** | empty, plus `PATH` `HOME` `USER` `LOGNAME` `SHELL` `TERM` `COLORTERM` `LANG` `LANGUAGE` `LC_*` `TZ`, `-e` and `--env-pass` | same |
| **Other processes** | their arguments and environment unreadable (`procargs`, `proc_pidinfo`); signals to them not restricted | `/proc` closed under `strict`; signals and abstract sockets scoped to the sandbox on Landlock ABI 6 |
| **Host facilities** | Keychain, `open(1)`/Launch Services, mounting, disk and packet devices, Apple Events, network-share agents closed | `ptrace`, `process_vm_*`, `pidfd_getfd`, `mount*`, `unshare`/`setns`/`clone(CLONE_NEW*)`, `bpf`, `perf_event_open`, `userfaultfd`, `keyctl`, `io_uring`, `clone3`, `execveat(AT_EMPTY_PATH)`, kernel modules, `TIOCSTI` refused by seccomp in every mode |
| **Read, `--read-policy strict`** | your mounts plus `/usr`, `/System`, `/bin`, `/sbin`, `/etc`, `/tmp`, `/dev` | your mounts plus `/usr`, `/lib`, `/bin`, `/sbin`, `/tmp`, `/dev`, and under `/etc` only the files a command needs to start (loader cache, resolver, trust store, `passwd`, `localtime`…) — never `shadow`, `sudoers` or the host keys, and not the listing |
| **Read-only mount** | `-v ./data:ro` → read yes, write no | same |
| **Network by port** | `--allow-net '*:443'` | same, needs Landlock ABI 4 |
| **Network by host** | `--proxy-allow` only, never `--allow-net` | same |
| **Proxy mode** | enforced by the profile | enforced by Landlock (the TCP port) plus seccomp (everything else) |

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
- **Linux protects a mount as a whole.** The repository-hooks and trusted-file
  protections inside a writable mount are macOS only until Landlock can express
  a directory minus some of its files.
- **macOS and Linux only**, and not identically. Anywhere else, native
  execution fails closed rather than running unrestricted.
- **Nothing is mounted implicitly.** `porta run` and `porta serve` see no
  directory until you pass `-v`.
