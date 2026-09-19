<!-- description: Guest-driven WASM agents and teams with shared execution budgets -->
# WASM Agent Runtime

The product objective is to outperform Docker Agent as an agent runtime using
WASM execution, not merely offer a native command wrapper.

Implemented: `porta agent`, the Almide chat agent, WASM tools, host-only model
credentials, scoped filesystem grants, isolated delegation, shared team budgets,
and process-level adversarial integration tests. Broker-boundary checkpoint/resume
and offline decision replay are also implemented. See `docs/agent-runtime.md` and
`docs/agent-journals.md`. Tool arguments are validated against JSON Schema before
WASM execution, with repairable errors and no external schema retrieval.
Explicit MCP HTTP tools support JSON/SSE, bounded transport, scoped credentials
and recorded replay; see `docs/agent-mcp.md`.

Required before declaring competitive completion:

- Improve real-model task completion against the same Docker Agent workload.
  The first matched suite found 0/12 full-task passes on each side; see
  `docs/benchmarks/real-model-evaluation.md`. Verifiable completion is implemented
  (`docs/completion-checks.md`); measure improvements and broaden models/tasks
  without hiding the initial failures.
- Broaden the delivered checkpoint/replay feature with inspection, cancellation,
  and authenticated distribution; retain the uncertain-operation safety rule.
- Model adapters, broader MCP features, model streaming.
- Immutable distribution and signatures.
- Measured startup, latency, memory and completion-rate improvements.
- Cross-platform CI and concurrency/cleanup evidence.

Keep the goal active until these have evidence. A mock model test proves protocol
and execution behavior, not real-model quality or superiority over Docker Agent.
