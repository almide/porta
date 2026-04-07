<!-- description: Logs, health probes, metrics, and OpenTelemetry tracing -->

# Observability

Runtime monitoring and diagnostics for porta instances. WASM's instruction-level visibility enables observability that is significantly more granular than Docker's process-level model.

## Logs

### Collection

- stdout and stderr captured per instance
- Each line tagged with instance ID, timestamp, and trace ID
- Storage: local file-based log with rotation

### CLI

- `porta logs <instance>` — View logs
- `porta logs -f <instance>` — Follow live
- `porta logs --since 5m <instance>` — Recent logs
- `porta logs --json <instance>` — JSON lines output

### Format

```json
{"ts":"2026-04-07T12:00:00Z","instance":"abc123","stream":"stdout","msg":"ready","trace_id":"t-789"}
```

## Health Probes

Three-probe model, aligned with Kubernetes conventions:

### Startup Probe

- Runs during `starting` state
- Determines when instance is ready to accept traffic
- Failure within window → `failed` state
- Prevents premature liveness checks from killing a slow-starting instance

### Liveness Probe

- Runs periodically during `running` / `healthy` state
- Failure → instance restarted (per restart policy)
- Detects deadlocks and unrecoverable states

### Readiness Probe

- Runs periodically during `running` / `healthy` state
- Failure → instance removed from service routing (but not restarted)
- Detects temporary inability to handle requests

### Configuration

```toml
[healthcheck.startup]
command = "porta-health startup"
interval = "2s"
timeout = "1s"
retries = 10

[healthcheck.liveness]
command = "porta-health live"
interval = "10s"
timeout = "3s"
retries = 3

[healthcheck.readiness]
command = "porta-health ready"
interval = "5s"
timeout = "2s"
retries = 1
```

A single `healthcheck` shorthand is also supported, but production deployments should use the three-probe model.

## Metrics

Per-instance metrics:

- Memory usage (current / peak / limit)
- CPU time consumed
- Instruction count
- Tool call count and latency (serve mode)
- Filesystem I/O (bytes read/written)
- Network I/O (bytes sent/received, connection count)
- Restart count

Export: Prometheus-compatible endpoint or JSON pull.

## Tracing

OpenTelemetry-native tracing:

### Span Hierarchy

```
instance lifecycle (create → start → run → exit)
  └─ command execution
       └─ tool call (serve mode)
            ├─ filesystem access
            ├─ network access
            └─ memory operations
```

### Export

- OpenTelemetry Protocol (OTLP) exporter
- Per-instance timeline via `porta inspect --trace <instance>`
- Trace ID propagation across compose services

## Module

- `observability.almd` — Log collection, health probe runner, metrics aggregation, trace span management
