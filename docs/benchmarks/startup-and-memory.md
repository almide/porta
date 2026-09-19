# Startup and command RSS, 2026-09-20

The current development build completes a fixed one-model-call task with lower
measured startup overhead than the installed Docker Agent in this local fixture.
This is not a real-model, tool-throughput, isolation or general quality benchmark.
The [real-task results](pre-tool-policy-evaluation.md) remain separate.

| Persistence configuration | Porta median | Docker Agent native median |
|---|---:|---:|
| Porta without journal; Docker Agent fresh session database | 18.01 ms | 235.00 ms |
| Porta durable journal; Docker Agent fresh session database | 36.66 ms | 238.30 ms |

Each row uses 15 fresh-process latency trials per runtime, after one warmup each,
with runtime order alternated. Every trial exits successfully, returns exactly
`benchmark-ok`, and makes exactly one loopback model request. The model fixture
returns a fixed answer; there is no inference or paid API use.

Porta's recorded configuration persists a header, model intent, model result and
completion with the broker's normal sync behavior. Those actual records and their
hash chains are preserved and independently audited. Docker Agent uses a fresh
session database in both rows. This reduces one configuration mismatch, but does
not make the two persistence implementations or their durability contracts equal.

## Separately measured memory

Memory trials run separately under macOS `/usr/bin/time -l`, seven fresh invocations
per runtime for each row. Their wrapper-inclusive durations are excluded from the
latency samples above.

| Persistence configuration | Porta median reported maximum RSS | Docker Agent median reported maximum RSS |
|---|---:|---:|
| Porta without journal | 32.20 MiB | 71.47 MiB |
| Porta with journal | 32.19 MiB | 71.52 MiB |

The measured host's `getrusage(2)` manual defines `ru_maxrss` in bytes. The reports
retain the original byte counts and complete `time -l` output. This is the OS
resource-usage metric reported for the command invocation, not a sampled sum of
all concurrently resident processes, a memory limit, or the peak of a production
agent workload. The local model-fixture server runs outside both measured commands.
Docker Agent is invoked through `docker agent`; its launcher/plugin process path
is part of the invocation. No container, VM or Docker sandbox is measured.

## Evidence and limits

- [Unrecorded raw report](2026-09-20-startup.json.gz),
  [summary and hashes](2026-09-20-startup-summary.json),
  [exact harness source](2026-09-20-startup-harness.py.gz).
- [Recorded raw report](2026-09-20-startup-recorded.json.gz),
  [summary and hashes](2026-09-20-startup-recorded-summary.json),
  [exact harness source](2026-09-20-startup-recorded-harness.py.gz).

The archives contain every warmup, latency and memory trial, model request body,
process output, answer check, and memory measurement. The recorded variant also
contains the actual durable journals. The corresponding harness source snapshots
match the SHA-256 values captured by each run.

Platform: macOS 26.3, arm64. Docker Agent: installed v1.98.0, native `--exec --json`,
without `--sandbox`. Porta: uncommitted 0.4.0 development build, identified by its
binary hash. Filesystem caches are warm. Each invocation creates fresh WASM stores
or a fresh Docker Agent session database.

Docker Agent receives SSE and usage events; Porta receives a single JSON response.
The same initial system/user text and final answer are used, but request shapes,
streaming and output formatting differ. Porta sends its normal output-token limit;
Docker's request omits one in this fixture. The response is fixed independently of
these inference parameters. No tools or concurrent teams run here.

The earlier 16.59 ms Porta startup measurement used a different host and guest
artifact and fewer samples. These runs do not isolate the effect of retaining
checked WASI linkage, or establish a regression/improvement attributable to a
single change. More platforms, workloads and repeated measurement sessions are
needed for general performance claims.

## Reproduce and audit

Build Porta and its chat guest, install the Docker Agent version under test, then:

```bash
python3 scripts/benchmark_agents.py --runs 15 --memory-runs 7 --output target/startup.json
python3 scripts/benchmark_agents.py --runs 15 --memory-runs 7 --porta-record --output target/startup-recorded.json
python3 scripts/audit_benchmark.py target/startup.json
python3 scripts/audit_benchmark.py target/startup-recorded.json --porta-record
```

`--memory-runs` currently requires macOS. Latency-only runs can omit it; pass
`--memory-runs 0` to the audit for those reports. The harness disables telemetry,
automatic installation and inherited HTTP proxies, bounds process execution,
terminates timed-out process groups, and refuses to summarize a failed run as a
successful timing sample. A failed answer/exit/call-count check or timeout writes
a separate `.failed.json` diagnostic. Resource parsing failures abort measurement.

The audit verifies the full expected matrix, exact answers, model-call counts,
matched prompts, separate phases, raw RSS values, calculated medians and recorded
journal integrity. Negative checks reject missing trials, misleading output that
merely mentions the answer, and changed RSS values. CI audits the saved evidence
without launching either runtime or downloading a model.
