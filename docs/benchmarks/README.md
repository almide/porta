# Agent runtime measurements

The [current startup and memory measurements](startup-and-memory.md) include
separate trials with Porta durable journaling enabled and retain complete evidence.
Keep these fixed-response results separate from [real task quality](loop-notice-experiment.md).
The latest follow-up reruns the current guest and rejects a repeated-tool notice
experiment. Historical policy gains in this small suite do not establish general
quality superiority or guarantee the same score on subsequent runs.

The separate [WASM compute capability smoke](compute-smoke.md) records two correct
tasks and one failed transformation with the new optional tool. It has no
competitor arm and does not replace the matched quality measurements.

The [shared WASM compute evaluation](shared-compute-evaluation.md) gives both
runtimes that same optional tool across the four matched tasks. No trial passed
every requirement, and all 115 compute calls failed inside the model-authored
script rather than in transport or the runtime. The expanded toolset and
instruction are not published as a quality improvement; the MCP
result-preservation fix and the removal of implicit WASM directory grants are
retained on their own test evidence.

## 2026-09-19: one local model call

Fresh process to final answer, seven measured runs per runtime after one warmup,
alternating runtime order. Both receive the same prompt and deterministic local
model answer. Each successful run must make exactly one model request. There are
no tools, real inference, cloud API calls, or external model credentials.

| Runtime | Median | Samples |
|---|---:|---:|
| Porta 0.4.0 development tree, WASM agent | 16.59 ms | 7 |
| Docker Agent v1.98.0, native `--exec --json` | 246.01 ms | 7 |

Porta completed this specific fixture about 14.8 times faster. This is evidence
about process startup and one-call orchestration overhead, not a claim that Porta
is generally faster or better at agent tasks.

Environment: macOS 26.3, arm64. Porta guest: 31,167 bytes. Porta host runtime:
34,339,016 bytes. Binary hashes, all samples, versions, and model request modes
are in [the raw report](2026-09-19-local-model.json).

### Important comparison limits

- Docker Agent runs natively, without `--sandbox`; no Docker container or VM
  startup is included. Porta's decision loop is in WASM.
- Docker Agent uses SSE streaming and writes a fresh session database; Porta uses
  a non-streaming response and did not persist a session in this historical run.
  The current runtime has optional durable journals. Both get equivalent
  model content, but the feature paths are not identical.
- This measures the locally installed Docker Agent v1.98.0, not the latest release.
  The Porta binary is an uncommitted development build; use its SHA-256 identity.
- Filesystem caches are warm. Every measured invocation is a fresh process, with
  a fresh WASM store or fresh Docker Agent session database.
- This does not measure answer quality, real-model performance, memory, tool
  throughput, parallel teams, Linux behavior, or equivalent isolation modes.
- The fixture sends streaming usage events requested by Docker Agent. Earlier
  malformed-fixture attempts did not terminate and were excluded as invalid;
  they are not evidence of a competitor performance or correctness defect.

### Reproduce

Build the Porta binary and chat-agent artifact as in `docs/agent-runtime.md`,
install Docker Agent, then run:

```bash
python3 scripts/benchmark_agents.py --runs 7 --output target/agent-benchmark.json
```

The harness validates completion, counts local model calls, kills the process
group on timeout, disables Docker Agent telemetry, and aborts on failed runs. It
does not turn failures into favorable timing samples. The matched real-task suite is recorded separately above; broader models and
public tasks remain necessary.
