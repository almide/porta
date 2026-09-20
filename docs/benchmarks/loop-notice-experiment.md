# Rejected experiment: repeated-tool notices

On 2026-09-20 we tested whether the WASM chat guest should detect repeated tool
calls and ask the model to reconsider its approach. The motivating failures were
repeated incorrect invoice writes after actionable verification feedback. This
experiment did not improve completion, and the default guest was restored to its
previous source and binary. The rejected implementation remains available as
measurement evidence, not as a shipped feature.

## Intervention

The candidate compared function names, parsed arguments and model-visible results
in the last 40 conversation messages. Three identical consecutive calls, or two
repetitions of a two-to-four-call cycle, appended a brief user message asking the
model to recheck the task and failure evidence. User messages reset the window.
The notice allowed intentional polling and never skipped, retried or rolled back
a tool. It appeared only after the entire pending batch completed and did not
increase any budget. No task-specific answer was added.

Real-WASM fixture tests verified identical-call and two-call-cycle detection,
changed arguments/results, intact pending batches, checkpoint reconstruction,
offline replay without repeated writes, and unchanged model budgets. Existing
agent, completion-check and malformed-batch integration suites also passed with
the candidate. These tests establish behavior, not model quality.

## Matched evaluation

| Matrix | Porta full pass | Docker Agent full pass | Porta artifact pass | Docker Agent artifact pass |
|---|---:|---:|---:|---:|
| Candidate with repeated-tool notice | 3/12 | 3/12 | 6/12 | 7/12 |
| Current guest without notice (control) | 4/12 | 3/12 | 6/12 | 7/12 |

All 48 runs are retained and independently regraded. Porta passes the mean-fix
task three times in each matrix. It passes release lookup zero times with the
candidate and once with the control; both produce the correct release artifact
three times. Neither Porta guest passes invoice totals or configuration migration.
Docker Agent passes release lookup three times in each matrix. Its one correct
configuration artifact in each matrix still fails the full task criterion.

The one-pass difference does not establish that notices caused a regression:
no release trial received a notice. The evidence supports rejecting the candidate
for lack of demonstrated benefit, not a statistically established effect size.

Both matrices retain the four existing tasks, three repeats, fixed model revision,
temperature zero, 512 output tokens, eight model calls, shared MCP tools,
actionable verification, completion checks and the task-specific pre-tool policy.
The host, harness, grader, feedback and checker hashes are identical between the
two new matrices. Only the Porta guest changes. Each matrix alternates runtime
order; the candidate matrix ran before the control matrix on the same local
inference server. This is a sequential ablation, not randomized interleaving.

The earlier [policy matrix](pre-tool-policy-evaluation.md) remains historical
evidence. Its Porta 6/12 score must not be presented as the current result: the
current control was rerun instead of attributing a cross-version difference to
the candidate. Temperature zero alone does not guarantee identical model
trajectories across sessions; generated call IDs, tool error paths and inference
server/cache state can differ. The release-task failure in the candidate did not
receive a loop notice, so its change from the historical result is not evidence
that the notice caused that failure.

## Findings and decision

The candidate notice reached the model in all three invoice trials, at model
call eight. None passed. It appeared in no other task. Configuration migration
still changed nested fields that the task required preserving. Both task classes
need correct computation/transformation and successful correction, not merely a
warning that work is repeating. Release lookup could produce a correct artifact
and verification at call eight but lacked the next model call needed to finish.
The existing success criterion and budget were retained.

Do not add this heuristic to the default guest without evidence of benefit on
broader tasks. The next quality gate remains reliable tool-assisted computation
and editing, followed by the unchanged suite and multiple models/public tasks.
The small local suite, native Docker Agent execution, explicit Porta-only
pre-tool enforcement and all previous comparison limits still apply. This
experiment is not a general comparison of agent quality or isolation modes.

## Reproduction artifacts

- [Candidate guest source](2026-09-20-loop-notice-guest.almd.gz)
- [Control guest source](2026-09-20-loop-control-guest.almd.gz)
- [Candidate integration tests](2026-09-20-loop-notice-tests.py.gz)
- [Exact evaluation harness](2026-09-20-loop-evaluation-harness.py.gz)
- [All candidate runs](2026-09-20-loop-notice.json.gz)
- [All control runs](2026-09-20-loop-control.json.gz)
- [Hashes, versions, per-task scores and notice positions](2026-09-20-loop-experiment-summary.json)

Both saved matrices are audited in CI. The archived candidate source was rebuilt
with Almide 0.63.0 and its WASM SHA-256 matched the measured candidate. The restored
default WASM matches the control and the preceding startup measurement's guest.

```bash
python3 scripts/audit_eval.py docs/benchmarks/2026-09-20-loop-notice.json.gz
python3 scripts/audit_eval.py docs/benchmarks/2026-09-20-loop-control.json.gz
```

Build either archived source with the pinned Almide 0.63.0 compiler. For example:

```bash
gzip -dc docs/benchmarks/2026-09-20-loop-notice-guest.almd.gz > target/loop-notice.almd
.tools/almide/almide build target/loop-notice.almd --target wasm -o target/loop-notice.wasm
gzip -dc docs/benchmarks/2026-09-20-loop-notice-tests.py.gz > target/loop-notice-tests.py
python3 target/loop-notice-tests.py target/porta target/loop-notice.wasm examples/demo-agent/agent.wasm
```

Use the [pinned model setup](real-model-evaluation.md) and a Python environment
containing `mcp==1.30.0` and `uvicorn`. Run the unchanged harness with either guest:

```bash
python scripts/evaluate_agents.py \
  --endpoint http://127.0.0.1:18764/v1/chat/completions \
  --model /tmp/porta-real-model --model-source /tmp/porta-real-model-source.json \
  --agent target/loop-notice.wasm \
  --completion-check examples/verify-json/verify-tool.wasm \
  --before-tool-check examples/verify-json/before-tool.wasm \
  --verification-feedback actionable --repeats 3 \
  --output target/eval-loop-model.json
python3 scripts/audit_eval.py target/eval-loop-model.json
```
