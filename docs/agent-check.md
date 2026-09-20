# Inspect a team before execution

```bash
porta agent-check agent.toml
```

This command loads the complete configured team and prints a JSON inspection
report. It checks configuration fields, paths, budgets, endpoint syntax, offline
argument schemas, module hashes, delegation cycles/depth, verifier mount rules,
WASM validity, allowed import modules, actual WASI import names and types,
and the `_start: () -> ()` export. All
local WASM modules are compiled, including modules in unused delegates.

No agent or tool is instantiated or run, including WASM module start sections.
The command does not contact a model or MCP server, resolve environment-based
credentials, create a run handle, or open a journal. It can run in a build or
review environment without API keys. A validation failure produces a nonzero
exit status and a diagnostic on stderr; success writes one JSON object to stdout.

```bash
porta agent-check examples/chat-agent/team.toml > team-inspection.json
```

The report has these top-level fields:

| Field | Meaning |
|---|---|
| `valid` | The configured tree passed load-time checks. |
| `fingerprint` | The same configuration/module/mount identity used by journals. |
| `team` | Root agent and recursive child inspection reports. |

Each agent report includes actual module/configuration digests, configured hash
pins, effective strict pinning, resolved tool and verifier mounts, both kinds of
checks and their parameters, local argument schemas, remote MCP tool mappings,
model/MCP endpoints, and credential environment-variable names. Credential values
are not read or included. Literal data in policy parameters and schemas is part
of the report, so handle it as configuration information when sharing it.

`limits.local` describes that agent's configured invocation limits;
`limits.root` describes the shared root step, model-call and time limits. A child
with larger local limits remains constrained by the root. The report distinguishes
these rather than suggesting that delegation creates a fresh root budget.

For identical files and resolved mount bindings, the report is deterministic and
independent of environment credential values. It includes neither elapsed time
nor credential availability. Changes to configuration bytes, modules or mount
bindings change the fingerprint.

## What validation establishes

A successful check establishes load-time validity. It does not test provider
availability, credentials, remote tool implementation, actual WASI instantiation,
model behavior or task correctness. A module whose start function traps can pass
this inspection: the command deliberately does not execute it. Argument schemas
and policy modules are loaded but their task-specific outcomes are not evaluated.

Checking does not lock files for a later run. Normal execution loads and validates
the configuration again. Use [strict artifact pins](artifact-pins.md) to require
reviewed module and delegated-policy content at launch. Keep the root policy
under operator control. Mounted data and external service state remain mutable.

Missing or incorrectly typed `_start` exports and unresolved or incompatible WASI
imports fail during normal team loading as well, before any model request,
including those in unused tools.
This is a load-time validation improvement; broker protocol remains 5.

```bash
python3 scripts/agent_check_integration.py target/porta examples/chat-agent/agent.wasm examples/demo-agent/agent.wasm
```

Tests inspect a team with trapping start sections and absent credentials, verify
that no model/MCP request occurs, compare output with credentials present, check
inherited pinning and budgets, and reject changed pins, external schema references,
missing/wrong entry points, nonexistent/type-incompatible WASI imports and malformed
CLI arguments. Correctly typed WASI imports still pass without instantiation.

The loader prepares and retains Wasmtime's checked import linkage without creating
an instance. Execution creates a fresh Store, WASI context, memory and instance
from that linkage each time; live guest state is not reused between invocations.
This avoids rebuilding the host-function registry at every step. No measured speed
improvement is claimed from this implementation change.
