<!-- description: --no-net: no network at all, backed by a network namespace on Linux -->
<!-- done: 2026-09-23 -->
# No network at all

## Why

Until now porta's network was open by default, the way a Docker container's
is, and could be narrowed to TCP ports (`--allow-net`) or to hosts through the
proxy (`--proxy-allow`). "Nothing at all" could not be said. The nearest was
a single unused port under `--allow-net`, and on Linux that still left UDP
open (see the threat model). srt and Fence start with no network. The
[comparison](../../benchmarks/competitors.md) listed the network namespace as
the one isolation layer porta lacked.

## What

`--no-net` (`no-net = true` in `porta.toml`) closes the network entirely.
Combining it with `--allow-net`, `--allow-bind` or a proxy is refused.

- **macOS**: the profile denies every outbound, bind and inbound network
  operation, the resolver socket included. Unix sockets named with
  `--allow-unix` are reopened as in every mode.
- **Linux, where unprivileged user namespaces are allowed**: `pid_namespace`
  adds `CLONE_NEWNET` to the namespaces it already creates and brings up the
  new namespace's loopback interface. The host's interfaces, its loopback
  services and its abstract Unix sockets are absent. The command can still
  serve and dial on its own 127.0.0.1, so a test suite that starts a local
  server works. The availability probe now asks for the network namespace
  too, so one answer covers both.
- **Linux, where they are not**: Landlock handles TCP with no port granted,
  and seccomp's proxy-only filter refuses every family but IPv4 TCP. Nothing
  leaves, the command's own loopback included. A kernel with neither a
  namespace nor Landlock ABI 4 refuses the run. With the namespace, `--no-net`
  therefore also works on kernels older than Landlock's network rules
  (before 6.7), where porta refuses `--allow-net`.

## Verified

2026-09-23:
- macOS arm64: integration suite, corpus 19/19.
- Linux aarch64 container, with and without user namespaces: integration
  suite 24/24 and 23/23, corpus 23/23 and 22/22.
- By hand, with the namespace: a service on the host's 127.0.0.1 got
  "Connection refused", a UDP DNS question got "Network is unreachable",
  a listener on the command's own loopback accepted, and `/proc/net/dev` had
  no `eth0`.
- The new corpus row "reach anything under --no-net" tries a service on the
  host's loopback, TCP to 1.1.1.1:443 and a UDP DNS question. Run outside any
  sandbox, the probe reaches all three. porta, srt, Fence and nono hold it;
  landrun lets the UDP question through.

## Not done

The network stays open by default. Changing that would break every existing
invocation that relies on it, and deserves its own decision. `--allow-net`
and proxy mode stay in the host's network namespace. A namespace there would
need porta to carry the traffic out (a proxy listener inside the namespace,
passed back to porta), and under `--allow-net` a user-space network stack.
