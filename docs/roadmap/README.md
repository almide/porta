# Porta Roadmap

> Auto-generated from directory structure. Run `bash docs/roadmap/generate-readme.sh > docs/roadmap/README.md` to update.

## Active

2 items

| Item | Description |
|------|-------------|
| [Full Almide Migration](active/full-almide.md) | Migrate Rust bridge functions to pure Almide where possible |
| [Replace Interpreter with wasmtime](active/wasmtime-migration.md) | Replace hand-rolled WASM interpreter with wasmtime via FFI |

## On Hold

6 items

| Item | Description |
|------|-------------|
| [Compose](on-hold/compose.md) | Multi-service orchestration with dependency and health management |
| [Image Distribution](on-hold/image-distribution.md) | OCI-compatible image push/pull for porta agent distribution |
| [Networking](on-hold/networking.md) | Port publishing, service discovery, and capability-routed networking |
| [Self-Hosted WASM Runtime](on-hold/self-hosted-wasm-runtime.md) | Build Almide-native WASM runtime to eventually replace wasmtime |
| [Snapshot & Replay](on-hold/snapshot-and-replay.md) | Instance snapshot, suspend/resume, and deterministic execution replay |
| [Supply Chain Security](on-hold/supply-chain.md) | Image signing, provenance attestation, SBOM, and dependency locking |

## Done

7 items

<details>
<summary>Show all 7 completed items</summary>

| Done | Item | Description |
|------|------|-------------|
| 2026-04-07 | [Sandbox Enforcement](done/sandbox-enforcement.md) | Capability-based security with profiles, policy engine, network control |
| 2026-04-07 | [Observability](done/observability.md) | Logs, health probes, metrics, and OpenTelemetry tracing |
| 2026-04-07 | [MCP Protocol Implementation](done/mcp-protocol.md) | MCP protocol implementation over JSON-RPC 2.0 stdio |
| 2026-04-07 | [Instance Lifecycle & Dispatch](done/dispatch.md) | Instance lifecycle, state machine, and tool call dispatch |
| 2026-04-07 | [Configuration & Secrets](done/config-and-secrets.md) | Environment variables, config injection, and secret management |
| 2026-04-07 | [CLI](done/cli.md) | CLI with run/serve/build and full lifecycle management |
| 2026-03-25 | [WASM Interpreter](done/wasm-interpreter.md) | WASM interpreter for Almide-compiled agent binaries |

</details>

