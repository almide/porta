# Threat model

porta confines a program you run — usually an AI agent and the commands it
spawns — to the files, network and host facilities you granted it, using the
kernel's own enforcement. This document says what that protects against, what
it does not, and how each claim is tested. It is the contract `SECURITY.md`
points at.

## What porta is defending

**The adversary is the confined program itself**, and anything steering it: a
prompt-injected instruction, a compromised dependency it executes, a tool it
was talked into misusing. The program is assumed to be trying to reach outside
its grant — to read a credential it was not given, write where it should not,
or send data somewhere it should not. porta's job is to make the boundary hold
even when the program is actively pushing on it.

**The operator is trusted.** The person who ran `porta` chose the policy, and
porta enforces their intent against the program, not against them. A policy
that grants too much is the operator's decision; porta's contribution is that
what they granted is exactly what the program gets, and that a policy the kernel
cannot enforce refuses the run rather than pretending.

## The boundary, and where it holds

porta applies OS-level enforcement to the process and every child it spawns:

- **macOS**: a `sandbox-exec` (Seatbelt) profile, applied before the program
  starts.
- **Linux**: a Landlock ruleset plus a seccomp-BPF filter, applied in the child
  between `fork` and `exec`, inherited by every descendant.

The restriction cannot be lifted from inside: `PR_SET_NO_NEW_PRIVS` is set, a
Landlock ruleset survives `execve`, and neither Seatbelt nor Landlock offers the
confined process a way to widen its own domain.

Each guarantee below is an invariant in `CLAUDE.md`, and each has a test in
`scripts/integration.py` or `scripts/escapes.py` that fails when it stops being
true.

| Claim | Held by | Tested by |
|---|---|---|
| No write outside the granted mounts (plus `/tmp`, `/dev`) | Seatbelt `deny file-write*` / Landlock write rights | `escapes.py` "write outside every mount" |
| Under `--read-policy strict`, no read outside the mounts and the platform's own directories | Seatbelt `deny file-read*` / Landlock read rights | integration "strict read policy"; `escapes.py` "/etc/shadow under strict" |
| Credential stores under the home are unreadable (macOS, every mode) | profile `deny file-read*` on `~/.ssh`, `~/.aws`, Keychains, … | `escapes.py` "read an SSH private key", "read the login Keychain" |
| Another process's arguments and environment are unreadable (macOS every mode; Linux only under `--read-policy strict`, see [the comparison](benchmarks/competitors.md#where-porta-loses)) | Seatbelt `procargs`/`process-info` denies; Linux `/proc` closed under strict | `escapes.py` "read another process's arguments" |
| A mount's repository hooks and trusted root files cannot be written or renamed away (macOS) | profile per-path denies plus `deny file-write-unlink` on ancestors | integration "repository hooks and mount roots are closed"; `escapes.py` |
| Proxy mode: the loopback proxy is the only egress | Landlock TCP port rule + seccomp socket-family filter; Seatbelt profile | integration "proxy mode is the only egress" |
| The proxy serves only this run's client, and never tunnels to loopback/metadata addresses | per-run credential; resolved-address guard | integration "CONNECT needs the run credential" |
| Syscalls that reach around a file policy are refused (Linux, every mode) | seccomp baseline: `ptrace`, `process_vm_*`, mounts, namespaces, `io_uring`, `execveat(AT_EMPTY_PATH)`, … | `escapes.py` "ptrace", "fileless exec", "io_uring", "new user namespace" |
| The child inherits no host environment beyond `PATH`, `HOME`, locale, terminal | `env_clear` then a named allow-list | integration "the host environment stays outside" |
| A policy the platform cannot express refuses the run | `prepare` returns an error; `run` exits 125 | integration "unexpressible rule refused" |

## What porta does not defend against

These are out of scope by design, not undiscovered. A report that porta does
not stop one of them is accurate and not a vulnerability.

- **Kernel exploits.** porta's boundary is the kernel's. A privilege escalation
  through a kernel bug defeats it — and defeats a container on the same kernel.
  Those belong upstream.
- **Side channels and resource exhaustion.** porta does not close timing or
  cache channels, and it bounds a native command's resources only on request
  and only as far as an unprivileged process can. `--timeout <secs>` kills the
  command and its whole process group at the deadline (exit 124). `--max-cpu`,
  `--max-procs` and `--max-file-size` are rlimits set between fork and exec and
  inherited by every descendant, so a CPU burn ends with SIGXCPU, a fork bomb
  cannot fork, and a file stops growing at the ceiling — each per process, not
  per run. A program that ignores SIGXCPU is killed anyway: by the kernel at
  the hard limit on Linux, and on macOS, which never follows the signal with a
  kill, by porta's supervisor once the process group's CPU reaches the ceiling.
  `--max-memory-mb` bounds resident memory for the whole run: on Linux through
  cgroup v2, placed by the systemd user manager, since an rlimit caps only
  address space, and refused where no user manager runs for the caller, so a
  run never proceeds with a ceiling it asked for and did not get; on macOS by
  the supervisor ending the group once its footprint reaches the ceiling,
  polled, which a burst can pass for up to a quarter second. (The WASM agent runtime bounds fuel, memory and deadlines outright,
  on every platform.)
- **A malicious operator.** porta enforces the operator's policy against the
  program. It does not protect the program, or a third party, from an operator
  who writes a policy that grants everything, or who passes `--allow-root`.
- **What a granted tool does with its grant.** porta checks a tool's arguments
  against its declared schema, not its intent. A tool granted the network can
  use the network; a mount granted for writing can be written.
- **TLS content.** Proxy mode controls which host a connection reaches, not
  what travels once it is established. It is not a data-loss filter, and (in
  0.6/0.8) does not terminate TLS to inspect it.
- **Confidentiality of what the operator left readable.** Without `--read-policy
  strict`, a program reads what the operator's user can read, minus the
  credential stores porta closes. The default policy is a write-and-egress
  boundary, not a secret store.

## Platform differences that matter to the model

- **`/etc` under strict.** Linux grants only the individual files a command
  needs to start (`ld.so.cache`, `resolv.conf`, the trust store, `passwd`,
  `localtime`, …) and never `shadow`, `sudoers` or the host keys. macOS grants
  `/private/etc` whole, because `sandbox-exec` grants a subtree; there, file
  permissions are what separate `shadow`, which is why porta refuses to run as
  root (`--allow-root` overrides, and the operator owns that choice).
- **Mount-internal protections.** The repository-hooks and trusted-file denies
  inside a writable mount are macOS-only: Landlock grants a directory whole and
  cannot carve files out of it. On Linux, a writable mount is writable throughout.
- **UDP under `--allow-net`.** Landlock's rules reach TCP; a named port on Linux
  says nothing about UDP, which stays open under `--allow-net` (and closes name
  resolution if it did not). Proxy mode closes UDP on both platforms via seccomp.
- **The denial footer.** After a failed run porta names the flag each refusal
  needed. On macOS it reads this from the unified log; on Linux the equivalent
  needs Landlock ABI 7 audit records and is not there yet.
- **Signal and abstract-socket scoping.** Closed on Linux only from Landlock ABI
  6; on older kernels a confined process can still signal the operator's other
  processes and reach their abstract sockets. `porta check` reports whether the
  running kernel has it.

## How the claims are kept honest

- Every invariant maps to a row in the table above and a test that runs in CI on
  every platform the release targets.
- The escape corpus (`scripts/escapes.py`) runs the same attacks a stranger
  would, against the published binary, and is published with its results —
  including any row porta loses.
- A rule the kernel cannot express refuses the run. There is no silent
  fallback, on any platform, to running with less than asked.
