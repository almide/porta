# Verification-assisted real-model evaluation

This follow-up retains all four tasks and grading rules from the
[first matched evaluation](real-model-evaluation.md). Both runtimes receive the
same `verify_task` MCP tool and instruction to correct failures before finishing.
Porta additionally requires a passing report through an operator-owned WASM
completion check. The verifier uses current files and observed operation order;
it does not accept the model's claim of success.

The actionable variant reports incorrect JSON field paths, missing output
inspection, and omitted pre-edit tests. It does not reveal expected values or
relax requirements. In particular, testing before the first edit cannot be
repaired retrospectively. The eight-call model budget is unchanged, so spending
calls on verification leaves fewer calls for editing.

## Corrected matrix results

| Result | Porta WASM | Docker Agent native |
|---|---:|---:|
| All requirements and successful process exit | 3 / 12 | 3 / 12 |
| Correct artifacts, ignoring procedure | 6 / 12 | 6 / 12 |
| Nonzero process exit | 9 / 12 | 9 / 12 |

All full passes were release lookup tasks. Porta's three successful runs each
executed and passed the host completion check. Both runtimes still failed invoice
and configuration tasks; the mean fixes passed function tests but omitted the
required pre-edit test. Eight of Porta's nine failed runs exhausted model-call
budgets; one received a model reply with neither content nor tool calls.

Compared with the baseline, both moved from 0/12 to 3/12 full passes. The shared
verification tool, added instruction and actionable feedback changed together;
this does not isolate a causal benefit of Porta's completion gate. It also does
not establish quality superiority over Docker Agent. Durations are retained for
audit, not reported as a speed advantage.

- [Complete corrected report: all 24 runs](2026-09-19-verification-fixed.json.gz)
- [Versions, hashes and scorecard](2026-09-19-verification-fixed-summary.json)

The audit independently regrades final artifacts and each verification snapshot,
checks feedback against the captured history, and compares model-visible inputs.
CI checks the saved corrected report without downloading or running a model.

```bash
python3 scripts/audit_eval.py docs/benchmarks/2026-09-19-verification-fixed.json.gz
```

## Compatibility correction

Development runs exposed the MCP SDK's string-return representation: JSON text
is duplicated in `structuredContent.result`. The initial verifier expected the
structured value itself to be the verdict and rejected this wrapper. The fixed
verifier normalizes only the exact one-field wrapper, parses its JSON, and
requires agreement with text. Malformed, errored and conflicting results still
reject completion. Rust and real-WASM MCP tests cover both representations.

The two pre-fix matrices are preserved as diagnostic evidence, not evidence for
the corrected completion gate. The final matrix uses the corrected WASM for all
runs. Hashes distinguish the implementations. Tool operations in the actionable
harness are serialized so verification snapshots and history cannot interleave
with concurrent writes.

## Preserved diagnostic matrices

| Before compatibility correction | Porta full pass | Docker Agent full pass |
|---|---:|---:|
| Per-check boolean feedback | 0 / 12 | 0 / 12 |
| Actionable feedback | 0 / 12 | 3 / 12 |

These are development results with a known verifier incompatibility, not a fair
quality comparison of the corrected system. The task grader and matched-input
audit pass, but they do not by themselves prove verifier compatibility.

- [Boolean-feedback raw report](2026-09-19-verification-checks-diagnostic.json.gz)
  and [hashes/summary](2026-09-19-verification-checks-diagnostic-summary.json).
- [Actionable-feedback raw report](2026-09-19-verification-feedback-diagnostic.json.gz)
  and [hashes/summary](2026-09-19-verification-feedback-diagnostic-summary.json).

## Scope

The model revision, runtime versions, local inference setup and remaining
limitations are the same as the baseline. This is one small quantized model and
four author-created tasks, not a general coding benchmark. A successful recorded
MCP verdict is historical evidence; external changes after that verdict require
an artifact-reading check or new independent verification. This comparison uses
fresh workspaces and does not test mutable external state during resume.

To reproduce the corrected matrix after building `verify-tool.wasm`:

```bash
/tmp/porta-model/bin/python scripts/evaluate_agents.py \
  --endpoint http://127.0.0.1:18764/v1/chat/completions \
  --model /tmp/porta-real-model --model-source /tmp/porta-real-model-source.json \
  --completion-check examples/verify-json/verify-tool.wasm \
  --verification-feedback actionable --repeats 3 \
  --output target/eval-actionable-fixed-model.json
python3 scripts/audit_eval.py target/eval-actionable-fixed-model.json
```

A subsequent [compact-result evaluation](compact-results-evaluation.md) measures
the updated WASM guest using the same tasks and budget, without replacing this
matrix.
