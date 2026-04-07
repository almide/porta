<!-- description: CLI with run/serve/build and full lifecycle management -->

# CLI

Command-line interface for porta. Two execution modes: `serve` (MCP bridge) and `run` (standalone execution).

## Build Model

Language build and runtime packaging are separate concerns:

- `almide build` — Language/module compilation (`.almd` → `.wasm`)
- `porta build` — Executable artifact packaging (`.wasm` + manifest → deployable unit)

`almide build` is the compiler. `porta build` is the packager. The two are never interchangeable.

## Execution Model

### Entry Point

Every porta module has a default entry point (`main`). Override and argument passing:

- `porta run app.wasm` — Run with default entry point
- `porta run app.wasm -- task sync` — Pass arguments to entry point
- `porta run --entry worker app.wasm` — Override entry point

### MCP Serve Mode

- `porta serve agent.wasm` — Start MCP server on stdio (JSON-RPC 2.0)
- Same configuration flags as `run`, plus MCP-specific options

## Subcommands

### Execution

- `porta run <module>` — Run a WASM module
  - `--entry <name>` — Override entry point (default: `main`)
  - `-v host:guest` / `-v host:guest:ro` — Mount host path into guest
  - `--mount type=bind,src=...,dst=...` — Explicit mount specification
  - `--mount type=volume,name=data,dst=/data` — Named volume
  - `--mount type=scratch,dst=/tmp` — Ephemeral scratch
  - `--env <KEY=VALUE>` — Pass environment variable (repeatable)
  - `--env-file <path>` — Load environment variables from file
  - `--secret <name>` — Inject secret (repeatable)
  - `--secret <name>,env=<VAR>` — Inject secret as environment variable
  - `-p <host>:<guest>` — Publish port
  - `--allow-net <host:port>` — Allow outbound network access
  - `--deny-net` — Deny all outbound network
  - `--allow-inbound <bind:port>` — Allow inbound on port
  - `--profile <name>` — Apply capability profile
  - `--timeout <duration>` — Per-execution timeout (default 30s)
  - `--max-memory <size>` — Memory limit (default 256MB)
  - `--restart <policy>` — Restart policy (`no` / `on-failure` / `always`)
  - `--detach` — Run in background
  - `--record` — Record execution trace for replay
- `porta serve <module>` — Start as MCP server on stdio

### Lifecycle

- `porta ps` — List running instances
- `porta stop <instance>` — Graceful stop
- `porta kill <instance>` — Force terminate
- `porta rm <instance>` — Remove stopped instance
- `porta wait <instance>` — Block until instance exits (returns exit code; CI-friendly)
- `porta events` — Stream lifecycle events

### Inspection & Diagnostics

- `porta inspect <module|instance>` — Print detailed information (secrets never shown)
- `porta logs <instance>` — View stdout/stderr
  - `-f` — Follow (stream live)
  - `--since <time>` — Show logs since timestamp
  - `--json` — JSON lines output
- `porta validate <module>` — Check manifest ↔ binary consistency

### Snapshot & Replay

- `porta snapshot <instance>` — Capture running instance state
- `porta restore <snapshot>` — Restore instance from snapshot
- `porta clone <instance>` — Instant clone from current state
- `porta replay <trace>` — Replay recorded execution
- `porta replay --diff <trace1> <trace2>` — Compare two executions

### Build & Registry

- `porta build` — Package `.wasm` + manifest into deployable artifact
- `porta push <image>` — Push to OCI registry (`--sign` to sign on push)
- `porta pull <image>` — Pull from OCI registry (`--verify` to reject unsigned)
- `porta verify <image>` — Verify image signature
- `porta sbom <image>` — Export software bill of materials

### Compose

- `porta compose up` — Start all services from `porta-compose.toml`
- `porta compose down` — Stop and remove all services
- `porta compose ps` — List compose services
- `porta compose logs [service]` — Aggregated or per-service logs
- `porta compose restart <service>` — Restart a service
- `porta compose scale <service>=N` — Adjust instance count

## Mount UX

Internal implementation uses WASI pre-opens, but the user-facing API follows mount-model conventions:

| Form | Example | Description |
|------|---------|-------------|
| Short | `-v /data:/data` | Bind mount (read-write) |
| Short (ro) | `-v /data:/data:ro` | Bind mount (read-only) |
| Explicit bind | `--mount type=bind,src=/data,dst=/data` | Verbose form |
| Named volume | `--mount type=volume,name=db,dst=/var/data` | Persistent named volume |
| Scratch | `--mount type=scratch,dst=/tmp` | Ephemeral, discarded on exit |

## Module

- `main.almd` — Argument parsing, subcommand dispatch
