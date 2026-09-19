# Pre-tool policy with actual local inference

This is a historical matrix. The [2026-09-20 follow-up](loop-notice-experiment.md)
reruns the current guest and records different scores; do not use this page's
6/12 result as the current quality measurement.

This matrix retains all four tasks, grading rules, model revision, initial
system/user messages, granted model-visible tools, and eight-call model budget
from the [compact-result matrix](compact-results-evaluation.md). The Porta host
is now broker version 5 and enforces a pre-tool policy for the task that explicitly
requests tests before editing. Other tasks receive no pre-tool policy. The WASM
chat guest and the completion checker are unchanged.

The configured prerequisite is `run_tests` with no arguments before `write_file`.
It does not supply an answer, change the task, run a test on the model's behalf,
or relax the existing grader. A rejected write does not reach the shared MCP
server; feedback asks the model to satisfy the prerequisite and consumes its
existing budget. The pre-edit tests may fail, as expected for the broken input.
The model must still fix the function, run tests after editing, pass independent
verification and finish within budget.

## Results

| Criterion | Porta with pre-tool policy | Docker Agent instruction only |
|---|---:|---:|
| Full requirements and successful exit | 6 / 12 | 3 / 12 |
| Correct artifacts, ignoring procedure | 6 / 12 | 6 / 12 |
| Nonzero process exit | 6 / 12 | 9 / 12 |

Porta passes all three mean-function runs and all three release-lookup runs.
Docker Agent passes the release-lookup runs, but still omits pre-edit tests in the
mean-function task. Both still fail invoice totals and configuration migration.
This is an improvement in procedure compliance, not in final artifact accuracy.

Every protected Porta run executes this sequence:
`read_file → run_tests → write_file → run_tests → verify_task`.
An earlier requested write is rejected before reaching the MCP server. A later
premature final answer is rejected by the completion check until `verify_task`
passes. Each run uses all eight permitted model calls, so this recovery has no
remaining model-call margin. The independent audit rechecks operation order and
final function behavior; the criteria are identical to earlier matrices.

| Task | Porta pass | Docker Agent pass |
|---|---:|---:|
| Invoice totals | 0 / 3 | 0 / 3 |
| Configuration migration | 0 / 3 | 0 / 3 |
| Empty-list mean fix | 3 / 3 | 0 / 3 |
| Release lookup | 3 / 3 | 3 / 3 |

- [All 24 runs, including failures and exact configurations](2026-09-19-pre-tool-policy.json.gz)
- [Versions, hashes, per-task scores and observed ordering](2026-09-19-pre-tool-policy-summary.json)

```bash
python3 scripts/audit_eval.py docs/benchmarks/2026-09-19-pre-tool-policy.json.gz
```

The artifact audit passes. Negative checks also confirmed that it rejects a
forged prerequisite or a recorded write moved before the first test. The guest,
tasks, grader, feedback and completion-check hashes match the preceding matrix;
the host and harness changed to implement and configure pre-tool enforcement.

## Comparison scope

This compares **Porta with an explicit operator policy** against **Docker Agent
following the same task instruction without an equivalent host policy configured**.
It is not evidence that Docker Agent cannot implement a similar prerequisite
using another configuration or integration. Model-visible starting inputs and
shared tools are matched; host-side enforcement intentionally differs. All
per-task Porta configurations and the policy module hash are preserved.

The usual limits remain: one small quantized model, four author-created tasks,
a bounded Python-function fixture rather than a real repository, local macOS
inference, and a buffering gateway. Durations are not a general speed comparison.
Previous failures and matrices remain published. Improved results here do not
establish general agent superiority, production readiness, or broader benchmark
leadership.

## Audit and reproduction

The audit requires Python 3.11 or later for standard-library TOML parsing (CI
uses Python 3.12). It regrades every final artifact and verification snapshot, checks matched
model-visible inputs, checks the saved task-specific policy against the declared
prerequisite, and ensures no executed Porta write precedes `run_tests` on the
protected task. Denied attempts are visible in the saved model conversation and
Porta verification metrics, not in the MCP server's executed-operation history.

Build the current Porta host, chat guest, `verify-tool.wasm` and `before-tool.wasm`,
then start the pinned local inference server using the
[baseline instructions](real-model-evaluation.md):

```bash
/tmp/porta-model/bin/python scripts/evaluate_agents.py \
  --endpoint http://127.0.0.1:18764/v1/chat/completions \
  --model /tmp/porta-real-model --model-source /tmp/porta-real-model-source.json \
  --completion-check examples/verify-json/verify-tool.wasm \
  --before-tool-check examples/verify-json/before-tool.wasm \
  --verification-feedback actionable --repeats 3 \
  --output target/eval-policy-model.json
python3 scripts/audit_eval.py target/eval-policy-model.json
```
