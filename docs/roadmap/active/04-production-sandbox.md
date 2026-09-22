<!-- description: What porta must do to be the sandbox people trust in production -->
# The Production Sandbox

This is the plan for porta to become the sandbox a team reaches for when an
agent's mistakes have to stay inside a boundary, and to be recognised as the
best one. It is built on a source-level study of nineteen competing tools
(checkouts under `.references/`, findings under `.references/notes/`) and on
measurements taken against porta itself on 2026-09-21. Every claim about porta
below was executed, not read.

## What the field looks like

Nineteen tools were read. Four compete on porta's exact ground — kernel
primitives, no container, macOS and Linux, wrapping an existing CLI agent:
nono (4.2k stars), Anthropic's sandbox-runtime (5.3k), Fence (1k) and its fork
Greywall. Codex CLI ships a comparable sandbox inside the agent. The rest are
Linux-only Landlock tools (sandlock, island, landrun), macOS-only Seatbelt
wrappers (agent-safehouse, sandvault), microVM or container platforms
(OpenShell, microsandbox, cleanroom, container-use, matchlock, shai), and one
WASM-tool MCP server (wassette).

Three things are true across all of them.

**Nobody fails closed consistently.** nono falls back from Landlock to seccomp
with a warning, ships network and environment open, and has an
`insecure_proxy` switch. sandbox-runtime runs with sockets unrestricted when
its seccomp helper is missing. Fence and Greywall silently drop the network
namespace inside containers. OpenShell's Landlock default is `best_effort`,
which runs with no filesystem restriction and files a finding. Codex warns and
then panics. porta's rule — a restriction the platform cannot express refuses
the run — is not shared by anyone, and is worth more than any single feature.

**Everyone with a Linux story except the Landlock-only tools depends on
bubblewrap**, and most on socat, ripgrep, a Node runtime, a gateway daemon or
Docker. porta is one static binary. sandbox-runtime's install page is a list of
four packages across two package managers.

**Nobody has durable execution.** No journal, no intent-before-effect, no
resume of a decision loop, no operator-owned completion check. wassette, the
closest WASM peer, lets the model grant itself permissions, has no fuel or
timeout, and reloads native code from an unverified cache.

And two things are true about porta that the study made unavoidable.

**Others have solved problems porta has not noticed.** Measured today on
macOS: a sandboxed process can read every other process's argv through
`sysctl kern.procargs2` (credentials passed as flags leak), the login Keychain
is reachable, the ssh-agent socket is reachable unless `--allow-net` happens to
be set, `open(1)` would launch a handler outside the sandbox, `.git/hooks` inside
a mount is writable, the mount root itself can be renamed away, and the child
inherits the whole host environment. On Linux: `handled_access_fs` omits
`REFER`, so a hard link across directories inside a mount fails with `EXDEV` and
`mv` silently degrades to copy-and-delete; `BIND_TCP` is unhandled; the ABI 6
signal and abstract-socket scoping is unused; `/etc` is granted whole; the
seccomp filter exists only in proxy mode and knows two rules. Codex, nono,
agent-safehouse and sandlock each close most of this list, and they did it
years of engineering ago.

**Others have solved the operator's problems.** A denial in porta is a bare
`Operation not permitted` from the tool. nono prints a footer naming the flag
that would have allowed it and offers to save it; Codex classifies the denial
and hands the model a structured escalation contract; sandlock ships a `check`
that prints what this kernel can and cannot enforce. Credentials in porta are
environment variables the child can read; nono, sandbox-runtime, Greywall,
OpenShell and microsandbox all hand the child a placeholder and substitute the
real value in the proxy, bound to a destination.

## The claim porta should make

> porta is the sandbox that never lies about what it enforces. One binary,
> kernel-enforced on macOS and Linux, and when the kernel cannot express your
> policy it refuses to run rather than run with less. Every release is
> installed and attacked before it is called latest, and the attack corpus is
> public — including the runs where porta lost.

Everything in this plan either strengthens that claim or removes something that
contradicts it. Features that neither (Windows, a dashboard, a microVM backend,
TLS interception of all traffic, command-string deny rules, a learning mode
built on strace) are explicitly not in scope, and the reasons are recorded at
the end.

"Production ready" is given a checkable meaning rather than a feeling:

1. A written threat model, and one invariant per claim, each with a test that
   fails when the claim stops being true.
2. No silent degradation anywhere. Every fallback is a refusal with a reason.
3. An escape corpus that runs in CI on every platform and kernel ABI the
   release claims, and is published.
4. Stable interfaces: command line, `porta.toml` schema, exit codes, a
   machine-readable output mode, and a written deprecation policy.
5. Releases that are reproducible, signed, and installed by the pipeline
   before they are promoted.
6. A disclosure process that works (SECURITY.md exists; private reporting is
   enabled) and a support matrix that says exactly which kernels and macOS
   versions are covered.

## Status

**v0.6.0 released 2026-09-21** — the closures below, the denial footer,
`check`/`explain`, exit codes, the escape corpus (published, green on both
platforms) and the threat model. It shipped through the self-verifying
release pipeline: built and tested on each platform, published as a
prerelease, installed and attacked from outside the repository, and only
then promoted. Three real bugs surfaced on CI's macOS runner and were fixed
before merge — the credential-socket deny had missed the runner's `/var/run`
agent path, and `--allow-unix` had not reopened a socket reached through a
symlink.

- **0.6 shipped** (2026-09-21): everything below under Phase 0.6 except two
  items withdrawn after measurement. `TMPDIR` was granted and passed, and the
  suite caught it opening other tools' scratch state to a strict run; it is
  neither. `/proc/self` was granted by the exec'd command and measured to cover
  the shell and none of the tools it starts; `/proc` stays closed. The UDP
  decision under `--allow-net` on Linux went the other way from the draft
  below: UDP stays open (a TCP port rule says nothing about UDP, and closing it
  would break name resolution), recorded in SECURITY.md; proxy mode closes it.
- **0.7 in progress**: the macOS denial footer (per-run tag on every deny rule,
  read back from the unified log after a failed run), `porta check`, `porta
  explain`, exit codes 125/126/127 with the child's code passed through in
  every mode, and `run` now supervising rather than exec-ing in place so porta
  is there to report. Supervising also bought `--timeout <secs>`: the command
  leads its own process group and porta kills the group at the deadline (exit
  124), so a hung or looping agent is bounded — the one wall-clock limit the
  native sandbox now has, closing the gap the threat model called out.
  `--max-cpu`, `--max-procs` and `--max-file-size` followed (2026-09-22):
  rlimits set between fork and exec on both platforms, so a CPU burn, a fork
  bomb and a disk fill are each stopped by the kernel, proven by three corpus
  rows. A memory ceiling is the remaining resource gap: an rlimit caps
  address space, not residency, and a real one needs cgroup v2 in a delegated
  subtree, Linux-only and environment-gated — fail-closed when absent, never
  silent. The WASM side moved to wasmtime 47 and runs WASI 0.2 and 0.3
  components beside core modules (2026-09-22): a component's imports are
  interfaces, each mapped to the capability it needs and refused when
  unknown, and a module the engine cannot read is refused rather than run
  unchecked. `explain
  --json` and `check --json` also landed: the effective policy (or a
  `{"refused":...}`) and the host's primitives as machine-readable objects, so
  a CI step can gate on the policy without parsing prose. `explain --save
  porta.toml` writes the flags in use as a committable config (secrets and `-e`
  values left out on purpose) that `porta up` reads back, so a converged
  invocation becomes the project's policy. Still to do: the Linux footer (needs
  ABI 7 audit records or a `SIGSYS`/exit classifier), the `--ldd`-style
  interpreter hint, and `strict` as the default.
- **0.8 started** with the parts that need no TLS termination: a per-run proxy
  credential (a CONNECT without it is 407, so the loopback proxy is not an
  open relay for other processes; `NODE_USE_ENV_PROXY=1` is set so Node's
  fetch honours the variable), the resolved-address guard (loopback,
  link-local, metadata, multicast are never reached by name; private ranges
  stay open), and the audit record synced to disk before a tunnel opens.
  Placeholder credentials and selective TLS termination remain.

## Phase 0.6 — Close what was measured

Everything here is a gap found by running porta today. None of it is a new
capability; all of it is porta's current claim being made true. It ships as
0.6.0 because two items change defaults.

### macOS profile

- **Cross-process argv and environment.** `(deny sysctl-read (sysctl-name-regex
  #"procargs"))`, `(deny process-info-pidinfo)`, then re-allow both with
  `(target same-sandbox)`. Source: agent-safehouse `10-system-runtime.sb:104-115`.
  Test: a decoy process holding a token in argv; the sandboxed reader gets
  `EPERM`.
- **Mount roots cannot be renamed or unlinked.** `(deny file-write-unlink
  (require-all (literal ROOT) (vnode-type DIRECTORY)))` for every writable
  mount, and unlink denies on every ancestor of a protected path. Source: Codex
  `seatbelt.rs:539-545, 1072-1081`. Test: `mv mount mount.moved` fails.
- **Protected paths inside writable mounts.** `.git/hooks`, `.git/config`, the
  `gitdir:` target of a `.git` pointer file, shell rc files, `.mcp.json`,
  `.vscode`, `.idea`, `.claude/`, and porta's own `porta.toml` and journals —
  excluded from the write grant with `require-not`, including when they do not
  exist yet. Sources: Codex `permissions.rs:36-45, 2232-2269`; sandbox-runtime
  `sandbox-utils.ts:11-40`. Test: writing a pre-commit hook inside a mount
  fails; creating `.git` inside a mount that lacks one fails.
- **Environment is not inherited.** The child gets `PATH`, `HOME`, `TERM`,
  `LANG`/`LC_*`, `TMPDIR`, and whatever `-e` names. Nothing else. `--env-pass
  NAME` opts a variable in. This is a default change and the reason for the
  minor bump. Source: agent-safehouse `environment.sh:10-49`; sandvault `env -i`.
  Test: a host variable set before `porta run` is unset inside.
- **Default denies for credential stores and agents.** Read-data denies for
  `~/.aws`, `~/.config/gcloud`, `~/.npmrc`, `~/.netrc`, `~/.docker/config.json`,
  browser cookie and login databases, plus the existing `~/.ssh`, `~/.gnupg`.
  Keychain closed by both file deny (`~/Library/Keychains`, `/Library/Keychains`
  minus `System.keychain`) and mach denies (`com.apple.SecurityServer`,
  `securityd.xpc`, `secd`, `security.agent`) — nono showed the file deny alone is
  bypassable (`macos.rs:617-635`). Unix-socket egress denied by default for
  launchd `Listeners`, `~/.ssh/agent*`, docker/podman/colima/orbstack sockets
  and gpg-agent, reopened only by `--allow-unix <path>`. Source:
  agent-safehouse `ssh-agent-default-deny.sb`, `container-runtime-default-deny.sb`.
- **Launch Services closed.** `(deny lsopen)` and mach denies for
  `com.apple.lsd.mapdb`, `lsd.modifydb`, `quarantine-resolver`, so `open(1)`
  cannot start an unsandboxed handler. Source: agent-safehouse
  `launch-services.sb:18-43`.
- **Cheap allow-default hardening from sandvault** (`sv:1644-1694`):
  `file-mount`/`file-unmount`, `diskarbitrationd`, `NetAuthAgent`,
  `appleeventsd` denied; `/dev/r?disk*`, `/dev/bpf*`, `/Volumes` closed.
- **Codex's two closures**: `(deny system-fcntl (fcntl-command 80 110))` and
  `(deny mach-lookup (xpc-service-name-prefix ""))`.
- `.ssh`/`.gnupg` denies extended from `file-read-data` to `file-read*` so
  listing does not leak key names.

### Linux ruleset

- **REFER.** Add `LANDLOCK_ACCESS_FS_REFER` (1<<13) to the handled set and to
  every writable grant on ABI ≥ 2. Measured today: without it a cross-directory
  hard link fails `EXDEV` and `mv` degrades to copy. Test: `ln a/x b/x` inside
  a mount succeeds; the same across the mount boundary fails.
- **`/proc/self` instead of nothing.** Rules are applied in `pre_exec`, in the
  child, so `getpid()` is the child's pid and `/proc/<pid>` can be granted as a
  single directory. This is island's trick (`island/src/main.rs:341-344`),
  reachable because porta already restricts in the child. Strict runs keep
  other processes' `/proc` closed and gain `/proc/self`, which is what ps-free
  tools actually read.
- **`/etc` as a file allowlist, not a directory.** Under `strict`, grant
  `ld.so.cache`, `ld.so.conf(.d)`, `localtime`, `resolv.conf`, `hosts`,
  `nsswitch.conf`, `passwd`, `group`, `services`, `protocols`, `ssl/`, `pki/`,
  `alternatives/`, `profile.d/` as file rules (`ACCESS_FILE` rights). `shadow`,
  `gshadow`, `ssh/` and `sudoers` are never in the list. A command that needs
  another `/etc` file gets the existing "grant it with `-v`" refusal. This
  removes the reason porta refuses root, so the root refusal stays but its
  message shrinks.
- **ABI 6 scoping**: `LANDLOCK_SCOPE_SIGNAL | LANDLOCK_SCOPE_ABSTRACT_UNIX_SOCKET`
  when the kernel has ABI 6. The guest cannot signal host processes or reach the
  same user's abstract sockets. Fail-closed rule applies: not a refusal on older
  kernels (this is defence in depth, not a requested rule), but `porta check`
  reports it.
- **`BIND_TCP` handled**, default deny, `--allow-bind <port>` opens one.
- **Named `AF_UNIX` connects.** On ABI ≥ 9 use `RESOLVE_UNIX` per path
  (`landrun/sandbox.go:138-146`). Below that, the baseline seccomp (next item)
  closes `AF_UNIX` `connect()` except to paths under a writable mount, checked in
  the supervisor with `pidfd_getfd` and never `CONTINUE` (sandlock
  `network/connect.rs:121-158`). Until the supervisor exists, `AF_UNIX` is
  denied outright when `--allow-net` or proxy mode is set, which is today's
  proxy-mode behaviour extended.
- **A baseline seccomp filter in every mode**, not only proxy mode. Deny:
  `ptrace`, `process_vm_readv/writev`, `pidfd_getfd`, `memfd_create`,
  `execveat` with `AT_EMPTY_PATH` (fileless exec defeats path policy — OpenShell
  `seccomp.rs:220-266`), `userfaultfd`, `keyctl`/`add_key`/`request_key`,
  `bpf`, `perf_event_open`, `mount` family, `pivot_root`, `unshare`/`setns`,
  `clone*` with `CLONE_NEW*`, `open_by_handle_at`, `io_uring_*` (all three),
  `personality`, `kexec*`, `*_module`, `ioctl(TIOCSTI|TIOCLINUX)`. Socket
  families: `AF_PACKET`, `AF_VSOCK`, `AF_BLUETOOTH`, netlink beyond
  `NETLINK_ROUTE`; `SOCK_RAW` always. Sources: sandlock `sys/structs.rs:306-342`,
  `seccomp_plan.rs:513-643`; OpenShell; Fence. Architecture check kills, as
  today. `PR_SET_NO_NEW_PRIVS` is already set.
- **UDP under `--allow-net`.** Landlock does not see UDP, so today
  `--allow-net '*:443'` leaves UDP open on Linux. Close `SOCK_DGRAM` for
  `AF_INET/6` in the baseline filter when `--allow-net` is set, and resolve
  hostnames named in `--allow-net` once at start into a synthetic `/etc/hosts`
  bind-mounted... no — porta has no mounts. Instead: resolve at start and pass
  `HOSTALIASES`/an `LD_PRELOAD`-free option is not available either. Decision:
  when `--allow-net` names hosts, porta resolves them before confinement and
  writes a `hosts` file into `$TMPDIR`, exporting `HOSTALIASES` for glibc
  resolvers; tools that ignore it get the existing UDP-53 path only if the
  caller passes `--allow-net '*:53/udp'`. This is the one place a documented
  gap remains; it is written into SECURITY.md until the supervisor lands.
- **MPTCP.** landrun documents that Landlock's TCP rules do not cover
  `IPPROTO_MPTCP` and Go ≥ 1.24 defaults to it. The baseline filter denies
  `socket()` with protocol `IPPROTO_MPTCP` (262). Test: a connect with proto 262
  under `--allow-net` fails with `EAFNOSUPPORT`.

### Both platforms

- **DNS story documented per mode**, since three competitors get it wrong in
  their own docs: `--allow-net` leaves the resolver reachable; proxy mode does
  not and the proxy resolves.
- **`--read-policy strict` becomes the default in 0.7**, once the 0.6 denial
  reporting (next phase) exists to tell users which `-v` they are missing. Not
  before: turning it on today would produce bare `Permission denied` for every
  `/opt` toolchain.

## Phase 0.7 — Explain every denial

This is the phase that decides whether people keep porta installed. Today a
denial looks identical to a broken tool. Each item below names the grant that
was missing, in the terms the user typed.

- **Denial harvest, macOS.** After the child exits, read the unified log
  (`log show --predicate 'senderImagePath CONTAINS "Sandbox"' --start <t0>`)
  filtered to the child's pid tree, classify each `deny(<op>) <path>`, and
  print a footer: the operation, the path, and the flag that would have
  allowed it (`-v /opt/homebrew:ro`, `--allow-unix ~/.ssh/agent`, `--allow-net
  '*:443'`). Sources: nono `sandbox_log.rs`, Codex `debug_sandbox/seatbelt.rs:87-114`.
- **Denial harvest, Linux.** ABI 7 `LANDLOCK_RESTRICT_SELF_LOG_NEW_EXEC_ON`
  when available (island `main.rs:295-315`), read back from the audit log;
  seccomp denials via `SECCOMP_RET_LOG`-style tagging on the deny branch;
  `SIGSYS`/`EPERM` exit classification as Codex's `denial.rs` does, but as a
  fallback rather than the primary signal.
- **Structured output.** `--json` on `run` emits one record per denial and one
  summary: `{op, path|host, rule, suggested_flag, code}` with stable codes
  (nono `diagnostic/codes.rs`). The same record goes to the proxy audit JSONL
  so file and network denials share one log.
- **`porta check`.** Prints what this host can enforce: Landlock ABI and each
  protection's status (as sandlock's `check` table), seccomp availability,
  macOS version and `sandbox-exec` presence, and what porta will therefore
  refuse. This is the fail-closed rule made visible before the first run.
- **`porta explain`.** Renders the policy for a command line without running
  it: the profile or ruleset, the mounts as the kernel will see them (after
  canonicalisation), the environment the child gets, the network rule. vibebox
  `explain` is the only competitor with a real dry-run; agent-safehouse's
  `--explain` is close.
- **Save prompt.** After a run with denials, in a TTY: "grant these for next
  time? [y/N/edit]" writing to `porta.toml`. nono's post-run save prompt
  (`profile_save_runtime.rs`) is the model. This replaces the learning modes
  competitors build on strace/eslogger: porta derives grants from real denials
  under the real policy, which is more honest than a trace taken with the
  policy off.
- **Stable exit codes**: 0 child ok; child's code passed through in every mode
  (proxy mode currently collapses to 1); 125 porta refused before exec (policy
  inexpressible, root, missing mount, unknown option); 126 command found but
  not executable under policy; 127 command not found. Documented in `porta
  help exit-codes`.
- **`-v` grants for interpreters found automatically.** When strict refuses an
  interpreter, porta already names its directory. Add landrun's `--ldd` idea
  (`elfdeps.go`): read the ELF dependencies and name every directory needed in
  one message.

## Phase 0.8 — Credentials never enter the sandbox

Five competitors do this and porta does not. It is the largest missing
capability and the one enterprise users ask about first.

- **Placeholders.** `--secret KEY=value` today puts `value` in the child's
  environment. It becomes: the child sees `KEY=porta:secret:v1:<random>`; the
  real value lives in the porta process only (`Zeroizing`), and the proxy
  substitutes it in the `Authorization`/`x-api-key` header **only** for
  requests whose destination matches the credential's binding. Sources: nono
  `credential-injection.mdx`, OpenShell `secrets.rs:831-844`, microsandbox
  `engine/secrets/handler.rs`.
- **Binding is host:port and optional path prefix**, declared in
  `porta.toml`: `[secrets.ANTHROPIC_API_KEY] from-env = true, to =
  "api.anthropic.com:443"`. A placeholder seen in a request to any other host
  is a refusal (403 with a stable code) and an audit record — never forwarded,
  never logged with the value.
- **TLS termination only for bound hosts.** Substituting a header requires
  seeing it. porta terminates TLS for hosts that carry a credential binding or
  an L7 rule, with a per-run ephemeral CA exported through the standard
  variables (`SSL_CERT_FILE`, `NODE_EXTRA_CA_CERTS`, `REQUESTS_CA_BUNDLE`,
  `CURL_CA_BUNDLE`, `GIT_SSL_CAINFO`). Every other host stays CONNECT
  passthrough. This is OpenShell's design minus the platform.
- **Provider profiles as data.** `providers/anthropic.toml`, `openai.toml`,
  `github.toml`: env var names, header, endpoints. A new API is a file, not a
  release.
- **Access presets and method/path rules.** `access = "read-only"` (GET, HEAD,
  OPTIONS) / `"read-write"` / `"full"`, and `allow = [{method, path-glob}]`,
  `deny` wins. Applies to terminated hosts. Codex's `limited` mode and
  OpenShell's presets.
- **Resolved-address guard.** Resolve once, refuse loopback, link-local,
  multicast, cloud metadata (`169.254.169.254`, `fd00:ec2::254`, GCP/Azure
  names), and the host's own interface addresses unless the policy names them
  literally; dial the surviving address. Source: sandbox-runtime
  `resolved-address-guard.ts`. RFC1918 stays allowed unless `--deny-private`.
- **Durable audit.** Each proxy decision is appended and fsynced before the
  connection proceeds. nono admits its in-memory drain loses records on crash;
  porta's audit is already a file, so this is a `sync_data` and a test.
- **Per-run proxy token.** The loopback proxy accepts only connections
  presenting this run's `Proxy-Authorization`, so another local process cannot
  use porta's proxy as an open relay (sandbox-runtime `http-proxy.ts:200-264`).
- **Binary attribution on Linux.** Map an accepted loopback connection to its
  owner via `/proc/net/tcp` inode → `/proc/*/fd` → `/proc/<pid>/exe`, record it
  in the audit line, and allow an optional `binaries = [...]` per rule
  (OpenShell `procfs.rs:165-243`). Attribution only; enforcement stays in the
  kernel.

## Phase 0.9 — Prove it, publicly

- **The escape corpus.** One directory, `scripts/escapes/`, one file per
  attempt, runnable against any binary on `$PATH` that takes `run -v DIR --
  cmd`. Contents: write outside mount; rename mount root; symlink swap during
  run; rename-exchange (`renameat2 RENAME_EXCHANGE`); `.git/hooks` write;
  procargs read; Keychain read; ssh-agent connect; docker.sock connect; `open`;
  `curl`/`wget`/`nc`/`ssh`/`ping`/`/dev/tcp` under each network mode; UDP 53;
  MPTCP; `io_uring`; `memfd_create`+`execveat`; `ptrace` a sibling;
  `process_vm_readv`; `/proc/<other>/cmdline`, `environ`, `maps`; `/etc/shadow`;
  abstract unix socket to a host listener; signal a host process; `unshare`;
  `mount`; setuid binary; `LD_PRELOAD`/`DYLD_INSERT_LIBRARIES`. Each records
  pass/fail/untestable with the reason. Sources for the list: Codex
  `linux-sandbox/tests/suite/landlock.rs`, sandbox-runtime "Security
  Boundaries", agent-safehouse bats, sandlock `test_landlock.rs`.
- **Run it against the competitors too**, weekly, from `.references/`
  refreshed to their latest tags, and publish the table under
  `docs/benchmarks/escapes.md` with the same rules the containment benchmark
  already follows: scenarios are not adjusted until porta wins; a competitor's
  configurable equivalent is stated; untestable is untestable, not implied.
  Nobody else publishes losses. That is the credibility porta is buying.
- **Kernel matrix.** CI runs the corpus on `ubuntu-22.04` (ABI 4),
  `ubuntu-24.04` (ABI 6), `ubuntu-24.04-arm`, and a `-latest` image, plus
  `macos-15` and `macos-26`. sandlock is the only competitor with a deliberate
  low-ABI job; porta's fail-closed claim needs one more than anyone.
- **Threat model document.** `docs/threat-model.md`: what porta protects
  against (an agent's mistakes and prompt-injected instructions, same-user
  process, network exfiltration by socket), what it does not (kernel bugs,
  side channels, a malicious operator, resource exhaustion), and the mapping
  from each SECURITY.md claim to the corpus file that tests it.
- **Fuzzing** of the two parsers that face attacker input: the CLI/`porta.toml`
  reader and the profile/ruleset generator (paths with quotes, control
  characters, `..`, symlinks, 10k mounts — sandbox-runtime hit a
  `SBPL_STRING_MAX_BYTES` cliff and nono a 17,770-rule crash).
- **Reproducible, signed releases.** Build in CI from a tag, publish
  provenance (SLSA attestation via `actions/attest-build-provenance`), sign
  checksums with Sigstore, and have `install.sh` verify the signature, not only
  the hash. This activates the on-hold `supply-chain.md`.
- **Published overhead.** `docs/benchmarks/startup.md`: wall time for `true`,
  `git status`, `python3 -c pass` under each mode versus bare, on each CI
  runner, regenerated per release. Only sandlock and Fence publish numbers;
  porta's should be there whether they flatter or not.
- **External review.** Budget one third-party review of `native/` before 1.0,
  with the report published.

## Phase 1.0 — The agent runtime nobody else has

porta's WASM side is the part no competitor can copy in a release cycle. The
study found no journal, no replay, no completion check anywhere. 1.0 makes
that side as production-grade as the native side, and makes the two one
product.

- **One policy for the agent and its tools.** An agent's `porta.exec` tool call
  runs under the same native policy as `porta run` today. 1.0 adds: the
  agent's declared capabilities in the manifest generate the native policy
  (mounts, hosts, secrets bindings), so the operator writes one document.
- **Budgets are enforced, not advisory.** Fuel/epoch interruption on every
  guest call with the existing `--step-limit`/`--max-memory`, plus a wall-clock
  deadline per tool call and per run. wassette has none; porta should document
  its limits in the same table as its mounts.
- **Signed, pinned distribution.** OCI push/pull for agent modules with the
  manifest as a layer, Sigstore signatures verified at load, and the existing
  hash pins as the fallback. Activates `image-distribution.md`. Unlike wassette,
  the policy layer shipped with an artifact is *never* trusted — the operator's
  `porta.toml` is.
- **Schema from source.** `porta build` already generates the manifest. Derive
  tool `input_schema` from the Almide tool function signatures so schema and
  code cannot drift (wassette's WIT derivation is the precedent).
- **Provenance-gated secrets.** A credential binding can require the agent
  module's hash (or a signer identity) — cleanroom's lineage gating
  (`mediation/config.go:151-217`) applied to porta's pins. A modified agent
  loses its credentials without anyone editing policy.
- **Escalation contract for agents.** When a tool call is refused by policy,
  the refusal returned to the guest is structured (`{code, op, path|host,
  grant}`), and the operator — never the guest — can approve a persisted grant
  from the journal record. Codex's `require_escalated` flow with the approval
  moved to the operator.
- **Journal and replay as the headline.** Document and demonstrate: a run
  killed mid-tool resumes without repeating a write; a completed run replays
  offline and verifies; a completion check the guest cannot talk past. These
  are the features that make "production" mean something for an agent, and
  they are porta's alone.

## Interfaces frozen at 1.0

- Command line: `porta run|up|serve|agent*|check|explain|build|inspect|validate`
  with options before `--`, arguments after. Unknown options are errors
  (done in 0.5.3).
- `porta.toml` schema versioned with `schema = 1`; unknown keys refused.
- Exit codes as in 0.7. `--json` records as in 0.7.
- Policy semantics: a documented list of what each mode denies on each platform,
  and a rule that tightening is a minor version and loosening is never silent.
- Deprecation: one minor version of warning, removal in the next major.

## Not in scope, and why

- **Windows.** sandbox-runtime and Codex each maintain a separate identity/ACL
  model with its own DNS and token-in-argv caveats. It is a second product.
  Revisit after 1.0 with WSL2 + Landlock (nono's path), which fits porta's model.
- **microVM or container backends.** They move the boundary to a hypervisor
  and bring Docker/KVM/HVF requirements. porta's position is the process
  boundary done honestly; users who need a VM have five good options.
- **TLS interception of all traffic.** Terminate only where a credential or L7
  rule requires it. A CA in every process's trust store for every host is a
  larger exposure than the policy it enables.
- **Command-string deny rules** (`git push`, `npm publish`). Fence documents
  that they do not bind descendants and that `sh -c`, `eval` and renamed
  binaries escape them. What binds is the kernel; what does not should not be
  advertised.
- **Learning mode from traces.** Derive grants from denials under the real
  policy (0.7 save prompt) instead of from strace/eslogger with policy off.
- **A dashboard or gateway daemon.** Greywall's proxy is a hard dependency
  with spoofable session identity; OpenShell needs a daemon and SQLite. porta's
  audit is a file; a viewer for it can be anyone's.
- **Hot policy reload.** Unless connections are pinned to a policy generation
  and closed on change (OpenShell `relay.rs`), reload grandfathers what it
  meant to revoke. Not before 1.x, and then only that way.
- **Model self-granting.** wassette's pattern. Never.

## Sequencing and effort

| Phase | Ships | Gate to promote |
|---|---|---|
| 0.6 | macOS and Linux closures above; `--env-pass`; REFER; `/proc/self`; baseline seccomp | corpus items for each closure pass on all CI platforms |
| 0.7 | denial harvest, `--json`, `check`, `explain`, save prompt, exit codes; `strict` default | a first-time user's five mistakes each print the fixing flag |
| 0.8 | placeholders + bound injection, selective TLS, presets, address guard, proxy token, durable audit | a credential never appears in the child's environment or in any audit line; a mis-addressed placeholder is refused |
| 0.9 | escape corpus public with competitor table, kernel matrix, threat model, fuzzing, signed releases, overhead numbers | corpus green on the matrix; competitor table published with at least one porta loss shown honestly |
| 1.0 | unified policy, enforced budgets, signed OCI distribution, schema from source, provenance-gated secrets, escalation contract | third-party review published; interfaces frozen |

Each phase is one to three weeks of the current pace, and every phase ships
independently. The order is deliberate: close known holes before advertising
(0.6), make failures explainable before tightening defaults (0.7), keep
secrets out before inviting teams (0.8), prove it before claiming it (0.9),
then lead with what only porta has (1.0).

## What "world's best" will be measured by

Not stars. Four numbers, all public and regenerated by CI:

1. **Escape corpus**: attempts stopped / attempts applicable, per platform,
   for porta and for each competitor at its latest release.
2. **Silent degradations**: zero. Every case where a competitor runs with less
   than asked is a case where porta refused, and the table says so.
3. **Time from denial to fix**: the median number of commands a new user needs
   to get from a refused run to a working one. Target: one, because the
   refusal printed it.
4. **Install to first enforced run**: seconds, on a clean machine, with the
   dependency count beside it. porta: one binary, zero dependencies. The
   competitors' numbers go in the same row.
