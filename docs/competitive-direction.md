# Porta direction

Reviewed 2026-09-19 against [Docker Agent documentation](https://docs.docker.com/ai/docker-agent/).
Docker Agent provides declarative multi-agent teams, model/provider selection,
MCP tools, and OCI distribution. Porta now runs guest-driven WASM agents and delegated teams with per-tool
filesystem grants, host-held model credentials, and shared team budgets.
Docker Agent also runs natively; Docker sandbox isolation is optional. Comparisons
must state the isolation mode rather than assume every run uses a container.
It also has a [browser WASM agent loop](https://github.com/docker/docker-agent/blob/v1.98.0/cmd/wasm/runtime_wasm.go).
WASM alone is therefore not a defensible differentiation claim: Porta must prove
its per-agent/per-tool isolation, host-held credentials, portability, and measured
execution benefits.

The proposed advantage is a small, auditable execution layer that works with
existing agents, requires no Docker daemon for WASM, and makes policy enforcement
testable. This is a product direction, not a claim of measured superiority.

## Delivered foundation

- Almide 0.63.0 API migration; pinned compiler artifact and checksum verification.
- Shared native sandbox policy across `run`, `up`, and MCP command execution.
- Default native write denial outside mounts, temporary storage and devices.
- Loopback-only CONNECT proxy egress and consistent declarative configuration.
- Removal of agent-adjacent executable-code cache deserialization and unchecked
  direct WASM-to-host execution/HTTP imports.
- MCP newline transport, Unicode and long-session process regression tests.
- HTTP allow lists use the transport URL parser; redirects and inherited HTTP
  proxies are disabled so requests cannot silently change the checked destination.
- macOS/Linux CI build and test matrix; unsupported native execution fails closed.

## WASM product delivered

- A compiled Almide guest controls model/tool decisions and conversation state.
- The broker executes only declared tools and model endpoints. Credentials are
  not placed in WASI environments or guest input.
- Tools receive separate read-only or read-write directory grants.
- The optional [compute tool](../examples/compute/README.md) executes bounded
  Rhai scripts inside WASM, with decimal arithmetic and exact JSON text transport.
  It exposes no file/network/environment/clock APIs; writing stays a separate grant.
- Parent agents can delegate tasks to separately configured WASM agents; the
  complete team shares root model/step/deadline budgets.
- Real WASM integration tests cover task completion and adversarial isolation.
- Operator-owned read-only WASM completion checks reject premature completion,
  return corrective feedback, and recheck current artifacts after interrupted passes.
- Explicit remote MCP tool grants with JSON/SSE HTTP, host-only credentials,
  bounded responses and durable replay; official Python SDK interoperability.
- Host-side JSON Schema validation rejects invalid tool arguments before effects,
  returns repairable errors, and disables external schema retrieval.
- Durable broker journals support restart/resume and offline guest-decision
  verification, including delegated teams and shared budgets. Uncertain external
  operations are never silently repeated.

The [current local one-call measurement](benchmarks/startup-and-memory.md) records
36.66 ms for Porta with durable journaling and 238.30 ms for installed Docker Agent
v1.98.0 native execution with a fresh session database. Separate OS-reported maximum
RSS medians are 32.19 MiB and 71.52 MiB. The report preserves feature-path limits;
these are fixed-response startup measurements, not real-task quality results.

## Next acceptance gates

1. **Real task evaluations:** the [first matched local-model suite](benchmarks/real-model-evaluation.md)
   recorded 0/12 full-task passes for both runtimes (3/12 correct artifacts each).
   The [verification-assisted follow-up](benchmarks/verified-model-evaluation.md)
   records 3/12 full passes and 6/12 correct artifacts for each runtime.
   [Compact MCP results](benchmarks/compact-results-evaluation.md) retain those
   scores while reducing Porta release-task model calls from eight to four
   (Docker Agent: six). The [explicit pre-tool policy comparison](benchmarks/pre-tool-policy-evaluation.md)
   improves full passes to 6/12 against Docker Agent instruction-only 3/12,
   with unchanged artifact accuracy (6/12 each). This is policy enforcement
   evidence, not proof that equivalent competitor configuration is impossible.
   The [current-guest rerun and rejected loop-notice experiment](benchmarks/loop-notice-experiment.md)
   record 4/12 for the current Porta guest and 3/12 for Docker Agent, with artifact
   passes of 6/12 and 7/12. The candidate reaches only 3/12 Porta full passes and
   is not adopted. Historical 6/12 is not the current result. Offering both
   runtimes the [shared WASM compute tool](benchmarks/shared-compute-evaluation.md)
   records 0/12 full passes on each side; every compute call failed in
   model-authored script, none in transport or the runtime, so that toolset is
   also not adopted. Broader public tasks across multiple models remain necessary
   before claiming quality superiority.
2. **Persistent execution:** broker-boundary checkpoint/resume and offline replay
   are implemented; stopped-journal inspection reports uncertain operations without
   effects. Cancellation controls, signed transcripts,
   and instruction-level clock/random replay remain.
3. **Tool ecosystem:** broader MCP interoperability, model adapters,
   bounded streaming, and richer WASM tools. Keep the reasoning loop in WASM.
4. **Distribution:** [SHA-256 pins](artifact-pins.md) now bind executable artifacts
   and delegated policies, with strict inheritance. Publisher signatures, trust
   and revocation, portable bundles and a content-addressed registry remain.
5. **Efficiency:** cold start, steady-state latency, RSS, and artifact sizes on
   identical workloads. Publish raw measurements before claiming superiority.
6. **Scale:** concurrent teams with independent budgets, reliable cleanup, and
   repeated adversarial testing on Linux/macOS.

Native Linux command restrictions are secondary interoperability work. They are
not the foundation of Porta's agent runtime or a prerequisite for WASM teams.

Native macOS mode currently permits broad filesystem reads and inherited
variables, and uses the deprecated `sandbox-exec` facility. It is not a complete
container or VM boundary. WASM capability/import coverage and host resources also
need ongoing adversarial testing. Do not present the roadmap as shipped features.

MCP transport contract: [official stdio specification](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports).
