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
| `wasmtime_bridge.rs` + `wasmtime_bridge/run.rs` | WASM instance lifecycle and the FFI surface |
| `sandbox_exec.rs`; `sandbox_profile.rs`; `landlock_policy.rs` + `landlock.rs`; `seccomp.rs` | native OS enforcement: one request, the macOS profile, the Linux ruleset and the syscalls under it, and the egress channels Landlock cannot reach |
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
.tools/almide/almide check src/mod.almd
.tools/almide/almide build src/mod.almd -o target/porta
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

## Repository workflow

- `main` accepts PRs from `develop`; work on `develop`.
- Commit messages are concise English without prefixes.
- Roadmap rules are in `docs/roadmap/CLAUDE.md`.
