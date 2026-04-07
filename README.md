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

## Install

```bash
# From source
almide build src/mod.almd -o porta
cp porta ~/.local/bin/
```

## Usage

```bash
# Start MCP server
porta serve agent.wasm --dir /workspace

# Inspect agent manifest
porta inspect agent.wasm

# Validate manifest ↔ binary consistency
porta validate agent.wasm
```

## Claude Code Integration

```json
// .claude/.mcp.json
{
  "mcpServers": {
    "agent": {
      "type": "stdio",
      "command": "porta",
      "args": ["serve", "agent.wasm", "--dir", "/workspace"]
    }
  }
}
```

## Security

Three-layer defense:

1. **Compiler** (Layer 1) — Almide rejects capability violations at compile time
2. **Binary** (Layer 2) — Disallowed WASI imports are physically absent from the `.wasm`
3. **Porta** (Layer 3) — Runtime enforcement: pre-opened directories, env filtering, memory limits, timeouts

## License

MIT
