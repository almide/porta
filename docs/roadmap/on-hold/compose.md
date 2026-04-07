<!-- description: Multi-service orchestration with dependency and health management -->

# Compose

Multi-service orchestration for porta, defined in `porta-compose.toml`.

## Service Definition

```toml
[services.api]
module = "api.wasm"
entry = "main"
ports = ["8080:80"]
env = { LOG_LEVEL = "info" }
env-file = ".env"
secrets = ["db-password", "api-key"]
mounts = [
  { src = "./data", dst = "/data" },
  { src = "./config", dst = "/config", readonly = true },
]
profile = "web-app"
restart = "on-failure"
depends_on = { db = "healthy", cache = "started" }
scale = 1

[services.db]
module = "db.wasm"
mounts = [
  { type = "volume", name = "db-data", dst = "/var/data" },
]
restart = "always"

[services.worker]
module = "worker.wasm"
entry = "worker"
env-file = ".env.worker"
secrets = ["api-key"]
depends_on = { db = "healthy", api = "healthy" }
scale = 3
restart = "on-failure"
```

## Dependency Management

### Health-Based Dependencies

`depends_on` supports conditions:

| Condition | Waits until |
|-----------|-------------|
| `started` | Instance enters `running` state |
| `healthy` | Instance passes startup probe |
| `completed_successfully` | Instance exits with code 0 |

`api` does not start until `db` reports healthy. This prevents cascading failures from premature startup.

### Start Order

Topological sort of dependency graph. Circular dependencies are rejected at parse time.

## Profiles

Selective service activation for different environments:

```toml
[services.debug-tools]
module = "debug.wasm"
profiles = ["dev"]

[services.metrics-exporter]
module = "metrics.wasm"
profiles = ["prod", "staging"]
```

- `porta compose up` — Start services with no profile (default set)
- `porta compose --profile dev up` — Include dev-only services

## Dev Override

Layer development-time configuration:

```toml
# porta-compose.override.toml (auto-loaded if present)
[services.api]
env = { LOG_LEVEL = "debug", HOT_RELOAD = "true" }
mounts = [
  { src = "./src", dst = "/app/src" },
]
```

## Shared Secret Namespace

All services in a compose environment share a secret namespace. Secrets are declared once and consumed by any service that references them. Access control is still per-service — a service only sees secrets it explicitly lists.

## Networking

- Services in the same compose file share a virtual network
- Service names resolve as hostnames
- Per-service egress control via `allow-net`
- Service-to-service permissions via permission graph
- See [networking.md](./networking.md) for details

## CLI

- `porta compose up` — Start all services (respecting dependency order)
- `porta compose down` — Stop and remove all services
- `porta compose ps` — List running services with status
- `porta compose logs [service]` — Aggregated or per-service logs
- `porta compose restart <service>` — Restart a service
- `porta compose scale <service>=N` — Adjust instance count

## Module

- `compose.almd` — TOML parser, dependency resolver, multi-instance orchestrator
