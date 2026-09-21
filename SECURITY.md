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
- **Under `strict`, the `/etc` files a command needs to start are readable.**
  On Linux they are granted one by one — the loader cache, the resolver
  configuration, the trust store, `passwd`, `localtime` — and `shadow`,
  `sudoers` and the host keys are not among them. On macOS `sandbox-exec`
  grants `/private/etc` whole; file permissions separate `shadow` there, which
  is one reason porta refuses to run as root.
- **Reads are open by default.** `--read-policy strict` is opt-in in 0.6.x.
- **The default policy is not a secret store.** Without `strict`, an agent can
  read what your user can read, minus the credential stores porta closes in
  every mode on macOS (`~/.ssh`, `~/.gnupg`, `~/.aws`, `~/.config/gcloud`,
  `~/.docker`, `~/.kube`, `~/.netrc`, `~/.npmrc`, `~/.pypirc`, the Keychains,
  browser profiles). On Linux the default mode closes none of these; `strict`
  closes the whole home.
- **With the network open, a child may listen and may reach Unix sockets.**
  Listening is closed only once `--allow-net` names a port, and reopened per
  port by `--allow-bind`. On macOS the SSH agent, gpg-agent and container
  runtime sockets are closed in every mode; on Linux they are not until
  Landlock's `RESOLVE_UNIX` is used.
- **Inside a writable mount, Linux protects nothing in particular.** The
  macOS profile keeps the operator's repository hooks and config and the
  trusted files at a mount root unwritable; Landlock grants a directory whole.
- **UDP is open under `--allow-net` on Linux.** Landlock's rules reach TCP;
  a named port on Linux says nothing about UDP. Proxy mode closes UDP on both
  platforms.
- **A tool you granted can do what you granted it.** porta checks arguments
  against a schema, not intent.

## Versions

Fixes go to the latest release. 0.6.x is the current line; there are no
backports to 0.5.x.
