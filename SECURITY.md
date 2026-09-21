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

porta's claims are the invariants in `CLAUDE.md`. A report is interesting if it
breaks one of them. The short version:

- A command cannot write outside the mounts it was granted, or `/tmp` and `/dev`.
- Under `--read-policy strict` it cannot read outside those mounts and the
  platform's own directories.
- Under proxy mode the loopback proxy is its only egress — no UDP, no Unix
  sockets, no `io_uring`.
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
- **`/etc` is readable under `strict`.** A command cannot start without
  `ld.so.cache`, `localtime`, `resolv.conf` and the trust store, and Landlock
  grants directories rather than files. File permissions are what separate
  `shadow` and host keys from an ordinary user — which is why porta refuses to
  run as root, where they separate nothing. Narrowing `/etc` is open work.
- **Reads are open by default.** `--read-policy strict` is opt-in in 0.5.x.
- **The default policy is not a secret store.** Without `strict`, an agent can
  read what your user can read, minus `~/.ssh` and `~/.gnupg` on macOS.
- **A child may listen on a port.** `LANDLOCK_ACCESS_NET_BIND_TCP` is unused;
  the egress invariant is about outbound.
- **A tool you granted can do what you granted it.** porta checks arguments
  against a schema, not intent.

## Versions

Fixes go to the latest release. 0.5.x is the current line; there are no
backports to 0.4.x.
