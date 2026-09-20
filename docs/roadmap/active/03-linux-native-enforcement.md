<!-- description: Enforce native command restrictions on Linux, not just macOS -->
# Linux Native Enforcement

**Priority: High**

`porta run <native command>` now enforces on Linux through Landlock, in
`native/landlock.rs`. Writes, TCP ports and — with `--read-policy strict` —
reads are restricted, and proxy mode is enforced with a seccomp filter beside
the ruleset.

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
| Read outside grants, `--read-policy strict` | denied | denied |
| `--allow-net` by port | enforced | enforced, needs ABI 4 |
| Proxy mode | enforced | enforced (Landlock + seccomp) |

`scripts/integration.py` asserts this in CI on both platforms: a write inside a
mount succeeds while a write outside every grant fails and leaves no file, a
credential outside every grant is unreadable under `strict` and readable
without it, and an unknown policy name refuses. On Linux it also asserts that a
connect to a granted port succeeds while the same connect fails when another
port is granted, that an unexpressible rule refuses, and that under proxy mode
a TCP socket opens while UDP, Unix, IPv6 and netlink sockets do not — and that
without proxy mode none of them are filtered.

Each platform's policy is built in its own module — `native/sandbox_profile.rs`
for the macOS profile text, `native/landlock_policy.rs` for the Landlock
ruleset — and every entry point on a platform goes through the one function, so
`run`, `up` and MCP execution cannot drift apart.

A rule names the path the kernel resolved, not a symlink to it: an absolute
mount is canonicalized before any rule is written, because `/var/folders/…` is
really `/private/var/folders/…` and a rule for the former matches nothing.

## Reads: strict is an allow-list, and Linux got it first

Landlock is allow-list only, so the macOS deny-list cannot be ported. That
turned out to be the better shape. `--read-policy strict` handles
`READ_FILE | READ_DIR` and grants reads beneath exactly two sets: the mounts the
caller was given, and the platform's own directories — `/usr`, `/lib`,
`/lib64`, `/bin`, `/sbin`, `/etc`, plus the always-writable `/tmp` and `/dev`.
Everything else in the filesystem is closed, including every home directory.

`/proc` is not in that set, and its absence is a security decision rather than
a loader one. It was there on the assumption an interpreter needs it. Measured,
none of `sh`, `python3`, `curl`, `perl` or `grep` does — and granting it hands
a confined command the command line of every other process the same user is
running, credentials included, plus the host's connection table and mount
layout. Landlock cannot narrow it to the process's own entry: the ruleset is
built before the fork, so the child's PID does not exist yet. A command that
genuinely needs `/proc` takes it as a mount the caller grants.

This was the one place where the two platforms' read sets were derived
differently. macOS's came from the kernel's own denial records, because nothing
else could produce it. Linux's was written from an assumption and never
checked. The assumption was wrong, and `scripts/integration.py` now asserts it:
a strict run cannot read another process's command line, with a decoy carrying
a credential to make the failure visible.

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

The command itself is subject to the policy: one that lives outside every grant
and outside the platform set cannot be read, and so cannot be started. A
toolchain installed under `/opt` — which is what GitHub's hosted Python is —
needs `-v` on its own directory. porta says that instead of letting the kernel
answer with a bare `Permission denied` after the policy is already applied.

The default stays `open`. Flipping it is a version boundary, once the grants a
real agent needs are known.

### macOS, and how its read set was found

`sandbox-exec` expresses the same allow-list — `(deny file-read*)` plus `(allow
file-read* (subpath ...))` — but a guessed system set aborts every process on
macOS 26.3, including `/bin/echo`, and the profile's `(trace ...)` facility that
would report what was missing is itself denied (exit 71).

The kernel says it anyway. Every denial is recorded in the unified log:

```
kernel (Sandbox) Sandbox: echo(35707) deny(1) file-read-data /
```

Running a target under a candidate profile, reading the denials back out of
`log show --predicate 'senderImagePath CONTAINS "Sandbox"'` and adding exactly
what was named, repeated to a fixed point, produces the set. The first blocker
is `/` itself: the loader reads the root directory entry before anything else,
which is why a subtree list alone aborts. Dropping each entry in turn against a
sample of commands then showed which are load-bearing.

`scripts/probes/macos_read_set.py` is that loop, and it reads the set out of
`native/sandbox_profile.rs` rather than repeating it, so it cannot drift from
what porta applies. `verify` runs the sample under the shipped set, `discover`
adds what the kernel names until everything runs, `ablate` drops each entry in
turn. Apple moves these paths between releases; re-run `verify` on a new one.

The result is `PROFILE_READABLE` and `PROFILE_READABLE_LITERALS` in
`native/sandbox_profile.rs`: `/usr`, `/System`, `/bin`, `/sbin`, `/private/etc`
and `/private/var/select` as subtrees, and `/`, `/tmp`, `/etc`, `/var` as
themselves — the last three are symlinks, so granting the link exposes nothing
that the subtree rules have not already decided. `/private/var/select` holds the
one symlink `/bin/sh` reads to choose which shell to behave as; without it every
`sh` run starts with a denial on stderr.

Twenty-six commands — `sh`, `cat`, `grep`, `curl`, `openssl`, `tar`, `perl`,
`sqlite3`, `find`, `ssh`, `plutil` among them — run clean under it with a home
directory unreadable. What is *not* in it is as deliberate: `ruby` fails,
because RubyGems reads `/Library/Ruby/Gems`. That is a language runtime's
package directory, and it is a mount the caller grants, not a hole this list
leaves open.

Both platforms now confine reads. The remaining asymmetry is the default: macOS
denies two named paths, Linux denies nothing, and neither confines until asked.

## Proxy mode: what Landlock could not reach

The invariant is that proxy mode permits only the loopback proxy endpoint,
without UDP or Unix sockets. Landlock's network rules cover TCP bind and
connect and nothing else, so for a long time this refused rather than enforce
the TCP half and leave the rest open.

A seccomp filter closes the rest, in `native/seccomp.rs`. It watches the one
syscall that opens an egress channel: `socket(2)` returns `EAFNOSUPPORT` unless
it asks for `AF_INET` with `SOCK_STREAM`, which is the errno a client already
knows how to fall back from. Landlock still decides *which* TCP port may be
reached, so the two together are the invariant rather than either alone.

Two details are the difference between a filter and a claim:

- **`io_uring` is refused outright.** A ring can open a socket without ever
  issuing `socket(2)`, so a filter that watched only that syscall would assert
  an egress policy the kernel does not hold. `ENOSYS` is the answer, which a
  library reads as an older kernel and falls back from.
- **A mismatched `arch` kills the process.** Syscall numbers mean different
  things in different tables, so a filter that returned an errno there would be
  guessing. This is the one case where an errno would be a lie.

The two platforms hold the same invariant at different points, which is worth
knowing before reading either one's test. macOS refuses at `connect` and `send`
— a UDP socket is created happily and then cannot reach anything — while Linux
refuses at `socket(2)`, so the socket never exists. `scripts/integration.py`
asserts the outcome on both and the mechanism on neither: on macOS that a UDP
send, a Unix-socket connect and a direct TCP connect all fail with `EPERM`, and
on Linux that those sockets do not open at all.

`socketpair(2)` is deliberately left alone: it makes an anonymous pair both of
whose ends the process already holds, so it reaches nothing, and runtimes use
it for their own plumbing.

The filter is checked for acceptance before the run commits to it. seccomp can
be compiled out or refused by a container's own policy, and a proxy-mode run
whose filter never loaded would be a policy that looks applied and is not, so
that refuses like an unexpressible Landlock rule does.

What this does **not** close: Landlock's `ACCESS_NET_BIND_TCP` is not used, so
the child may still listen on a port. That is ingress, and the invariant is
about egress, but it is a difference from a container's network namespace and
not a thing this filter decides.

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

The macOS read set is version-dependent in the same way.
`scripts/probes/macos_read_set.py verify` was clean on macOS 26.3, arm64: 18
commands, no failure and no denial, with `~/.ssh` unreadable.

## Remaining

- Flip the default to `strict` at a version boundary.
- A host with a Landlock ABI below 4, to confirm the refusal path on a real
  kernel rather than only by construction.
