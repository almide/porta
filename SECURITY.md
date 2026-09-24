# Reporting a security issue

Use [GitHub's private vulnerability reporting][report] on this repository. That
reaches the maintainer without the report being public first.

[report]: https://github.com/almide/porta/security/advisories/new

Please do not open a public issue for something that lets an agent escape a
restriction porta claims to enforce. Anything else — a crash, a wrong error
message, a confusing default — is fine in the open.

There is one maintainer and no service-level commitment. You will get an
acknowledgement, and if the report is valid, a fix and credit in the release
notes unless you would rather not be named.

## What counts

porta's claims are the invariants in `CLAUDE.md`, and what porta does and does
not defend against is written out in [the threat model](docs/threat-model.md),
with each claim mapped to the test that keeps it honest. A report is interesting
if it breaks one of them. The short version:

- A command cannot write outside the mounts it was granted, or `/tmp` and `/dev`.
- Under `--read-policy strict` it cannot read outside those mounts and the
  platform's own directories.
- Under proxy mode the loopback proxy is its only egress — no UDP, no Unix
  sockets, no `io_uring` — and the proxy serves only a client presenting this
  run's credential, never tunnels to loopback, link-local, metadata or
  multicast addresses, and records every decision before acting on it.
- The child's environment holds nothing of the caller's shell beyond `PATH`,
  `HOME`, the locale and the terminal unless `-e` or `--env-pass` named it.
- What the preset (`native/presets/default.toml`, shown by `porta explain`)
  closes stays closed: credential stores to reads in every mode, the credential
  agents' sockets to `connect`, and inside a writable mount the repository hooks
  and config and the trusted files at the root, which cannot be written or
  renamed away. On Linux the sockets and the mount protections need the
  command's mount namespace; without one the run says what stays open.
- On macOS another process's arguments, the Keychain and `open(1)` are closed.
- Under `--allow-net` no UDP leaves: on Linux an internet socket must be TCP.
- A WASM agent's tool arguments are validated against the declared schema
  before anything executes, and a guest cannot grant itself a capability,
  choose a credential, or bypass a failed completion check.
- Model credentials stay in the host and are never handed to a guest.
- A restriction porta cannot enforce refuses the run rather than narrowing it.

## What does not count, and why

These are known and documented rather than undiscovered. A report that porta is
not a container is accurate and not news.

- **porta is not a VM.** Restrictions apply to a process on your kernel. A
  kernel privilege escalation defeats them, and defeats them for a container
  too. Report those upstream.
- **Under `strict`, the `/etc` files a command needs to start are readable.**
  On Linux they are granted one by one — the loader cache, the resolver
  configuration, the trust store, `passwd`, `localtime` — and `shadow`,
  `sudoers` and the host keys are not among them. On macOS `sandbox-exec`
  grants `/private/etc` whole; file permissions separate `shadow` there, which
  is one reason porta refuses to run as root.
- **Reads are open by default.** `--read-policy strict` is opt-in in 0.6.x.
- **The default policy is not a secret store.** Without `strict`, an agent can
  read what your user can read, minus what the preset closes. A credential
  store the preset does not name is readable; add it with `--deny-read`.
- **With the network open, a child may listen and may reach Unix sockets.**
  Listening is closed only once `--allow-net` names a port, and reopened per
  port by `--allow-bind`. The credential sockets the preset names are closed;
  on Linux only those bound when the run starts, and only where the host
  allows unprivileged user namespaces.
- **Inside a writable mount on a Linux host without user namespaces, nothing
  in particular is protected.** Landlock grants a directory whole; the run
  says so.
- **Name resolution under `--allow-net` on Linux goes over TCP.** A resolver
  that ignores `RES_OPTIONS=use-vc` cannot resolve there; proxy mode can.
- **A tool you granted can do what you granted it.** porta checks arguments
  against a schema, not intent.

## Versions

Fixes go to the latest release. 0.6.x is the current line; there are no
backports to 0.5.x.
