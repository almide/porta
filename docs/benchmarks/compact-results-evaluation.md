# Compact MCP results in the WASM agent

This measurement preserves the four tasks, grading, model revision, eight-call
budget and shared verification tool from the [previous corrected matrix](verified-model-evaluation.md).
Only Porta's guest artifact changes: it includes bounded empty-response recovery
and compacts unambiguous MCP text results before sending them to the model.
Docker Agent, the host broker, verifier, harness and feedback remain unchanged;
the archived reports record their hashes. These two guest changes are measured
together, not as a controlled attribution to either change alone.

The original MCP result remains in the host journal and completion-check history.
The display path retains envelopes for errors, conflicting structured data,
multiple blocks, images and annotated or malformed text. Official SDK tests cover
JSON/SSE, stateful/stateless sessions, real writes, resume and offline replay.

## Results

Full task success remains **3/12 for each runtime**, with correct artifacts in
6/12 runs each when procedure requirements are ignored. Porta's nine failures now
all exhaust the model budget; no run stops on an empty response. This does not
prove empty-response recovery caused a quality improvement.

| Task | Porta pass | Docker pass | Porta calls (median) | Previous Porta | Docker calls (median) |
|---|---:|---:|---:|---:|---:|
| invoice_totals | 0/3 | 0/3 | 8 | 8 | 8 |
| config_migration | 0/3 | 0/3 | 8 | 8 | 8 |
| mean_empty_fix | 0/3 | 0/3 | 8 | 8 | 8 |
| release_lookup | 3/3 | 3/3 | 4 | 8 | 6 |

The release task passes on all three repetitions with four model calls in Porta,
versus eight in its prior matrix and six in Docker Agent. Tools still execute
sequentially: model responses can request multiple tools together. This is a
specific observed efficiency improvement, not a general quality or speed win.

| First tool result | Previous bytes | Current bytes |
|---|---:|---:|
| invoice_totals | 351 | 124 |
| config_migration | 391 | 130 |
| mean_empty_fix | 203 | 55 |
| release_lookup | 489 | 169 |

These sizes are identical across the three repetitions for each task. They cover
only the first tool message's UTF-8 content.

- [All 24 runs, including failures](2026-09-19-compact-results.json.gz)
- [Hashes, scorecard and comparison metrics](2026-09-19-compact-results-summary.json)

```bash
python3 scripts/audit_eval.py docs/benchmarks/2026-09-19-compact-results.json.gz
```

## Interpretation

This remains one small quantized local model and four author-created tasks.
Neither a smaller request nor fewer model calls proves broader task quality or
isolation superiority. Full failures remain in the report. Process duration is
recorded but is not used as a general speed claim. The published earlier matrices
remain available and are not replaced.

Input display size measures UTF-8 bytes in the first returned tool message for
each task, not the entire prompt and not billed tokens. Those reads return the
same fixed input files. Later conversation trajectories may differ.

## Reproduce

Build the current Almide 0.63.0 chat guest, start the pinned model server using the
[baseline instructions](real-model-evaluation.md), then run:

```bash
/tmp/porta-model/bin/python scripts/evaluate_agents.py \
  --endpoint http://127.0.0.1:18764/v1/chat/completions \
  --model /tmp/porta-real-model --model-source /tmp/porta-real-model-source.json \
  --completion-check examples/verify-json/verify-tool.wasm \
  --verification-feedback actionable --repeats 3 \
  --output target/eval-render-model.json
python3 scripts/audit_eval.py target/eval-render-model.json
```

The next [pre-tool policy matrix](pre-tool-policy-evaluation.md) measures explicit
prerequisite enforcement with the same tasks and model-call budget.
