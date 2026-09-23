# WASM agents and teams

`porta agent` executes the agent's decision loop inside WebAssembly. The provided
Almide agent handles chat-completions responses, requests WASM tool calls, resumes
with their results, and returns an answer. Porta owns only the capability broker:
model transport, credentials, tool grants, and execution budgets.

```bash
bash scripts/install-almide.sh
.tools/almide/almide build src/main.almd -o target/porta
.tools/almide/almide build examples/chat-agent/src/main.almd --target wasm -o examples/chat-agent/agent.wasm
.tools/almide/almide build examples/demo-agent/src/main.almd --target wasm -o examples/demo-agent/agent.wasm
# Inspect the complete team without credentials or execution.
target/porta agent-check examples/chat-agent/team.toml
# Set model endpoint/name in examples/chat-agent/agent.toml first.
target/porta agent examples/chat-agent/agent.toml -- 'What is 20 + 22? Use the tool.'
target/porta agent examples/chat-agent/team.toml -- 'Ask the calculator to add 20 and 22.'
```

The provider must implement non-streaming chat completions with function tool
calls, such as a suitably configured [vLLM server](https://docs.vllm.ai/en/latest/serving/online_serving/openai_compatible_server/).
No paid provider or downloaded model is needed for the regression suite: it runs
against a deterministic local model fixture and executes real compiled WASM.

Use [agent-check](agent-check.md) to inspect resolved grants, budgets, artifact
pins and the entire team before execution.

## Configuration

Unknown fields are rejected. Paths are relative to the config file, independent
of the working directory. The complete team and all WASM modules are loaded
before running any guest. Cycles and depth greater than four agents are rejected.

```toml
version = 1

[agent]
wasm = "agent.wasm"
instruction = "Answer using the tools available to you."

[model]
endpoint = "https://model.example/v1/chat/completions"
name = "your-model"
token_env = "MODEL_API_KEY"

[limits]
max_steps = 64
max_model_calls = 16
fuel_per_step = 50000000
memory_pages = 1024          # 64 MiB per WASM instance
max_output_tokens = 2048     # Sent as max_tokens to the model provider
timeout_seconds = 120

[[tools]]
name = "read_file"
wasm = "tools.wasm"
description = "Read a file from the provided workspace."
input_schema = { type = "object", properties = { path = { type = "string" } }, required = ["path"] }
mounts = [{ host = "workspace", guest = ".", read_only = true }]

[[agents]]
name = "reviewer"
config = "reviewer.toml"
description = "Review a proposed solution independently."
```

A delegated agent appears as `delegate_reviewer`, with one string argument,
`task`. Children have their own model configuration, conversation state, WASM
module, and explicitly granted tools. They do not receive the parent's history,
credentials, mounts, or tool registry. Parent and child configurations are trusted
operator inputs, not files supplied by a guest at delegation time.

## Enforced boundaries

- Agent WASM gets in-memory stdin/stdout, no inherited environment, no directories,
  no network sockets, and no native process execution imports.
- Each tool runs in a fresh WASI instance. Only its explicitly declared directories
  are preopened. Mounts default to read-only; writable access is opt-in. Wasmtime
  confines traversal and symlinks to the grant.
- API tokens stay in the host and are attached only to the configured model URL.
  HTTPS is required except for explicit loopback endpoints. Redirects and inherited
  proxy settings are disabled. Response bodies are limited to 1 MiB.
- The root's step count, model-call count and deadline apply across the whole team.
  A child also observes its local limits. A child cannot multiply the root budget
  by delegating again. Fuel and memory limits apply to each guest invocation.
- Each WASM invocation has a fresh store, bounded stdout/stderr and memory, fuel,
  and an epoch deadline. The process stops the run on an exhausted budget, unknown
  tool, malformed action, guest trap, or model transport failure.
- Guest state and tool output are untrusted data. They cannot change model URLs,
  credentials, tool schemas, grants, or root budgets.

The deadline interrupts CPU-bound WASM and bounds HTTP transport. It does not
promise preemption of every operating-system filesystem call (for example a hung
network filesystem). `max_output_tokens` is a provider request parameter, not a
billing guarantee. Tool arguments must be objects. The broker compiles every tool's
`input_schema` at launch and validates arguments before instantiating the tool or
writing a journal intent. Invalid arguments return a tool result with
`error.code = "invalid_tool_arguments"` and a bounded `violations` list naming
what failed and where, for example
`"arguments: Additional properties are not allowed ('audit' was unexpected)"`.
The detail is deliberate: a rejection that only says "invalid" leaves a model
guessing, and a guessing model tends to resend exactly what was refused, so the
call is contained but the task fails too. Nothing in the list is new to the
caller — the schema was already sent to it as part of the tool definition, and
the values are its own. Rejected attempts consume steps and any subsequent model
calls consume the shared model budget; rejected attempts do not increment
executed `tool_calls`.

Validation uses [jsonschema 0.56.0](https://docs.rs/jsonschema/0.56.0/jsonschema/)
with offline resolution and default network/file retrieval features disabled.
Internal references work; unresolved or external references fail at launch.
`$schema` selects a supported draft; omission uses the library's default draft
2020-12. Known formats are asserted and unknown formats are rejected. Schemas are
operator configuration, not guest-editable policy. As in JSON Schema, unknown
keywords can be annotations; this does not detect every misspelled keyword.
The schema validator runs in the host and is not metered by WASM fuel.

Schema rejection is recomputed during replay, without recording a tool effect.
The journal broker fingerprint is now version 5; journals from the earlier broker
are refused rather than replayed under different validation rules.

`model.temperature` is optional. When present it must be finite and within
0–2, and is sent unchanged to the configured model. Omitting it preserves the
provider's default. Set `temperature = 0.0` for controlled greedy-decoding
comparisons; this does not guarantee bitwise deterministic inference across
servers or hardware.

See [verified completion](completion-checks.md) for optional operator-owned WASM
checks that must pass before an agent can finish.

## Guest protocol, version 1

Each invocation reads one UTF-8 JSON line from stdin and writes one JSON action
on stdout. Diagnostics can use stderr. Compiled modules are reused within a run;
linear memory and WASI state are fresh for each invocation.

| Event | Fields |
|---|---|
| `start` | `task`, `instruction`, `tools` |
| `model` | `response`, `state` |
| `tool` | `name`, `result`, `state` |

| Action | Fields |
|---|---|
| `model` | `messages` array, optional `state` |
| `tool` | granted `name`, object `arguments`, optional `state` |
| `done` | string `output` |
| `fail` | string `error` |

`state` is the guest-owned continuation passed to the next invocation. The host
never interprets it as authority. Existing WASM tools use the existing Porta tool
ABI: a four-byte little-endian byte length followed by JSON containing `tool` and
`arguments`; stdout is a JSON result. Delegation produces a tool result containing
`{"ok": "child output"}`. The included guest supports multiple tool calls in one
model response, executed sequentially.

Final answers go to stdout. Structured metrics go to stderr prefixed with
`[porta agent]`; counters cover the complete team. No conversation or secret is
written to a persistent host log by default. Opt-in [durable journals](agent-journals.md)
record broker operations and support checkpoint/resume and verified offline replay.

## Verification

```bash
.tools/almide/almide build scripts/fixtures/guest_isolation.almd --target wasm -o target/guest-isolation.wasm
python3 scripts/agent_integration.py target/porta examples/chat-agent/agent.wasm examples/demo-agent/agent.wasm target/guest-isolation.wasm
python3 scripts/schema_integration.py target/porta examples/chat-agent/agent.wasm examples/demo-agent/agent.wasm
```

Tests execute the complete model/tool loop, read-only and writable grants,
traversal and symlink attacks, a WASI environment probe, unknown tools, budget
exhaustion, an infinite-loop deadline, multi-agent delegation, shared budgets,
and cyclic configuration rejection. Linux/macOS CI runs the same WASM artifacts.

Remote tools are available through [explicit MCP grants](agent-mcp.md), with
JSON/SSE HTTP responses and durable replay.

Not yet implemented: model streaming, full-memory
snapshots, signed distribution, parallel delegation, and
broader multi-model quality evaluations. These remain delivery gates, not claimed features.

## Repairing invalid model responses

The included Almide chat guest can recover when a successful model response has
no tool calls and no non-whitespace text, including replies declaring tool calls
but providing none. It sends a new model request with existing conversation and
tool results plus a correction instruction. It does not append a malformed blank
assistant message. Each recovery consumes the existing model-call, step and time
budgets; at most two consecutive recovery requests are made. Empty replies and
malformed tool-call batches share this limit. A usable response resets the counter. An explicit refusal is a failure, not a
request to retry.

The counter travels in guest state and survives durable reconstruction. Tests
pause after empty replies following an actual file write, modify that file, and
resume: the recorded write is not repeated, the counter does not reset, and
completed replay makes no network requests. This prevents automatic duplication
by recovery; the model can still explicitly request a new tool operation, subject
to the usual policy and budgets.

Recovery is a new guest-requested model action. HTTP transport errors and
uncertain external intents still fail closed; there is no automatic HTTP retry. A changed guest WASM changes journal fingerprints; older journals
require their original guest artifact for replay. Historical model comparisons
predate this change. The [current-guest follow-up](benchmarks/loop-notice-experiment.md)
does not isolate response repair as a cause of quality improvement.

Before dispatching a model tool-call batch, the chat guest validates every call:
an assistant message, nonempty unique IDs within the batch, `type: "function"`, nonempty function names,
and argument strings that parse as JSON objects. A non-array `tool_calls` field
is also rejected. If any call is malformed, no call from that response executes.
The invalid assistant message is omitted from subsequent model requests and the
model receives a repair instruction. Previously completed conversation and tool
results remain available. IDs and ordering from a corrected batch are preserved.

This validates response structure. Accepted calls subsequently execute in order
under host grants, argument schemas, pre-tool policies and budgets. If a later
call fails those checks or its external operation fails, earlier completed
operations remain; execution is not an atomic transaction. Provider refusals
remain terminal. Only the included chat guest implements this repair behavior;
custom guest programs define their own response handling.

```bash
python3 scripts/batch_recovery_integration.py target/porta examples/chat-agent/agent.wasm examples/demo-agent/agent.wasm
```

This test executes real WASM and checks whole-batch rejection before file effects,
corrected call IDs, mixed invalid/empty reply limits, prior completed effects,
partial-batch resume and offline replay. These fixture tests do not establish
improved completion rates; see the [current-guest measurements](benchmarks/loop-notice-experiment.md).
