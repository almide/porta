# Verified completion

`completion_checks` lets an operator require independent evidence before accepting
an agent's `done` action. The host executes every configured check in order, using
fresh WASI instances. All checks must pass. A failure returns feedback to the WASM
agent so it can correct its work within the existing budgets.

```toml
[[completion_checks]]
name = "artifact"
wasm = "verify-json.wasm"
parameters = {path="result.json", expected={value=42}, required_tool_calls={write_file=1, read_file=1}}
mounts = [{host="workspace", guest="."}]
```

There may be at most 16 uniquely named checks. Check modules and parameters are
loaded with the complete team before execution. Mounts must be read-only; a
writable grant is rejected at launch. Check code has no inherited environment,
implicit directories, network sockets or host execution imports. Verifiers use
the same per-invocation fuel, memory, output and deadline limits as other WASM.
Each check also consumes one agent and root-team step.

The verifier gets one JSON line on stdin:

```json
{"kind":"verify","candidate":"proposed answer","parameters":{"path":"result.json","expected":{"value":42}},"tool_calls":[{"name":"write_file","arguments":{"path":"result.json","content":"{\"value\":42}"},"result":{"ok":"wrote 12 bytes"}}]}
```

`candidate` is untrusted agent output. `tool_calls` is rebuilt by the broker from
actual completed local tool/delegation calls, including recorded calls during
resume. It does not come from the agent's supplied state. The history includes
arguments and results, is bounded to 512 KiB, and is only retained when checks are
configured. Total verifier input is bounded to 1 MiB. Child agents have their own
histories; a parent's delegation entry contains the child's returned result.

The check prints one JSON verdict:

```json
{"passed":false,"feedback":"The artifact is missing; write and inspect it before completing."}
```

`passed` must be a boolean. A rejected verdict must have a nonempty feedback
string. Unknown verdict fields, malformed JSON, traps and budget exhaustion stop
the run; they never count as a pass. Feedback is optional for an accepted verdict.

## Correction and budgets

A guest may attach `state` to its `done` action. When a check rejects completion,
the broker sends `kind: "verification_failed"` with that state, the candidate and
`failure: {check, feedback}`. The included Almide chat agent preserves its
conversation and asks the model to correct the reported failure. Older guests
that do not handle this event fail rather than bypass checks.

Checks are not exposed as model-callable tools and cannot be disabled, replaced
or given broader mounts by guest output. Repeated `done` claims rerun the checks
and consume steps. Correction calls consume the same model-call and deadline
budgets. Delegated agents must pass their own checks before returning a result;
a parent can additionally configure independent checks of its own.

Metrics include root-team `verification_calls` and `verification_failures`.
An operator can write a verifier that always returns true, or checks the wrong
property. This feature enforces the configured acceptance policy; it does not
prove that arbitrary policies establish task correctness.

## Recovery semantics

Verification requests, results and fuel consumption are journaled. Check module
hashes, mounts and parameters participate in the complete-team fingerprint.
Journals must remain outside check mounts as well as tool mounts. Broker identity
version 5 refuses journals from earlier execution rules.

Offline replay verifies historical decisions using recorded verdicts, without
reading today's artifact files. It does not certify the current filesystem.
When a live resume reaches the end of an incomplete record after a cached pass,
the host reruns all checks before accepting a new completion. Artifact-reading
checks inspect current artifacts; history-only checks still see recorded results
and cannot establish that external state has remained unchanged. Subsequent resumes also replay those additional rounds correctly.
Completed writes and model calls are not repeated by reconstruction. New repair
actions explicitly requested by the guest may perform new writes.

Checks run sequentially and do not snapshot or lock all artifacts. Another host
process can change a file during or after verification. Validation feedback can
also become stale while an agent corrects another issue; all checks rerun on the
next completion attempt. State these limits when interpreting an accepted run.
An unfinished verifier intent currently uses the same fail-closed uncertain-intent
rule as other broker operations, even though check mounts are read-only.

## Example and tests

Build the [JSON artifact verifier](../examples/verify-json/README.md), the chat
agent and demo tools, then configure the model in
[verified.toml](../examples/chat-agent/verified.toml):

```bash
mkdir -p target/verified-workspace
target/porta agent examples/chat-agent/verified.toml --record target/verified-run.jsonl -- 'Write result.json containing {"value":42}, then inspect it.'
```

The JSON verifier compares parsed values and can require tool-call counts. It is
an example of an operator policy, not a general coding-task evaluator. In
particular, call counts alone do not prove that tests ran before an edit or that
a read occurred after the last write. Such sequencing requires a stronger
history-aware check or [pre-tool policy](before-tool-checks.md).

```bash
.tools/almide/almide build scripts/fixtures/verify_readonly.almd --target wasm -o target/verify-readonly.wasm
python3 scripts/verification_integration.py target/porta examples/chat-agent/agent.wasm examples/demo-agent/agent.wasm examples/verify-json/check.wasm target/verify-readonly.wasm
```

The suite executes real WASM and real file effects: premature completion,
correction, host-observed operations, read-only/outside-file/environment isolation,
malformed verdicts, runaway verifier deadlines, changed verifier fingerprints,
artifact drift at recovery boundaries, completed offline replay, and delegated
verification under shared budgets. The model replies are fixtures; improved
real-model completion rates must be measured separately against the preserved
baseline, not inferred from this integration test.

The [dedicated MCP verifier example](../examples/verify-json/README.md#require-a-dedicated-mcp-verification-result)
requires a passing result from the most recent actual tool operation. It rejects
missing, failed, malformed, errored and conflicting reports. Its trust and
freshness limits differ from artifact-reading checks: a recorded tool verdict
does not establish the current state of external files, even after live resume.
