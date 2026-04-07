<!-- description: Port publishing, service discovery, and capability-routed networking -->

# Networking

Network model for porta instances. All network access is explicitly granted, never implicitly available.

## Port Publishing

Expose instance ports to the host:

- `porta run -p 8080:80 app.wasm` — Map host 8080 to guest 80
- `porta run -p 127.0.0.1:8080:80 app.wasm` — Loopback only
- `porta run -p 8080:80/tcp app.wasm` — Protocol specification

Multiple ports: `-p 8080:80 -p 8443:443`

## Service Discovery

Within a porta compose environment, services resolve each other by name:

```toml
[services.api]
module = "api.wasm"
ports = ["8080:80"]

[services.db]
module = "db.wasm"

[services.worker]
module = "worker.wasm"
```

- `api` can reach `db` via hostname `db`
- DNS-like name resolution within the compose network
- No implicit access to host network or other compose environments

## Egress Control

Outbound connections follow an allowlist model:

- `--allow-net example.com:443` — Specific host:port
- `--allow-net *.example.com:443` — Wildcard subdomain
- `--deny-net` — Block all outbound (default)

In compose mode, service-to-service communication within the same network is automatically allowed. External egress must be explicitly declared per service.

## Network Isolation

- Each compose environment has its own virtual network
- Instances cannot reach each other unless in the same network
- Host network access is never implicit
- Cross-compose communication requires explicit network linking

## Service-to-Service Permission Graph

Fine-grained control over which services can communicate within a compose network:

```toml
[services.api]
module = "api.wasm"
depends_on = ["db"]
allow-net = ["db:5432", "cache:6379"]

[services.worker]
module = "worker.wasm"
allow-net = ["db:5432"]
# worker cannot reach api or cache
```

Even within the same network, connections can be scoped. This goes beyond Docker's all-or-nothing network membership.

## Module

- `networking.almd` — Port binding, name resolution, egress enforcement
