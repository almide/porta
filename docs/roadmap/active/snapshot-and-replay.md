<!-- description: Instance snapshot, suspend/resume, and deterministic execution replay -->

# Snapshot & Replay

Leverage WASM's deterministic execution model for state capture and reproduction. These capabilities are difficult or impossible to achieve reliably with OS-level containers — this is where porta's architecture pays off most clearly.

## Snapshot / Checkpoint

Capture full instance state at a point in time:

- `porta snapshot <instance>` — Create snapshot of running instance
- `porta restore <snapshot>` — Restore instance from snapshot
- `porta clone <instance>` — Instant clone from current state

### Use Cases

- **Warm start** — Pre-initialize (load config, connect, warm caches), snapshot, then restore for near-instant startup
- **Suspend / resume** — Pause a long-running agent, free resources, resume later with full state
- **Fork** — Clone a running instance to handle parallel workloads from the same state
- **Pre-initialized pool** — Maintain snapshots for instant instance creation at scale

### What is Captured

- Linear memory contents
- Global variable state
- Call stack
- Table entries
- Mount bindings (re-attached on restore; file contents are not snapshotted)

## Deterministic Replay

Record and replay execution for debugging and verification:

- `porta run --record <module>` — Record execution trace
- `porta replay <trace>` — Replay recorded execution
- `porta replay --diff <trace1> <trace2>` — Compare outputs of two executions

### Recording

Captures all external inputs:

- stdin, file reads, network responses
- Timestamps, random values
- Entry point and arguments
- Environment and configuration

Does not capture internal computation — that is deterministic from inputs.

### Replay

- Feed recorded inputs → produces identical execution
- Divergence detection: flag any point where replay output differs from recording
- Step-through mode for debugging

### Use Cases

- **Bug reproduction** — Share a trace file, reproduce the exact execution
- **Regression testing** — Record a golden run, replay after changes, diff outputs
- **Audit** — Prove what an agent did, with what inputs, producing what outputs

## Module

- `snapshot.almd` — Memory serialization, state capture and restore
- `replay.almd` — Input recording, deterministic replay engine
