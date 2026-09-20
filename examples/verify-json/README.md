# JSON artifact completion check

This is an operator-owned WASI program for Porta's completion gate. It reads one
JSON request from stdin, reads the configured artifact through a read-only WASI
mount, compares it to `parameters.expected`, and prints a verdict. Optional
`parameters.required_tool_calls` checks minimum call counts from host-observed
local tool history. Counts alone do not establish call ordering or successful
inspection; implement stronger checks when those are requirements.

```bash
rustup target add wasm32-wasip1
cargo build --locked --manifest-path examples/verify-json/Cargo.toml \
  --target wasm32-wasip1 --release --target-dir target/verify-json
cp target/verify-json/wasm32-wasip1/release/porta-verify-json.wasm examples/verify-json/check.wasm
```

Requests and artifacts are bounded to 1 MiB. Missing/unreadable files, malformed
JSON, unequal JSON values and unmet required call counts return `passed: false`
with a diagnostic. Object key ordering and whitespace are insignificant. Numeric
representation is significant (`42`, `42.0` and `42e0` may compare differently);
this is exact serde_json value equality, not a universal semantic equivalence
checker. Arbitrary-precision parsing preserves large numeric representations.

The checked value comes from the artifact, not the agent's candidate answer.
The checker cannot protect against an operator configuring a wrong expected
value or replacing it with an always-passing program. Its source, parameters and
mounts are part of the trusted policy.

This sample uses Rust because the pinned Almide 0.63.0 compiler rejects the
filesystem-read plus JSON-parse shape for stock WASI. A reproducer is preserved
in `scripts/fixtures/verify_json_compiler_repro.almd` and documented in
`docs/compiler-requests.md`. The Porta CLI and chat agent remain on Almide 0.63.0.

## Require a dedicated MCP verification result

The same build produces `verify-tool.wasm`:

```bash
cp target/verify-json/wasm32-wasip1/release/verify-tool.wasm examples/verify-json/verify-tool.wasm
```

Configure a dedicated, operator-trusted MCP verifier as an ordinary granted tool,
then require its result before accepting completion:

```toml
[[completion_checks]]
name = "task-verification"
wasm = "verify-tool.wasm"
parameters = {tool="verify_task"}
```

This checker requires the latest host-observed tool call to be `verify_task`,
with an unambiguous MCP JSON result containing boolean `passed: true`. The
FastMCP string-return wrapper `structuredContent: {result: "<JSON>"}` is
normalized only when it has exactly that one field; it must agree with text. An MCP
error, malformed JSON, conflicting text/structured results, or another tool call
after verification rejects completion. The model cannot supply its own history.
The verifier must independently inspect the task; an echo tool or a tool that
reads model-written verdicts is not a trusted verifier.

This is evidence of a recorded verification, not a fresh filesystem inspection.
It cannot detect external changes after the tool returned, including changes
between pause and resume. Rerunning this history-only checker during live resume
still reads that historical report. For mutable artifacts, also configure an
artifact-reading completion check or require a new independent verification.
Do not interpret this sample alone as protection against artifact drift.

```bash
cargo test --locked --manifest-path examples/verify-json/Cargo.toml
python3 scripts/mcp_agent_integration.py target/porta examples/chat-agent/agent.wasm examples/verify-json/verify-tool.wasm
```

## Require a prerequisite before effects

The build also emits `before-tool.wasm`, a sample for
[pre-tool checks](../../docs/before-tool-checks.md). It can require an actual
completed `run_tests` call before allowing `write_file`, with optional exact
argument/result constraints. Unlike completion checks, it runs before the
protected operation and can reject an edit before its side effect occurs.
