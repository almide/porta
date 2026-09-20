<!-- description: Enforce native command restrictions on Linux, not just macOS -->
# Linux Native Enforcement

**Priority: High**

`porta run <native command>` now enforces on Linux through Landlock, in
`native/landlock.rs`. Writes and TCP ports are restricted; reads and proxy mode
are not, and both refuse rather than pretend.

## What holds now

Landlock is used unprivileged, without namespaces and without an external
runtime. The ruleset is built before the fork; the child applies it with two
syscalls and no allocation. For `porta run`, which replaces the process, porta
restricts itself and then execs — a ruleset survives `execve`.

| Control | macOS (`sandbox-exec`) | Linux (Landlock) |
|---|---|---|
| Write outside `-v` mounts | denied | denied |
| `/tmp` and `/dev` | writable | writable |
| Read `~/.ssh`, `~/.gnupg` | denied | **not confined** |
| `--allow-net` by port | enforced | enforced, needs ABI 4 |
| Proxy mode | enforced | **refused** |

`scripts/integration.py` asserts the Linux half in CI: a write inside a mount
succeeds, a write outside every grant fails and leaves no file, a connect to a
granted port succeeds while the same connect fails when another port is granted,
and both an unexpressible rule and proxy mode refuse to run.

## Why reads are still open

Landlock is allow-list only. The macOS profile denies two paths and permits
every other read, which cannot be expressed: either reads are not handled, and
those paths stay readable, or they are handled and every path the command
legitimately reads has to be enumerated.

Replicating the macOS deny-list is the wrong target. Denying `~/.ssh` and
`~/.gnupg` while `~/.aws/credentials`, `~/.config/gh`, `~/.npmrc` and every
`.env` stay readable is a mitigation, not a guarantee, and it contradicts the
claim that porta runs a command with the permissions it was granted.

The intended end state is deny-by-default reads on **both** platforms:

1. Add `--read-policy` with `open` (today's behaviour) and `strict`
   (allow-list), defaulting to `open`.
2. Implement `strict` on Linux first, since Landlock can only do that shape.
3. Refuse any run whose requested policy the platform cannot express, as the
   write and network paths already do.
4. Flip the default to `strict` at a version boundary, once the grants a real
   agent needs are known.

Until then the documents must say plainly that reads are not confined, which
they now do.

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

- `--read-policy strict` on both platforms.
- Proxy mode on Linux, together with `01-http-proxy-filtering` v2.
- A host with a Landlock ABI below 4, to confirm the refusal path on a real
  kernel rather than only by construction.
