# Porta

Porta is an Almide 0.63.0 WASM agent and team runtime, with MCP interoperability.
Almide owns CLI, manifests, capability checks and MCP. `examples/chat-agent` is
the actual reasoning/control loop compiled to WASM.

A module is read through the engine that will run it — `wasm_rt.wt_inspect`,
which is wasmtime. porta has no WASM parser of its own; `src/wasm_imports.almd`
holds only the import shape the capability check matches against.

`native/` is the host side. The Almide `@extern(rs, "<module>", …)` declarations
resolve against `wasmtime_bridge` and `agent_runtime`, so those two files are
the FFI surface and re-export the rest; a `pub use` satisfies the binding just
as a definition does.

| Module | Owns |
|---|---|
| `wasmtime_bridge.rs` + `wasmtime_bridge/{load,run}.rs` | WASM instance lifecycle and the FFI surface; `load` reads a file as a core module or a WASI 0.2 / 0.3 component, `run` executes it |
| `policy_preset.rs` + `presets/default.toml` | what a run closes beyond its grants, as data: reads denied, names protected inside mounts, sockets refused; extended by `--deny-read`, `--protect`, `--deny-unix`, replaced by `--preset` |
| `unix_sockets.rs` | on Linux, the credential sockets bound now that the preset closes, from `/proc/net/unix` |
| `sandbox_exec.rs`; `sandbox_profile.rs`; `landlock_policy.rs` + `landlock.rs`; `seccomp.rs` | native OS enforcement: one request, the macOS profile, the Linux ruleset and the syscalls under it, and the seccomp baseline plus proxy-only filter for what Landlock cannot see |
| `pid_namespace.rs` | the command's own user, PID and mount namespace on Linux, a fresh `/proc` in it, and the pid-1 helper that reports how the command ended; where the host refuses them the run goes ahead and says so |
| `ceilings.rs`, `memory_ceiling.rs` | resource ceilings: rlimits set between fork and exec, and the supervised wait that kills the group at `--timeout` or, on macOS, at the CPU or memory ceiling; on Linux the memory ceiling is a cgroup v2 scope asked of the systemd user manager |
| `denials.rs`, `sandbox_check.rs` | what the kernel refused during a run and which flag would have allowed it (macOS, from the unified log via a per-run tag on every deny rule); what this host can enforce, for `porta check` |
| `http_proxy.rs`, `proxy_audit.rs` | the loopback CONNECT proxy and its decision trail |
| `http_client.rs`, `host_process.rs`, `wasm_inspect.rs` | checked host services: one HTTP request, process helpers, module inspection |
| `agent_runtime.rs` + `agent_runtime/` | the broker: `loading` (config, pins, team), `guest`, `model`, `verification`, `tools`, `inspect`, `ffi` |
| `agent_journal.rs` | durable broker records, exclusive access, replay validation |
| `agent_mcp.rs` | explicitly granted remote MCP tool calls |
| `json_text.rs`, `locking.rs` | JSON escaping for hand-built replies; lock acquisition that survives poisoning |

## Build and verify

```bash
bash scripts/install-almide.sh
bash scripts/install-wasmtime.sh && export PATH="$PWD/.tools/wasmtime:$PATH"
.tools/almide/almide check src/main.almd
.tools/almide/almide build src/main.almd -o target/porta
.tools/almide/almide test --ci
python3 scripts/integration.py target/porta

bash scripts/install-codopsy.sh
.tools/codopsy/codopsy analyze . -o /tmp/codopsy.json
python3 scripts/check_grade.py /tmp/codopsy.json --min-score 90
```

CI fails below 90, the grade-A boundary. The analyzer is pinned because the
score moves with its thresholds and with how much Almide its grammar parses;
run the pinned binary, not whichever `codopsy` is on PATH. Where parse coverage
is poor the reported function boundaries are wrong, so splitting an `.almd`
file may not move its complexity number — see
`docs/roadmap/done/04-code-quality-grade.md` before chasing one.

`landlock.rs`, `landlock_policy.rs`, `seccomp.rs`, `memory_ceiling.rs` and
`pid_namespace.rs` open with `#![cfg(target_os = "linux")]`, so on macOS they
are not compiled — not type checked, not const-evaluated, not linted. Every gate above can pass while one
of them does not build. Changing one means building it on Linux before
committing:

```bash
docker run --rm -v "$PWD:/w:ro" rust:1-trixie bash -c '
  apt-get update -qq && apt-get install -y -qq python3 curl
  useradd -m dev && cp -r /w /home/dev/src && chown -R dev /home/dev/src
  su dev -c "cd /home/dev/src && rm -rf target .tools
    && bash scripts/install-almide.sh
    && .tools/almide/almide build src/main.almd -o target/porta
    && python3 scripts/integration.py target/porta"'
```

Mount the source read-only and copy it: the container must not write the host's
`target/`. Run the suite as the unprivileged user, not root: porta refuses to
run as root, so as root every positive test fails. `rust:1-bookworm` is too
old — almide needs GLIBC_2.39.

`almide test` runs a test file through `wasmtime` when it is on PATH and
otherwise builds it natively; the native path rebuilds every test binary and
costs about an hour on a cold cache. Six of the nine files take the wasmtime
path; `mcp_test`, `sandbox_test` and `wasm_rt_test` always build natively.

The published 0.63.0 artifact is currently v0.63.0-rc1. Keep CI pinned to the
verified artifact until the final release is published. JSON constructors and
accessors use `value.*`; serialization and typed key lookups use `json.*`.

## Security invariants

- Hash-pinned modules must compile from the same verified bytes; strict pinning propagates to descendants.
- Validate WASI names and types at load; reuse checked linkage, never live stores or instances.
- agent-check must not instantiate WASM, resolve credentials, contact services, or create a run.
- Never deserialize native Wasmtime code from agent-controlled files.
- Native sandbox execution must fail closed on unsupported platforms.
- A restriction the platform cannot express must refuse the run, never narrow
  the policy: macOS enforces through `sandbox-exec`, Linux through Landlock.
  Both cover writes, TCP ports and, under `--read-policy strict`, reads. On
  Linux, proxy mode needs a seccomp filter as well, because Landlock's rules
  reach TCP only; a kernel that will not take the filter refuses the run.
- A policy rule names the path the kernel resolved, never a symlink to it.
- The child's environment starts empty. `PATH`, `HOME`, the locale and the
  terminal cross by name; nothing else of the caller's shell does unless `-e`
  or `--env-pass` names it. `TMPDIR` is not among them: the sandbox's
  temporary directory is `/tmp`.
- A seccomp baseline runs in every mode on Linux, not only proxy mode: the
  syscalls that reach around a file policy (`ptrace`, `process_vm_*`,
  `execveat` of a pathless descriptor, `io_uring`, mounts, namespaces) and
  the socket families a TCP rule cannot see. A kernel that will not take the
  filter refuses the run.
- Which paths, names and sockets a run closes is the preset's, not the core's:
  no credential path is written in enforcement code. The core supplies the
  mechanisms and applies them to whatever the preset and flags resolve to.
- Inside a writable mount, the repository hooks and config and the names the
  preset protects stay unwritable, and neither they nor the mount root can be
  renamed away — on Linux through the mount namespace, and where the host
  refuses one the run says it is unprotected.
- A credential socket the preset names is refused unless `--allow-unix`
  names it; on Linux through the mount namespace, and where the host refuses
  one the run names the sockets that stay reachable.
- Under `--allow-net` no UDP leaves on either platform; on Linux an internet
  socket must be TCP, and names resolve over TCP 53.
- What the preset closes to reads stays closed in every mode on both
  platforms; on Linux without a mount namespace, a closed path inside a grant
  refuses the run. The Keychain is closed by its mach services as well.
- `run`, `up`, and MCP execution must share native policy generation.
- Proxy mode permits only the loopback proxy endpoint, without UDP, Unix
  sockets or any other egress channel — including a ring that would open a
  socket without `socket(2)`.
- MCP stdio uses newline-delimited JSON; diagnostics belong on stderr.
- MCP result adaptation must preserve multi-field payloads and error status; only legacy single-key Result envelopes unwrap.
- Agent and tool instances inherit no host environment or implicit directories.
- Delegation must not reset root budgets or load mutable policies mid-run.
- Guest output never grants capabilities, selects credentials, or changes endpoints.
- Validate tool arguments before effects; schema resolution must remain offline.
- MCP grants cannot expand through discovery; remote effects never automatically retry.
- Pre-tool checks must finish before effects; rejection cannot execute the protected tool.
- Completion checks are operator-owned read-only WASM; guests cannot bypass a failed verdict.
- Live resume must recheck current artifacts after replaying a pass from an incomplete run.
- Journal inspection is read-only metadata; uncertain outcomes must still block resume and replay.
- Journal intent must be durable before external effects; uncertain intents never retry.
- Resume counts previous model calls and steps and must not repeat completed writes.
- Journals stay outside every tool mount, including delegated agents.

## Releasing

```bash
git tag v0.5.2 && git push origin v0.5.2     # builds, tests and publishes
bash scripts/verify_release.sh v0.5.2        # installs it the way a user would
```

`.github/workflows/release.yml` runs the integration suite against the binary
it is about to publish, on the machine that produced it. `verify_release.sh`
then checks the published release from outside the repository: the archive
exists for this host, its checksum is listed and matches, the binary runs,
reports the version the tag claims, and still refuses a write outside every
mount. Publishing and being installable are different things.

Bump the version in `util.version()` and `almide.toml` together; the
integration suite asserts the binary, the generated manifest and `almide.toml`
all say the same thing.

## Repository workflow

- `main` accepts PRs from `develop`; work on `develop`.
- Commit messages are concise English without prefixes.
- Roadmap rules are in `docs/roadmap/CLAUDE.md`.
