<!-- description: Capability-based security with profiles, policy engine, network control -->

# Sandbox Enforcement

Runtime security layer (Layer 3 of Almide's three-layer defense).

## Security Model

porta's isolation boundary differs fundamentally from Docker:

- Docker relies primarily on OS kernel boundaries (namespaces, cgroups, seccomp)
- porta uses WASM execution boundaries with capability-based access control

The attack surface is smaller, and the permission model is more granular. All access is **deny-by-default** — capabilities must be explicitly granted. This is not "more secure than Docker" in a blanket sense; the two have different attack surfaces. porta's advantage is that its isolation boundary is smaller and its permissions are finer-grained.

## Core Enforcement

- Mount scoping (bind, volume, scratch — all explicit)
- Environment variable filtering (`--env` / `--env-file`)
- Memory limits (`--max-memory`)
- Timeout enforcement (epoch-based interruption)
- WASI import validation against manifest capabilities

## Network Permissions

Outbound network access follows an allowlist model (unlike Docker's default-allow):

- `--allow-net example.com:443` — Allow specific host:port
- `--allow-net *.example.com:443` — Wildcard subdomain
- `--allow-inbound 0.0.0.0:8080` — Allow inbound on port
- `--deny-net` — Deny all outbound (default)

Connections must be explicitly listed. This is one of porta's strongest differentiators against Docker's "everything is open unless you restrict it" model.

## Capability Profiles

Reusable permission sets for common use cases:

| Profile | Grants |
|---------|--------|
| `web-app` | HTTP inbound, scoped outbound, read-write mounts |
| `worker` | No inbound, outbound to queue only, ephemeral storage |
| `read-only-job` | No network, read-only mounts, bounded execution time |
| `ai-agent` | Tool call permissions, scoped network, secret access |

Profiles are composable: `--profile worker --profile metrics` combines both.

Custom profiles in `almide.toml`:

```toml
[profile.my-app]
allow-net = ["api.example.com:443"]
allow-inbound = ["0.0.0.0:8080"]
mounts = [{ src = "./data", dst = "/data", readonly = true }]
max-memory = "128MB"
timeout = "60s"
```

## Policy Engine

Organization-level constraints enforced at runtime, independent of instance-level flags:

- Outbound network restrictions (org-wide allowlist / denylist)
- Writable filesystem restrictions
- Secret access control per instance
- Tool call whitelist (in MCP serve mode)
- Mandatory capability ceilings

An instance cannot grant itself more permissions than the policy allows. Policy is the ceiling; instance flags select within that ceiling.

## 13 Capability Categories

Validated against WASI imports at module load time. The category system is the bridge between human-readable profiles and raw WASI import validation.

## Current Status

Core implemented in `sandbox.almd`:
- 8 capability types (IO, FS, FSWrite, Process, Env, Clock, Random, Net)
- Deny-by-default enforcement at import validation and runtime WASI dispatch
- 3 built-in profiles (ai-agent, worker, full)
- manifest.json capabilities → CapabilitySet parsing

### Remaining

- Network permission allowlist (`--allow-net`, `--deny-net`, `--allow-inbound`)
- Custom profiles in `almide.toml`
- Organization-level policy engine (capability ceilings)
- Mount scoping and validation
- Tool call whitelist in serve mode

## Module

- `sandbox.almd` — Capability enforcement, profile evaluation, policy engine, network permission checks
