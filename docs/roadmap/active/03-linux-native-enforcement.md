<!-- description: Enforce native command restrictions on Linux, not just macOS -->
# Linux Native Enforcement

**Priority: High**

`porta run <native command>` now enforces on Linux through Landlock, in
`native/landlock.rs`. Writes, TCP ports and — with `--read-policy strict` —
reads are restricted. Proxy mode is not, and refuses rather than pretends.

## What holds now

Landlock is used unprivileged, without namespaces and without an external
runtime. The ruleset is built before the fork; the child applies it with two
syscalls and no allocation. For `porta run`, which replaces the process, porta
restricts itself and then execs — a ruleset survives `execve`.

| Control | macOS (`sandbox-exec`) | Linux (Landlock) |
|---|---|---|
| Write outside `-v` mounts | denied | denied |
| `/tmp` and `/dev` | writable | writable |
| Read outside grants, default | denied for `~/.ssh`, `~/.gnupg` only | not confined |
| Read outside grants, `--read-policy strict` | **refused** (unimplemented) | **denied** |
| `--allow-net` by port | enforced | enforced, needs ABI 4 |
| Proxy mode | enforced | **refused** |

`scripts/integration.py` asserts the Linux half in CI: a write inside a mount
succeeds, a write outside every grant fails and leaves no file, a connect to a
granted port succeeds while the same connect fails when another port is granted,
and both an unexpressible rule and proxy mode refuse to run.

## Reads: strict is an allow-list, and Linux got it first

Landlock is allow-list only, so the macOS deny-list cannot be ported. That
turned out to be the better shape. `--read-policy strict` handles
`READ_FILE | READ_DIR` and grants reads beneath exactly two sets: the mounts the
caller was given, and the platform's own directories — `/usr`, `/lib`,
`/lib64`, `/bin`, `/sbin`, `/etc`, `/proc`, plus the always-writable `/tmp` and
`/dev`. Everything else in the filesystem is closed, including every home
directory.

That is a guarantee rather than a mitigation. The deny-list this item
originally rejected would have left `~/.aws/credentials`, `~/.config/gh`,
`~/.npmrc` and every `.env` readable; the allow-list closes them without
enumerating them. It needs ABI 1 — a lower bar than the ABI 4 the network rules
need — and `scripts/integration.py` asserts on Linux that a credential outside
every grant is unreadable, that the granted mount and a system interpreter
still are, and that an unknown policy name refuses.

A path in the system set that does not exist on a host is skipped rather than
refused: these are the platform's directories, not a caller's grant, and
`/lib64` is absent on arm64 Debian.

The default stays `open`. Flipping it is a version boundary, once the grants a
real agent needs are known.

### macOS is not done

`--read-policy strict` refuses on macOS. `sandbox-exec` can express an
allow-list — `(deny file-read*)` plus `(allow file-read* (subpath ...))` — but a
naive system set aborts every process with SIGABRT on macOS 26.3, including
`/bin/echo`: dyld needs paths this list does not name, and the profile `(trace
...)` facility that would report them is itself denied (exit 71). Determining
the set empirically is the remaining work. Until then macOS refuses the policy
rather than running with reads open, which is what the write and network paths
already do when a rule cannot be expressed.

This leaves the platforms crossed over: macOS denies two named paths by default
and cannot yet do better, Linux confines everything or nothing.

## Why proxy mode is still refused

The invariant is that proxy mode permits only the loopback proxy endpoint,
without UDP or Unix sockets. Landlock's network rules cover TCP bind and connect
only, so it cannot deny UDP or Unix-socket egress. Enforcing the TCP half and
leaving the rest open would be a policy that looks applied and is not, so
`wt_exec_supervised` keeps returning a failure on Linux.

Closing this needs a mechanism beyond Landlock — a network namespace, or seccomp
for the socket families — and it is the same work
`active/01-http-proxy-filtering.md` defers to its v2.

## Measured hosts

`scripts/probes/landlock_probe.c` reports the ABI and demonstrates enforcement
on a host. Run it on a target before assuming a rule applies there.

| Host | Kernel | Result |
|---|---|---|
| Docker Desktop VM, aarch64, Docker's default seccomp applied | 6.12.76-linuxkit | ABI 6, enforcing |
| GitHub `ubuntu-latest`, Ubuntu 24.04.5, x86_64 | 6.17.0-1022-azure | enforcing |

Two kernels are not the matrix. Older distribution kernels report lower ABI
levels, so porta reads the ABI at runtime and refuses rules the kernel cannot
express.

## Remaining

- `--read-policy strict` on macOS: determine the dyld read set empirically.
- Flip the default to `strict` at a version boundary.
- Proxy mode on Linux, together with `01-http-proxy-filtering` v2.
- A host with a Landlock ABI below 4, to confirm the refusal path on a real
  kernel rather than only by construction.
