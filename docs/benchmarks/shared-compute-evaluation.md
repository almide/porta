# Shared WASM compute evaluation

This follow-up gives both runtimes the same optional Rhai computation tool while
retaining the four existing tasks, graders, eight-call budget, 512-token output
limit, pinned local Qwen3-4B revision and three repeats. Both receive the same
additional instruction explaining Rhai variables, decimal literals and exact
JSON text transport. The existing actionable verification, Porta completion
gate and task-specific Porta pre-tool policy remain enabled.

The tool is served through the same shared MCP HTTP endpoint as the existing
tools. Each `compute` request launches `porta serve` through the official MCP
SDK and executes the same WASM binary with 50 million fuel, 64 MiB memory, an
empty WASI environment and no directory preopens. The adapter's async operation
has a 20-second timeout; process cleanup is additional. Each call creates a fresh
process and WASI instance. Docker Agent uses Porta for this computation too.
This compares agent behavior with a common tool service, not independent
implementations of the tool or container versus WASM isolation.

## Results

Twenty-four trials: four tasks, three repeats, two runtimes.

| Runtime | All requirements passed | Artifact checks passed | Non-zero exit | Compute calls | Script errors | Transport/runtime errors |
|---|---:|---:|---:|---:|---:|---:|
| Porta 0.4.0 development tree, WASM agent | 0 / 12 | 3 | 12 | 49 | 49 | 0 |
| Docker Agent v1.98.0, native | 0 / 12 | 3 | 9 | 66 | 66 | 0 |

"Artifact checks passed" counts trials whose output files were correct while a
procedural requirement (reading the input, inspecting the written artifact) was
not met; with zero full passes, those three per runtime are artifact-only. The
Porta completion gate fails the process when the final verification does not
pass, so its non-zero exits include those trials.

## Findings

Every one of the 115 model-authored compute calls failed, all of them inside the
script; none failed in transport or in the runtime, and the preflight computation
succeeded through the same route.

| Script error | Calls |
|---|---:|
| Syntax error: Expecting `;` to terminate this statement | 82 |
| Data type incorrect: `core::ops::range::Range<i64>` (expecting i64) | 12 |
| Function not found: `get ((), &str \| ImmutableString \| String)` | 9 |
| Function not found: `contains_key ((), &str \| ImmutableString \| String)` | 6 |
| Function not found: `mean (array)` | 6 |

Of the 82 syntax errors, 67 are `{ key: value }` object literals, which Rhai
writes `#{ key: value }`, and 15 are `let mut` bindings, which Rhai has no
keyword for. The same brace mistake explains the failed lookups: `{}` parses as a
block evaluating to `()`, so `get` and `contains_key` are reported against `()`
rather than a map. The remaining calls use Rust range slicing such as
`input[1..]`, `unwrap_or`, and a `mean` function the engine does not define. The
model wrote Rust and JavaScript although the shared instruction stated the tool
"runs Rhai, not Python or JavaScript" and described variable declaration and
decimal accumulators. Repeated script repair consumed the fixed model-call
budget. A
computation engine executing code correctly cannot make incorrect model-authored
code correct. Every failure is a dialect mismatch rather than an arithmetic
error, so any future attempt has to change the surface the model writes against;
the engine's correctness is not the constraint.

Do not promote this expanded toolset/instruction as a quality improvement for
this model. The tool remains optional. The prior four-task results and separate
three-task capability smoke remain published; they are not replaced by easier
tasks or a larger budget. Historical comparisons differ in toolset, instructions,
host version and inference-session state, so changes from them are not a
single-variable causal estimate. Broader models and tasks remain necessary.

The MCP result-preservation fix and removal of implicit WASM directory grants
are retained independently of model scores. Before the fix, `porta serve`
unwrapped `{"ok":true,"json":"..."}` into the text `true`, losing the result.
Multi-field responses now remain intact; structured compute failures set MCP
`isError`. Legacy single-key Result envelopes retain their previous behavior.
Official SDK tests cover those cases and verify reads/writes are denied without
an explicit directory grant and permitted with one.

## Adapter diagnostic and preflight

An initial development attempt called `asyncio.run` from FastMCP's running event
loop. Its seven completed runs could not execute compute successfully; an eighth
run was interrupted. These are infrastructure diagnostics, not agent-quality
scores, and the interrupted run is not present as a completed measurement.
The corrected adapter owns its async stdio client in a separate worker thread.
A regression test covers calling it from an existing event loop.

The corrected evaluator also executes `input + 0.2` on input `0.1` through the
complete HTTP → stdio → WASM route before any model request. A failed preflight
aborts evaluation. Its request and result are stored separately from task runs.
Each task compute call records both the decoded result and inner MCP reply,
including failure flags. The shared HTTP wrapper returns the computation's JSON
status as text, like the other fixture tools; script failure is represented by
`ok:false` in that payload. Inner stdio error flags are preserved in the evidence.

## Evidence

- [All 24 trials with final prompts, model and tool journals, compute scripts,
  errors, configuration and hashes](2026-09-20-shared-compute.json.gz)
- [Scores, compute-call accounting, script-error categories, hashes and
  versions](2026-09-20-shared-compute-summary.json)
- [Harness, compute adapter, tasks, graders and manifest sources](2026-09-20-shared-compute-sources.json.gz),
  keyed by repository path; each file's SHA-256 is the matching entry in the
  report's `sha256` map.
- [Interrupted adapter diagnostic](2026-09-20-shared-compute-diagnostic.json.gz):
  the seven completed runs of the `asyncio.run` attempt. Infrastructure
  diagnostics, not agent-quality scores.

## Reproduction

Build Porta with Almide 0.63.0 and build the
[compute tool](../../examples/compute/README.md), chat guest and existing checkers.
Start the [pinned local inference server](real-model-evaluation.md) and use a
Python environment with `mcp==1.30.0` and `uvicorn`:

```bash
python scripts/evaluate_agents.py \
  --endpoint http://127.0.0.1:18764/v1/chat/completions \
  --model /tmp/porta-real-model --model-source /tmp/porta-real-model-source.json \
  --completion-check examples/verify-json/verify-tool.wasm \
  --before-tool-check examples/verify-json/before-tool.wasm \
  --compute-wasm examples/compute/compute.wasm \
  --verification-feedback actionable --repeats 3 \
  --output target/eval-shared-compute-fixed.json
python3 scripts/audit_eval.py target/eval-shared-compute-fixed.json
python3 scripts/audit_eval.py docs/benchmarks/2026-09-20-shared-compute.json.gz
```

The audit regrades artifacts and procedural requirements, checks all 24 trials
and matched model-visible inputs, validates the successful compute preflight,
and checks that decoded compute values and error status match the inner MCP
wire result. It reports script errors separately from transport/runtime errors.
It does not rerun the computation or authenticate the report. Compute service
startup/compilation is included in run duration; this is not a latency benchmark.
