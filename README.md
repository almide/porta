<p align="center">
  <img src="docs/assets/logo.png" alt="Porta" width="200">
</p>

<h1 align="center">Porta</h1>

<p align="center">
  A secure MCP bridge for AI agents and native commands.<br>
  WASM isolation for agents. OS-level restrictions for everything else.
</p>

<p align="center">
  <a href="https://github.com/almide/almide">Almide</a> + <a href="https://wasmtime.dev">Wasmtime</a> · No Docker required
</p>

---

## What is Porta?

Porta controls what programs can access — filesystem, network, commands — using capability-based security.

Two execution modes:
- **WASM sandbox** — Almide/Rust/C agents compiled to WASM run inside wasmtime with mathematical isolation
- **Native restrictions** — Any command (Claude Code, Python, Node.js) runs with OS-level filesystem and network restrictions (macOS sandbox-exec)

## Quick Start

### Run Claude Code with restrictions

```bash
porta run claude --allow-net '*:443' -v . -e "HOME=$HOME" -- --print "Fix the bug"
```

Or declaratively:

```bash
porta init native claude
porta up -- --print "Fix the bug in main.rs"
```

### Verify network restrictions

```bash
# All network blocked (no --allow-net)
porta run curl -- https://example.com
# → exit status: 7 (blocked by sandbox)

# Only HTTPS allowed
porta run curl --allow-net '*:443' -- https://example.com
# → works

# Specific port only — HTTPS blocked
porta run curl --allow-net '*:80' -- https://example.com
# → exit status: 7 (port 443 not allowed)
```

### Run a WASM agent

```bash
porta run agent.wasm --profile full -v ./workspace
porta serve agent.wasm   # Start as MCP server
```

`porta run` auto-detects mode: `.wasm` files run in WASM sandbox, everything else runs as a native command with OS-level restrictions.

## porta.toml

Declarative configuration for restricted execution.

```toml
[runtime]
type = "native"           # "native" or "wasm"
command = "claude"         # Command to run (native mode)
# wasm = "agent.wasm"     # WASM binary (wasm mode)

[sandbox]
mounts = ["."]            # Directories the command can write to
# mounts = [".:ro"]       # Read-only mount
network = ["*:443"]       # Allowed outbound ports (empty = all blocked)

[env]
NODE_ENV = "production"

[secrets]
API_KEY = "sk-..."
# Or read from host environment:
# API_KEY = { from-env = true }
```

```bash
porta init native claude   # Generate porta.toml
porta up                   # Run from porta.toml
porta up -- --print "hi"   # Pass arguments to the command
```

## CLI Reference

### Project

| Command | Description |
|---------|-------------|
| `porta init [native\|wasm] [cmd]` | Create porta.toml |
| `porta up [-- args...]` | Run from porta.toml |

### Runtime

| Command | Description |
|---------|-------------|
| `porta run <target>` | Execute WASM (.wasm) or native command |
| `porta run -d <agent.wasm>` | Run WASM as background daemon |
| `porta serve <agent.wasm>` | Start MCP server on stdio |

### Development

| Command | Description |
|---------|-------------|
| `porta build <agent.wasm>` | Generate manifest.json |
| `porta inspect <agent.wasm>` | Show module info |
| `porta validate <agent.wasm>` | Check WASI imports against profile |

### Instances

| Command | Description |
|---------|-------------|
| `porta ps` | List instances |
| `porta stop <id>` | Stop instance (SIGTERM) |
| `porta kill <id>` | Kill instance (SIGKILL) |
| `porta logs <id>` | View instance logs |
| `porta rm <id>` | Remove stopped instance |

### Common Options

| Flag | Description |
|------|-------------|
| `-e`, `--env <KEY=VALUE>` | Set environment variable |
| `--env-file <path>` | Load env vars from file |
| `--secret <KEY=VALUE>` | Inject secret as env var |
| `-v <path>` | Mount directory (writable) |
| `-v <path>:ro` | Mount directory (read-only) |
| `--allow-net <host:port>` | Allow outbound network (repeatable) |
| `--allow-exec <cmd,...>` | Allow specific commands (comma-separated) |
| `--profile <name>` | Capability profile: `ai-agent`, `worker`, `full` |
| `--step-limit <n>` | Max WASM instructions |
| `--max-memory <pages>` | Max WASM memory pages |
| `--restart <policy>` | `no`, `on-failure`, `always` |
| `-d`, `--detach` | Run as background daemon |
| `--help`, `-h` | Show help for any command |

## Security Model

### Two-Layer Enforcement

Porta enforces restrictions at two levels:

1. **OS layer** (sandbox-exec) — Process-level port-based network control and filesystem restrictions. Cannot be bypassed by the child process.
2. **MCP layer** — Application-level host+port URL filtering and capability checks on `porta.exec` and `porta.http` builtin tools.

### Native Restrictions (macOS)

Uses `sandbox-exec` to enforce:

| Control | Behavior |
|---------|----------|
| **FS write** | Denied everywhere except `-v` mounted dirs |
| **FS read** | `~/.ssh`, `~/.aws`, `~/.gnupg`, `~/.kube`, `~/.docker`, `~/Documents`, `~/Desktop`, `~/Downloads`, `~/Pictures` denied |
| **Network** | **All blocked by default.** `--allow-net "*:443"` → allow HTTPS port only |
| **Read-only** | `-v ./data:ro` → read OK, write denied |

> Note: macOS sandbox-exec supports port-based filtering only. Host-based filtering (`api.example.com:443`) is enforced at the MCP layer for builtin tools.

### WASM Sandbox

Deny-by-default capability system. Every WASI import is validated against the capability set before execution.

| Capability | Controls |
|------------|----------|
| `io` | stdin/stdout/stderr |
| `fs` | File read (path_open, stat, readdir) |
| `fs.write` | File write (create, rename, delete) |
| `process` | Process lifecycle, args |
| `env` | Environment variables |
| `clock` | Time/clock |
| `random` | Random bytes |
| `net` | Network access |
| `exec` | Command execution |

Built-in profiles: `ai-agent` (IO + Process), `worker` (+Clock +Random), `full` (all).

Manifest capabilities are respected in both `serve` and `run` modes.

## MCP Server

```bash
porta serve agent.wasm --profile full
```

### Built-in Tools

| Tool | Requires | Description |
|------|----------|-------------|
| `porta.exec` | `CapExec` + `--allow-exec` | Execute a command with filesystem and network restrictions |
| `porta.http` | `CapNet` + `--allow-net` | Make HTTP requests to allowed hosts |
| Agent tools | — | Dispatched to WASM agent |

### Supported MCP Methods

`initialize`, `tools/list`, `tools/call`, `resources/list`, `resources/read`, `prompts/list`, `prompts/get`, `ping`

### Claude Code Integration

```json
{
  "mcpServers": {
    "agent": {
      "type": "stdio",
      "command": "porta",
      "args": ["serve", "agent.wasm", "--profile", "full", "--allow-net", "*:443"]
    }
  }
}
```

## Architecture

```
porta
├── cli.almd            — Options, arg parsing, help
├── mod.almd            — Command dispatch (entry point)
│
├── engine.almd         — serve, run, validate, inspect
├── dispatch.almd       — WASM instance lifecycle & tool dispatch
├── mcp.almd            — MCP protocol (JSON-RPC 2.0 / stdio)
├── jsonrpc.almd        — Content-Length framed JSON-RPC
├── sandbox.almd        — Capability-based security
│
├── ops.almd            — Daemon management (ps/stop/kill/logs/rm)
├── build.almd          — Manifest generation
├── project.almd        — porta.toml (up/init)
│
├── wasm_rt.almd        — Wasmtime bridge + runtime functions
├── config.almd         — porta.toml parser
├── manifest.almd       — manifest.json parser
├── observability.almd  — Execution metrics
├── util.almd           — CLI utilities
│
└── wasm/
    ├── binary.almd     — WASM binary parser
    └── wasi.almd       — WASI Preview 1 host functions
```

## Install

```bash
# From source (requires Almide >= 0.12.0)
almide build src/mod.almd -o porta
cp porta ~/.local/bin/
```

## Language Support

| Runtime | Status | Example |
|---------|--------|---------|
| Almide → WASM | Full support | `porta run agent.wasm` |
| Python 3.14 | Runs in WASM | `porta run python.wasm -- script.py` |
| Native commands | OS restrictions | `porta run claude -- --print "hi"` |

## License

MIT
