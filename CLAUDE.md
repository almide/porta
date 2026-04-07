# Porta

A secure MCP bridge for Almide-compiled WASM agents. The gate between the sandbox and the world.

## What Porta Does

Porta loads an Almide-compiled `.wasm` binary and exposes it as an MCP server over JSON-RPC 2.0 / stdio. It includes a built-in WASM interpreter — no external runtime dependency.

## Architecture

```
MCP Client (Claude Code / Cursor / etc.)
    ↕ JSON-RPC 2.0 / stdio
Porta (100% Almide)
    ├── jsonrpc.almd       — JSON-RPC 2.0 protocol
    ├── mcp.almd           — MCP state machine
    ├── manifest.almd      — manifest.json parser
    ├── dispatch.almd      — Tool call dispatch
    ├── sandbox.almd       — Capability enforcement (Layer 3)
    └── wasm/
        ├── binary.almd    — .wasm binary parser
        ├── validate.almd  — Module validation
        ├── interp.almd    — Instruction interpreter
        ├── memory.almd    — Linear memory management
        └── wasi.almd      — WASI host functions
```

## Building

```bash
almide build src/main.almd -o porta
```

## Usage

```bash
porta serve agent.wasm --dir /workspace
porta inspect agent.wasm
porta validate agent.wasm
```

## Project Rules

### Branch Strategy

- **main** — protected. Only accepts PRs from `develop`
- **develop** — working branch. All commits go here

### Git Commit Rules

- Write commit messages in **English only**
- No prefix (feat:, fix:, etc.)
- Keep it to one concise line

### Testing

```bash
almide test
```

## Documentation

- Roadmap: `docs/roadmap/` — rules in [docs/roadmap/CLAUDE.md](./docs/roadmap/CLAUDE.md)
