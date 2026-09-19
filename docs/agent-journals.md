# Durable WASM agent runs

Porta can checkpoint an agent or team at a completed root turn and resume it in a
new process. Replay reruns the WASM decision loop against recorded broker results,
without invoking models or tools again. The same compiled guest protocol supports
both; guest code does not need a serialization or checkpoint library.

```bash
# Choose a private journal directory outside every tool's mounted directory.
mkdir -p .porta-runs

# Stop after two root turns, after making the completed operations durable.
porta agent agent.toml --record .porta-runs/work.jsonl --pause-after 2 -- "Complete the task"

# Inspect the stopped record without loading WASM or exposing the conversation.
porta agent-journal .porta-runs/work.jsonl

# Continue in a new process, preserving model-call, step, and active-time budgets.
porta agent-resume agent.toml .porta-runs/work.jsonl

# Recompute and verify a completed run with no model key or network required.
porta agent-replay agent.toml .porta-runs/work.jsonl
```

If a tool mounts the project root, put the journal in a separate private directory
outside the project. Porta rejects journals inside **any** tool mount, including
read-only mounts and mounts belonging to delegated agents.

`--pause-after` counts root turns. A delegated child finishes its current work
before that root turn completes. Omit it to record the full run without pausing.
Normal runs without `--record` do not persist conversations.

## What is durable

The journal records each external broker operation in two synced records:

1. An `intent` is written and synced before sending a model request or executing
   a WASM tool.
2. Its `result` is written and synced before feeding it to the next guest turn.

It includes the task, guest input and decision, model/tool response, actor path,
recorded tool fuel, and elapsed active time. Delegated agents share the same
ordered journal and root budgets. A checkpoint is a synced marker at a safe root
boundary. Completion records the final output.

On resume the broker recompiles the same modules, reconstructs the guest's
continuations from the beginning, and returns recorded results for the completed
prefix. It only performs new operations after that prefix. Consequently a file
changed by the user after a completed write stays changed: resume does not repeat
that write. Past model calls still count against the run's budget.

Paused wall time is excluded. Previous active time, restart/reconstruction
cost, and new work are charged against the configured deadline. Replay of a
completed run gets a fresh verification-time deadline but still checks the
original configured step/model-call budgets.

## Integrity and isolation

- New journals use exclusive creation and owner-only permissions (`0600` on
  Unix). Existing files are never overwritten. Symlink journal files are refused
  on Unix. An exclusive file lock prevents concurrent resume/replay writers.
- A SHA-256 chain detects corrupted, reordered, or accidentally edited records.
  Config source bytes, all transitive agent/tool WASM bytes, and resolved mount
  bindings contribute to the configuration fingerprint. Changed artifacts or
  policies are rejected before issuing any new external operation.
- Replay executes the actual WASM guest and compares its decisions and input
  events with the journal, rather than merely printing the recorded answer.
  It rejects an unexpected operation, divergence, unused records, or a different
  final answer. It never writes to the journal.
- Credentials used in HTTP authorization headers are not recorded or passed to
  WASM. Completed replay does not require them. Requests, responses, prompts,
  and tool output **are** recorded; these files contain conversation data.
- The journal is limited to 64 MiB. Partial records, invalid UTF-8/JSON, broken
  chains, and ambiguous operation outcomes fail closed.

The hash chain is corruption detection, not a digital signature or authentication
against someone who can rewrite the entire journal. Replay verifies guest
behavior relative to recorded inputs; it does not prove that an external service
really produced those inputs or that historical filesystem effects occurred.

## Interrupted operations

If Porta dies after an intent but before its result is durable, it cannot know
whether the external request or tool write succeeded. `agent-resume` reports:

```text
operation outcome is uncertain and will not be repeated
```

This is deliberate. An ordinary filesystem write or remote API does not provide
an exactly-once transaction spanning the journal. Inspect and reconcile the
external effect before starting a separately authorized run. There is currently
no automatic retry or journal-repair override for such a record.

A process killed after a complete result record can resume that prefix. A missing
credential detected before sending a new request does not create a pending
intent, so credentials can be supplied and the same journal resumed later.

## Boundaries

This is continuation replay at agent/broker boundaries, not a snapshot of linear
memory, the WASM stack, mounted files, or the host process. It currently requires
the same configuration, WASM artifacts, and accessible declared mount directories
for reconstruction. Editing even a comment in a config changes its fingerprint.

A guest that consults WASI clocks or randomness may produce a different decision
when reconstructed. Such divergence is rejected; there is no claim of universal
instruction-level deterministic replay. The included chat agent derives its
continuation solely from the input event and is covered by the replay suite.

Filesystem durability and lock behavior depend on the underlying filesystem.
The process-level crash tests have been verified locally on macOS. CI is
configured to run them on macOS and Linux; power loss and arbitrary distributed
filesystems are not certified by these tests.

## Verification

```bash
python3 scripts/journal_integration.py target/porta examples/chat-agent/agent.wasm examples/demo-agent/agent.wasm
```

The suite executes real WASM and real writes against a local model server. It
covers checkpoint/resume, offline replay without credentials, every durable
result prefix, semantic divergence despite a valid hash chain, changed artifacts,
permission and mount checks, delegated teams, budget preservation, pause/active
time, concurrent access, and SIGKILL during an in-flight request. These tests are
separate from real-model quality evaluation.

## Inspecting stopped records

`porta agent-journal <journal.jsonl>` reads a private journal and returns JSON
without loading an agent configuration or WASM, resolving credentials, making
network requests, replaying decisions, resuming operations or modifying the file.
This also works when the original executable artifacts are unavailable.

| `status` | What the durable record establishes |
|---|---|
| `complete` | A final completion record exists. |
| `checkpoint` | The last record is a checkpoint with no pending intent. |
| `incomplete` | A header/result prefix ends without a checkpoint or completion. |
| `uncertain` | An intent exists without a durable result; its effect may have happened. |

The report includes `fingerprint`, byte size, `recorded_elapsed_ms`, the count of
completed operations, and counts grouped by operation kind and actor. Pending
operation metadata contains its index, actor, kind and tool/check name when
available. It excludes the task, final answer, conversation, tool arguments,
results and endpoint details. The pending operation is not included in completed
counts. Recorded elapsed time comes from the latest durable timing record; time
spent in an interrupted operation may be absent.

Inspection verifies record ordering and the SHA-256 chain. It always reports
`replay_verified: false`: checksums do not authenticate the author or establish
that recorded decisions match the actual WASM. A modified record with a recomputed
hash chain can pass inspection and still fail `agent-replay`. Use replay with the
original configuration and artifacts for that stronger check. A `complete`
status alone also does not establish task correctness.

The command uses a read-only descriptor and a nonblocking exclusive lock. A live
writer causes an `already in use` error; this is inspection of stopped runs,
not a live log tail. Regular-file, no-symlink, owner-only permission and 64 MiB
bounds remain enforced. Read-only owner permissions (`0400`) are supported.
Truncated, corrupted, invalidly ordered or overly permissive records are rejected
rather than summarized as safe to resume.

Inspection of `uncertain` records does not clear the pending intent. Resume and
replay still refuse it. Likewise, a `checkpoint` or `incomplete` summary is not a
promise that resume will succeed: configuration identity, code reconstruction,
remaining budgets and current policy checks still apply. No automatic retry or
reconciliation is performed by inspection.

The integration suite covers all four states, read-only files, missing artifacts,
shared-team counts, metadata-only output, corruption and permission failures,
active-writer locking, killed model requests and killed remote MCP effects. It
also checks that validly rehashed semantic divergence remains distinguishable
from successful WASM replay.
