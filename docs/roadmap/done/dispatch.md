<!-- description: Instance lifecycle, state machine, and tool call dispatch -->
<!-- done: 2026-04-07 -->

# Instance Lifecycle & Dispatch

Manages the full lifecycle of WASM instances. Bridges MCP tool calls to WASM execution in serve mode, and manages standalone execution in run mode.

## Conceptual Model

A porta instance is an isolated WASM execution context with:

- Capability set (what it can access)
- Mounts (filesystem bindings)
- Network bindings (ports, egress rules)
- Configuration (env, secrets)
- Lifecycle state

Docker container = isolated process group + fs/net/config.
porta instance = isolated WASM execution context + capabilities + mounts + network bindings.

The isolation boundary is the WASM execution model, not OS kernel namespaces.

## State Machine

```
created → starting → running ──→ exited
                  ↓       ↕            
              failed   healthy / unhealthy
```

- **created** — Instance allocated, not yet started
- **starting** — Initialization in progress (startup probe window)
- **running** — Executing normally
- **healthy** — Running and passing health probes
- **unhealthy** — Running but failing health probes
- **exited** — Terminated with exit code
- **failed** — Terminated due to error, timeout, or startup probe failure

## Exit Code & Restart Policy

Exit codes follow Unix convention (0 = success, non-zero = failure).

| Policy | Behavior |
|--------|----------|
| `no` | Do not restart (default) |
| `on-failure` | Restart on non-zero exit, with exponential backoff |
| `always` | Always restart, with exponential backoff |

`porta wait` blocks until the instance reaches `exited` or `failed`, then returns the exit code. Designed for CI pipelines.

## Dispatch: Serve Mode

When running as MCP server (`porta serve`):

- Create fresh WASM instance per `tools/call`
- Serialize tool arguments as JSON → agent stdin (length-prefixed)
- Read agent stdout → deserialize JSON result
- Map `{"ok": ...}` / `{"err": ...}` to MCP response format
- Capture stderr for diagnostics
- Enforce timeout and memory limits
- Clean up instance after each call (no state leaks)

## Dispatch: Run Mode

When running standalone (`porta run`):

- Create single WASM instance with full configuration
- Drive instance through state machine transitions
- Evaluate health probes (startup → liveness → readiness)
- Apply restart policy on exit
- Forward stdin/stdout/stderr
- Report exit code to caller

## Module

- `dispatch.almd` — Instance lifecycle, state machine, stdin/stdout pipe protocol
