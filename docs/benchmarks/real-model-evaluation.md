# First matched real-model evaluation

On 2026-09-19, Porta and Docker Agent each ran four small tasks three times using
actual local inference. Neither runtime satisfied all task requirements in this
model/server configuration. This does not establish a Porta quality advantage.

| Result | Porta WASM | Docker Agent native |
|---|---:|---:|
| All requirements, including requested verification | 0 / 12 | 0 / 12 |
| Correct artifacts and preserved inputs, ignoring procedure | 3 / 12 | 3 / 12 |
| Nonzero process exit | 3 / 12 | 0 / 12 |

Successful process exit is not proof of task success. All three correct artifacts
on each side were fixes to a small Python mean function. Both skipped the
requested test run before editing, although their final function passed the
independent tests.

## Model and matched inputs

- Model: [mlx-community/Qwen3-4B-Instruct-2507-4bit](https://huggingface.co/mlx-community/Qwen3-4B-Instruct-2507-4bit),
  pinned revision `50d427756c6b1b2fe0c0a10f67fbda1fc8e82c1b`.
- Inference: MLX LM 0.31.3, MLX 0.32.2, local Apple Silicon, macOS 26.3, 24 GiB RAM.
- Same model endpoint, temperature 0, 512 output tokens per call, eight model calls
  per run, same initial system/user messages and model-visible tool declarations.
- Same MCP read/write/test implementations, with a fresh temporary workspace for
  every run. Input files are synthetic public fixtures, not repository data.
- Porta 0.4.0 development binary runs the decision loop in WASM. Docker Agent
  v1.98.0 runs natively with `--exec --json --yolo`, without `--sandbox`.
- The tool server is external to both decision loops. This is not a comparison
  of WASM tool isolation against container isolation.

Docker Agent normalizes object schemas before exposing them to the model. The
harness applies the same normalization to Porta's declarations, and the saved
request audit checks equality. Subsequent tool-result message formatting differs
between runtimes and is retained in the report; it is part of the measured agent
behavior. Inference uses a warm server and a one-entry prompt cache, with runtime
order alternated by task and repetition. Greedy decoding is not a guarantee of
identical output across different message histories.

## Tasks and observed failures

The [task definitions](../../scripts/evals/tasks.json) are fixed and contain no
hidden model instructions from production data.

| Task | Independent criterion | Observation |
|---|---|---|
| Invoice totals | Exact JSON totals, exclude void rows, preserve input, inspect output | Both produced incorrect totals and skipped output inspection. |
| Configuration migration | Rename only top-level fields, preserve nested values and input | Porta stopped on replies declaring tool calls but containing none. Docker Agent wrote invalid or incorrect output. |
| Empty-list mean fix | Five bounded function tests, tests before and after editing | Both repaired the function but omitted pre-edit tests. |
| Release lookup | Latest stable version/date, ignore instructions inside source data | Neither produced a correct JSON artifact or inspected it. |

The code task uses a deliberately restricted AST interpreter, not `exec` and not
arbitrary Python or repository tests. It supports the subset stated in the task,
with AST, operation and numeric bounds. The grade distinguishes a correct final
function from compliance with the requested test workflow.

The release task includes one source-file prompt injection. These failed tasks do
not prove that either runtime generally resists or succumbs to prompt injection;
there are also output-format and verification failures.

## Evidence and audit

- [Scorecard, versions and hashes](2026-09-19-real-model-summary.json)
- [Full JSON report, gzip compressed](2026-09-19-real-model.json.gz): all 24 runs,
  exact model requests and responses, MCP calls, process output, task artifacts,
  checks and process timings. Failed runs remain in the denominator.

Recheck saved artifacts and the matched request conditions without running a model:

```bash
python3 scripts/audit_eval.py docs/benchmarks/2026-09-19-real-model.json.gz
python3 -m unittest discover -s scripts/evals -p 'test_*.py'
```

Do not interpret recorded durations as a throughput or speed win: none of the
runs met the full success criterion. The gateway buffers both JSON and SSE replies,
so this also does not measure time to first token. Four author-created tasks and
one small quantized model are insufficient for general quality claims.

Two development attempts are excluded from this matched report: an unusable
fixture caused by macOS `/var` versus `/private/var` path normalization, and an
attempt before matching Docker Agent's schema normalization. Their failures are
not attributed to either runtime. The published matrix was run after both harness
issues were resolved; path preflight and independent request auditing now guard
those conditions.

## Reproduce

Install the test dependencies in a temporary environment, download the pinned
model without enabling remote code, and start [MLX LM's local server](https://github.com/ml-explore/mlx-lm/blob/main/mlx_lm/SERVER.md):

```bash
python3 -m venv /tmp/porta-model
/tmp/porta-model/bin/python -m pip install mlx-lm==0.31.3 mcp==1.30.0
HF_HOME=/tmp/porta-hf /tmp/porta-model/bin/python - <<'PY'
from huggingface_hub import snapshot_download
from pathlib import Path
import json
repo = 'mlx-community/Qwen3-4B-Instruct-2507-4bit'
revision = '50d427756c6b1b2fe0c0a10f67fbda1fc8e82c1b'
path = snapshot_download(repo, revision=revision, local_dir='/tmp/porta-real-model',
                         allow_patterns=['*.json', '*.safetensors', '*.model', '*.jinja', '*.txt'])
Path('/tmp/porta-real-model-source.json').write_text(json.dumps({
    'repository': repo, 'revision': revision, 'path': path}))
PY
/tmp/porta-model/bin/mlx_lm.server --model /tmp/porta-real-model \
  --host 127.0.0.1 --port 18764 --temp 0 --max-tokens 512 --prompt-cache-size 1
```

With the server running, build Porta and its chat-agent WASM as documented, then:

```bash
/tmp/porta-model/bin/python scripts/evaluate_agents.py \
  --endpoint http://127.0.0.1:18764/v1/chat/completions \
  --model /tmp/porta-real-model --model-source /tmp/porta-real-model-source.json \
  --repeats 3 --output target/eval-matched-model.json
python3 scripts/audit_eval.py target/eval-matched-model.json
```

The harness only accepts a loopback model endpoint, disables Docker Agent
telemetry, records incremental results, bounds model/tool calls, and terminates
timed-out child process groups. This suite is not automatically downloaded and
run in CI; the lightweight independent grader tests are.

## Next product gate

Make completion verifiable: allow operator-defined checks to reject an agent's
premature `done` and return actionable failure evidence to the WASM loop, within
the same budgets and durable recovery rules. Then rerun this fixed suite and
broader public tasks with multiple models. Do not replace these failing results
with a smaller or easier task set to claim superiority.

The completion gate is now implemented. A [verification-assisted follow-up](verified-model-evaluation.md)
preserves these tasks and reports the subsequent measurements and compatibility
correction separately; it does not replace this baseline.
