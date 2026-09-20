<!-- description: Enforce native command restrictions on Linux, not just macOS -->
# Linux Native Enforcement

**Priority: High**

`porta run <native command>` enforces nothing on Linux. `exec_sandboxed_linux`
in `native/wasmtime_bridge.rs` is a deliberate fail-closed stub that refuses to
run rather than run unrestricted, which is correct but leaves the feature
absent.

## Why this is now the largest gap

The README leads with restricting an agent the reader already runs:

```bash
porta run claude --allow-net 'api.anthropic.com:443' -v ./project -- --print "..."
```

That is the entry point most people will try first, and it is macOS-only. The
people who most want an agent that cannot write outside one directory or reach
an unlisted host are running CI and servers on Linux. Until this exists, the
claim that the OS stops the agent holds on a laptop and not in production.

## Parity target

A Linux backend has to enforce what the macOS `sandbox-exec` profile already
does, or refuse the specific rule it cannot enforce:

| Control | Current macOS behaviour |
|---|---|
| Filesystem write | denied outside `-v` mounts and `/tmp` |
| Filesystem read | `~/.ssh` and `~/.gnupg` denied |
| Read-only mounts | `-v ./data:ro` permits read, denies write |
| Network | open by default; `--allow-net '*:443'` restricts by port |
| Proxy mode | only the loopback CONNECT proxy; no UDP, no Unix sockets |

## Feasibility probe

Measured on kernel 6.12.76-linuxkit, aarch64, inside Docker Desktop's VM, with
Docker's default seccomp profile already applied. `landlock_create_ruleset` with
`LANDLOCK_CREATE_RULESET_VERSION` reported **ABI 6**. A single process then
handled `WRITE_FILE | MAKE_REG` and `CONNECT_TCP`, allowed one directory and
port 443, and called `prctl(PR_SET_NO_NEW_PRIVS)` plus `landlock_restrict_self`:

| Operation | Before | After |
|---|---|---|
| `connect 127.0.0.1:80` | ECONNREFUSED | **EACCES** |
| `connect 127.0.0.1:443` | ECONNREFUSED | ECONNREFUSED (policy passed) |
| write in the allowed directory | ok | ok |
| write outside it | ok | **EACCES** |

No privileges, no namespaces, no external binary. This is one kernel, not the
matrix: the target hosts, including the GitHub `ubuntu-latest` runner, still
have to be probed, and network rules need a sufficient ABI. Porta must read the
ABI at runtime and refuse rules the kernel cannot express.

### One semantic gap found

Landlock is allow-list only. The macOS profile denies reads of `~/.ssh` and
`~/.gnupg` while leaving every other read permitted, which Landlock cannot
express: either read access is not handled at all, and those paths stay
readable, or it is handled and every path the command legitimately reads has to
be enumerated. This is a decision to make explicitly, not a detail to discover
during implementation.

## Candidate mechanisms

Landlock now looks sufficient for writes and TCP ports. The rest remain open for
what it cannot cover.
- **seccomp** — syscall filtering. No host or port semantics on its own.
- **User and network namespaces with bind mounts** — closest to the mount model,
  but changes the process tree and requires unprivileged user namespaces, which
  some distributions and container hosts disable.
- **nftables or cgroup eBPF** — egress control, usually needs privileges Porta
  should not require.
- **bubblewrap** — would work, at the cost of an external runtime dependency
  that the macOS path does not have.

Running inside a container is a common deployment and restricts what any of
these can do, so the design has to state what holds when Porta itself is
already containerised.

## Must not regress

`Native sandbox execution must fail closed on unsupported platforms` is a
standing invariant. Partial support must reject the rules it cannot enforce
instead of accepting a policy it will not apply. A kernel that supports
filesystem restrictions but not the requested network rule must refuse that run,
not run it with the network open.

## Acceptance

- The integration tests that cover the macOS restrictions run on Linux in CI,
  which already builds on `ubuntu-latest`, including denial cases.
- A run whose policy cannot be fully enforced fails, and the message names the
  rule that could not be applied.
- `docs/` stops carrying the macOS-only caveat for the controls that now hold,
  and keeps it for those that do not.

## Related

`active/01-http-proxy-filtering.md` records the supervised exec path as macOS
only and defers Linux to its v2; the proxy and the sandbox need the same
platform backend, so these should land together rather than twice.
