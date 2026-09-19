# Checks before tool effects

`before_tool_checks` lets an operator require a read-only WASM policy before any
granted local tool, remote MCP call, or delegation is executed. This can enforce
requirements that completion-time checks cannot repair, such as running tests
before the first edit. It does not infer policies from task text: configure the
required policy explicitly.

```toml
[[before_tool_checks]]
name = "test-before-edit"
wasm = "before-tool.wasm"
parameters = {tool="write_file", requires="run_tests"}
```

The [sample verifier](../examples/verify-json/src/bin/before-tool.rs) requires a
completed local operation with the configured name before allowing the protected
tool. Optional `requires_arguments` and `requires_result` require exact JSON value
matches. Reported `error`, `err`, or MCP `isError` results do not satisfy it. A
normal test report can contain failing tests: this sample enforces execution,
not test success, unless an appropriate expected result is configured. It checks
any preceding matching call in the current agent invocation, not freshness after
every edit. Write a stronger policy when that distinction matters.

## Protocol

Each check receives one JSON line:

```json
{"kind":"before_tool","tool":{"name":"write_file","arguments":{"path":"stats.py","content":"..."}},"parameters":{"tool":"write_file","requires":"run_tests"},"tool_calls":[{"name":"run_tests","arguments":{},"result":{"passed":0,"total":5}}]}
```

`tool_calls` contains only actual completed operations observed by the broker.
Rejected operations and guest-supplied history cannot satisfy prerequisites.
The pending action is supplied separately. Local histories do not automatically
include a delegated child's internal calls: configure the child's checks as well
as any policy on the parent's delegation action.

Checks return the same strict verdict as [completion checks](completion-checks.md):
`{"passed":true}` or `{"passed":false,"feedback":"what must happen first"}`.
Every check must pass in configuration order. Checks apply to all granted tools;
the checker decides which names/arguments it protects and permits unrelated calls.
A rejection returns a tool result with `error.code="tool_policy_rejected"`, the
check name and feedback, preserving the guest's pending state. No protected tool
execution or MCP connection occurs, and executed-tool metrics do not increment.
The included chat guest can correct the issue through its normal tool loop.
A guest may still stop without doing the task; combine pre-effect policy with
completion checks to require successful work before accepting completion.

## Isolation, budgets and recovery

There are at most 16 pre-tool checks per agent, independently of the completion
check limit. Policy modules, parameters and read-only mounts are frozen with the
entire team before execution. There is no inherited environment, ambient host
filesystem, network or process execution. Journals must remain outside all policy
mounts. Invalid verdicts, traps and deadlines fail closed. Each check consumes an
agent and root-team step, fuel and remaining wall-clock time. The existing
`verification_calls` and `verification_failures` metrics include both phases.

Checks and their results are journaled as `before_tool_check` operations. Offline
replay uses recorded decisions. Live recovery that reaches an unfinished record
immediately after a cached passing check reruns the checks before a new effect.
Repeated interrupted recheck rounds are reconstructed from the journal. Completed
tool effects are not repeated. An unfinished check intent is treated as uncertain
and fails closed, even though the verifier itself only has read-only mounts.
Broker fingerprint version 5 binds both check phases and rejects earlier broker
journals; use the original broker and guest artifacts for older records.

History-only policies certify recorded operations, not current external state.
Artifact-reading policies can recheck current files, but there is no atomic lock
between a check and a tool effect: another host process can change an artifact in
that interval. Protect or transact the underlying resource if that guarantee is
required. This mechanism enforces operator policy; it does not establish that the
policy is sufficient for arbitrary task correctness.

## Build and test

```bash
cargo build --locked --manifest-path examples/verify-json/Cargo.toml \
  --target wasm32-wasip1 --release --target-dir target/verify-json
cp target/verify-json/wasm32-wasip1/release/before-tool.wasm examples/verify-json/before-tool.wasm
python3 scripts/before_tool_integration.py target/porta examples/chat-agent/agent.wasm \
  examples/demo-agent/agent.wasm examples/verify-json/before-tool.wasm examples/verify-json/check.wasm
```

The suite verifies actual write prevention and correction, forged history,
checkpoint/resume, changed artifacts at a pre-effect recovery boundary, offline
replay, budgets, mounts, malformed verdicts and delegated policy enforcement.
The MCP suite additionally verifies rejection before any remote connection.
These fixture tests do not establish improved real-model completion rates; the
published quality comparisons predate this broker change.
