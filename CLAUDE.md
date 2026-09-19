# Porta

Porta is an Almide 0.63.0 WASM agent and team runtime, with MCP interoperability.
Almide owns CLI, manifests, capability checks and MCP. `native/wasmtime_bridge.rs`
provides Wasmtime, native OS enforcement, CONNECT proxying and TOML parsing.
`native/agent_runtime.rs` brokers guest-driven agents: model credentials, scoped
WASM tools, delegation, and shared team budgets. `examples/chat-agent` is the
actual reasoning/control loop compiled to WASM. `native/agent_journal.rs` owns
durable broker records, exclusive access, and replay validation.
`native/agent_mcp.rs` handles explicitly granted remote MCP tool calls.

## Build and verify

```bash
bash scripts/install-almide.sh
bash scripts/install-wasmtime.sh && export PATH="$PWD/.tools/wasmtime:$PATH"
.tools/almide/almide check src/mod.almd
.tools/almide/almide build src/mod.almd -o target/porta
.tools/almide/almide test --ci
python3 scripts/integration.py target/porta
```

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
- `run`, `up`, and MCP execution must share native policy generation.
- Proxy mode permits only the loopback proxy endpoint, without UDP or Unix sockets.
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
