<p align="center">
  <img src="docs/assets/logo.png" alt="Porta" width="200">
</p>

<h1 align="center">Porta</h1>

<p align="center">
  A secure MCP bridge for Almide-compiled WASM agents.<br>
  The gate between the sandbox and the world.
</p>

<p align="center">
  Written in <a href="https://github.com/almide/almide">Almide</a> with a built-in WASM interpreter. No external runtime dependency.
</p>

---

## What is Porta?

Porta loads an Almide-compiled `.wasm` binary and exposes it as an MCP server over JSON-RPC 2.0 / stdio. It includes a built-in WASM interpreter — no wasmtime, wasmer, or other external runtime needed.

## Install

```bash
# From source (requires Almide >= 0.12.0)
almide build
cp porta ~/.local/bin/
```

## Quick Start

```bash
# Run a WASM agent
porta run agent.wasm

# Start as MCP server (for Claude Code, Cursor, etc.)
porta serve agent.wasm

# Generate manifest from WASM binary
porta build agent.wasm

# Inspect module info
porta inspect agent.wasm
```

## Architecture

```
MCP Client (Claude Code / Cursor / etc.)
    | JSON-RPC 2.0 / stdio
Porta
    |-- jsonrpc.almd       JSON-RPC 2.0 protocol
    |-- mcp.almd           MCP state machine (tools, resources, prompts)
    |-- manifest.almd      manifest.json parser
    |-- dispatch.almd      Instance lifecycle, restart policies
    |-- sandbox.almd       Capability enforcement (deny-by-default)
    |-- observability.almd Metrics and diagnostic logging
    |-- util.almd          CLI utilities
    +-- wasm/
        |-- binary.almd    .wasm binary parser
        |-- interp.almd    Stack machine interpreter + WASI
        +-- memory.almd    Linear memory management
```

## CLI Reference

```
porta run <agent.wasm> [options]     Execute WASM binary
porta serve <agent.wasm> [options]   Start MCP server on stdio
porta build <agent.wasm>             Generate manifest.json
porta inspect <agent.wasm>           Show module info
porta validate <agent.wasm>          Validate WASI imports against profile
porta help [command]                 Show help
porta version                        Show version
```

### Common Options

| Flag | Description | Default |
|------|-------------|---------|
| `--entry <name>` | Entry point function | `_start` |
| `--step-limit <n>` | Max WASM instructions (0 = unlimited) | `0` |
| `--max-memory <pages>` | Max WASM memory pages (0 = unlimited) | `0` |
| `--restart <policy>` | `no`, `on-failure`, `always` | `no` |
| `--profile <name>` | `ai-agent`, `worker`, `full` | varies |
| `--env <KEY=VALUE>` | Set environment variable (repeatable) | |
| `--env-file <path>` | Load env vars from file | |
| `--secret <KEY=VALUE>` | Inject secret (repeatable, redacted in inspect) | |
| `--manifest <path>` | Path to manifest.json | |

## MCP Integration

### Claude Code

```json
// .claude/.mcp.json
{
  "mcpServers": {
    "agent": {
      "type": "stdio",
      "command": "porta",
      "args": ["serve", "agent.wasm"]
    }
  }
}
```

### Supported MCP Methods

- `initialize` / `notifications/initialized`
- `tools/list`, `tools/call`
- `resources/list`, `resources/read`
- `prompts/list`, `prompts/get`
- `ping`

## Security Model

Porta uses a **capability-based, deny-by-default** security model. All WASI access requires explicit capability grants.

### Capability Profiles

| Profile | Grants |
|---------|--------|
| `ai-agent` | IO, Process (minimal for MCP tool dispatch) |
| `worker` | IO, Process, Clock, Random |
| `full` | All capabilities |

### 8 Capability Types

`io`, `fs`, `fs.write`, `process`, `env`, `clock`, `random`, `net`

### Enforcement

1. **Import validation** — WASI imports checked against capabilities at module load
2. **Runtime check** — Every WASI call verified before execution

### Three-Layer Defense

1. **Compiler** (Layer 1) — Almide rejects capability violations at compile time
2. **Binary** (Layer 2) — Disallowed WASI imports are absent from `.wasm`
3. **Porta** (Layer 3) — Runtime enforcement: fd table, preopen dirs, env filtering, memory limits, step limits

## Manifest Format

```json
{
  "schema_version": "1.0",
  "name": "my-agent",
  "version": "0.1.0",
  "description": "An example agent",
  "entry": "_start",
  "capabilities": ["io", "fs"],
  "tools": [
    {
      "name": "greet",
      "description": "Say hello",
      "inputSchema": { "type": "object", "properties": { "name": { "type": "string" } } }
    }
  ],
  "resources": [
    {
      "uri": "file:///config",
      "name": "Config",
      "description": "Agent configuration",
      "mimeType": "application/json"
    }
  ],
  "prompts": [
    {
      "name": "summarize",
      "description": "Summarize a document",
      "arguments": [{ "name": "text", "description": "Text to summarize", "required": true }]
    }
  ],
  "wasi_imports": ["fd_write", "fd_read", "proc_exit"]
}
```

## Observability

- **Run mode**: Metrics summary printed to stderr after execution (steps, memory, restarts)
- **Serve mode**: Diagnostic log per tool call to stderr: `[porta] tool=name steps=N memory=M ok`

## License

MIT
