# WASM computation capability smoke

The optional [compute tool](../../examples/compute/README.md) was tested on
2026-09-20 with the existing pinned local Qwen3-4B model. These are three
single-trial capability checks, not the four-task comparative benchmark and not
an improvement claim against Docker Agent. The prompts, tools and instruction
differ from that benchmark. There is no competitor arm and no completion gate
in this small smoke; a zero exit alone is not a passing task.

| Task | Correct tool result and final answer | Model calls | Compute calls | Tool errors before final computation |
|---|---|---:|---:|---:|
| Conditional decimal sum | Yes, `3.80` | 6 | 5 | 4 |
| Rename only a top-level key | No, unwanted `old_name: 0` remains | 4 | 3 | 2 |
| Increment integer above 2^53 | Yes, `9007199254740994` | 2 | 1 | 0 |

The decimal trial first produced incorrectly escaped JSON, then used an
undeclared variable three times before supplying a valid script. The transformation
trial recovered from syntax errors but chose an incorrect transformation. The
interpreter correctly executed that incorrect script; the model accepted its
result. This demonstrates why operator-owned completion checks remain necessary
even when computation is delegated to a tool. It also shows that Rhai syntax and
nested JSON escaping can consume substantial model-call budget with this model.

Input and output JSON travel as strings through the broker. The integer trial
checks that those digits survive the actual model → WASM → model path, rather
than only a native unit test. Separately, fixture integration tests verify correct
nested renaming, decimal aggregation, prohibited APIs, interpreter limits, host
fuel interruption and checkpoint/replay into a separately granted writer.

All three final journals replayed successfully after the inference server was
stopped, including the semantically incorrect transformation. Replay verifies
recorded execution consistency; it does not turn a wrong answer into a correct
one. The standard chat guest and host binary were unchanged by the compute tool.

- [All final prompts, model/tool journals, errors, configuration, hashes and source/lock snapshots](2026-09-20-compute-smoke.json.gz)
- [Earlier development probe](2026-09-20-compute-development.jsonl.gz): one decimal aggregation run before formatting and strict exponent-input handling; it also needed six model calls and returned `3.80`. It is retained separately, not counted as a final-artifact trial.

The saved configuration uses 8 model calls, 32 steps, 512 output tokens per model
call, temperature zero, 50 million fuel per WASM execution, 64 MiB WASM memory
and a 120-second deadline. The engine is Rhai 1.26.0 with rust_decimal 1.43.0,
locked in the archived Cargo.lock; the chat guest uses Almide 0.63.0. The exact
model repository/revision and host/guest/tool hashes are in the raw report.

```bash
python3 scripts/audit_compute_smoke.py docs/benchmarks/2026-09-20-compute-smoke.json.gz
```

The offline audit checks the complete three-task set, source hashes, journal
hash chains, operation pairing and final-output consistency. It independently
grades both the last compute result and final answer with decimal-aware JSON
parsing, retaining the failed transformation. Hash chains are not signatures;
this audit is not a substitute for native guest replay. CI runs the audit without
downloading or contacting a model.

For reproduction, build the tool and guest as described in the compute README,
start the [pinned local inference server](real-model-evaluation.md), and configure
`examples/chat-agent/compute.toml` with the saved endpoint/model and limits.
The three exact prompts are in the report's `runs[].prompt`; record each with
`porta agent CONFIG --record JOURNAL -- PROMPT`. Use a fresh journal per run.
Keep all errors and final results. The next comparative evaluation must give both
runtimes the same expanded tools and retain the original tasks and graders.
