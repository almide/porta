<p align="center">
  <img src="docs/assets/logo.png" alt="Porta" width="200">
</p>

<h1 align="center">Porta</h1>

<p align="center">
  Sandboxed runtime for AI agents and native commands.<br>
  WASM isolation for agents. OS-level sandbox for everything else.
</p>

<p align="center">
  <a href="https://github.com/almide/almide">Almide</a> + <a href="https://wasmtime.dev">Wasmtime</a> · No Docker required
</p>

---

## What is Porta?

Porta is a sandboxed runtime that controls what programs can access — filesystem, network, commands — using capability-based security.

Two execution modes:
- **WASM sandbox** — Almide/Rust/C agents compiled to WASM run inside wasmtime with mathematical isolation
- **Native sandbox** — Any command (Claude Code, Python, Node.js) runs inside an OS-level sandbox (macOS sandbox-exec, Linux namespaces)

## Quick Start

### Run Claude Code in a sandbox

```bash
porta init native claude
porta up -- --print "Fix the bug in main.rs"
```

This creates a `porta.toml` and runs Claude Code with:
- Filesystem writes restricted to the current directory
- Sensitive directories (~/.ssh, ~/.aws, ~/Documents) unreadable
- Network limited to HTTPS only

### Run a WASM agent

```bash
porta run agent.wasm --profile full -v ./workspace
porta serve agent.wasm   # Start as MCP server
```

### Run Python in WASM sandbox

```bash
porta run python.wasm --env PYTHONHOME=/path/to/lib -v /path/to/lib -- script.py
```

## porta.toml

Declarative configuration for sandboxed execution.

```toml
[runtime]
type = "native"           # "native" or "wasm"
command = "claude"         # Command to run (native mode)
# wasm = "agent.wasm"     # WASM binary (wasm mode)

[sandbox]
mounts = ["."]            # Directories the command can write to
# mounts = [".:ro"]       # Read-only mount
network = ["*:443"]       # Allowed outbound ports (empty = allow all)

[env]
NODE_ENV = "production"

[secrets]
# API_KEY = "sk-..."
```

```bash
porta init native claude   # Generate porta.toml
porta up                   # Run from porta.toml
porta up -- --print "hi"   # Pass arguments to the command
```

## CLI Reference

### Execution

| Command | Description |
|---------|-------------|
| `porta up` | Run from porta.toml |
| `porta run <agent.wasm>` | Execute WASM binary |
| `porta run-native <cmd>` | Execute native command in sandbox |
| `porta serve <agent.wasm>` | Start MCP server on stdio |

### Lifecycle

| Command | Description |
|---------|-------------|
| `porta ps` | List instances |
| `porta stop <id>` | Stop instance (SIGTERM) |
| `porta kill <id>` | Kill instance (SIGKILL) |
| `porta logs <id>` | View instance logs |
| `porta rm <id>` | Remove stopped instance |
| `porta run -d <agent.wasm>` | Run in background |

### Tooling

| Command | Description |
|---------|-------------|
| `porta init [native\|wasm] [cmd]` | Create porta.toml |
| `porta build <agent.wasm>` | Generate manifest.json |
| `porta inspect <agent.wasm>` | Show module info (any size) |
| `porta validate <agent.wasm>` | Check WASI imports against profile |

### Common Options

| Flag | Description |
|------|-------------|
| `-v <path>` | Mount directory (writable) |
| `-v <path>:ro` | Mount directory (read-only) |
| `--allow-net <host:port>` | Allow outbound network |
| `--profile <name>` | Capability profile: `ai-agent`, `worker`, `full` |
| `--env <KEY=VALUE>` | Set environment variable |
| `--secret <KEY=VALUE>` | Inject secret (redacted in inspect) |
| `--step-limit <n>` | Max WASM instructions |
| `--max-memory <pages>` | Max WASM memory pages |
| `--restart <policy>` | `no`, `on-failure`, `always` |

## Security Model

### Native Sandbox (macOS)

Uses `sandbox-exec` to enforce:

| Control | Behavior |
|---------|----------|
| **FS write** | Denied everywhere except `-v` mounted dirs |
| **FS read** | `~/.ssh`, `~/.aws`, `~/.gnupg`, `~/Documents`, `~/Desktop`, `~/Downloads` denied |
| **Network** | `--allow-net "*:443"` → HTTPS only. No flag = allow all |
| **Read-only** | `-v ./data:ro` → read OK, write denied |

### WASM Sandbox

Deny-by-default capability system:

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

## MCP Server

```bash
porta serve agent.wasm --profile full
```

### Built-in Tools

| Tool | Description |
|------|-------------|
| `porta.exec` | Execute shell commands (sandboxed) |
| `porta.http` | Make HTTP requests |
| Agent tools | Dispatched to WASM agent |

### Supported MCP Methods

`initialize`, `tools/list`, `tools/call`, `resources/list`, `resources/read`, `prompts/list`, `prompts/get`, `ping`

### Claude Code Integration

```json
{
  "mcpServers": {
    "sandbox": {
      "type": "stdio",
      "command": "porta",
      "args": ["serve", "agent.wasm", "--profile", "full"]
    }
  }
}
```

## Architecture

```
porta
├── WASM Runtime (wasmtime 42)
│   ├── Module cache (instant second-run startup)
│   ├── WASI Preview 1 (filesystem, env, args, clock)
│   ├── Host functions (porta.http_request, porta.exec_command)
│   └── Fuel-based instruction limiting
│
├── Native Sandbox
│   ├── macOS: sandbox-exec profiles
│   └── Linux: namespace isolation (planned)
│
├── MCP Server
│   ├── JSON-RPC 2.0 / stdio
│   ├── Built-in tools (exec, http)
│   └── Agent tool dispatch
│
├── Instance Management
│   ├── Daemon mode (-d)
│   ├── ps / stop / kill / logs / rm
│   └── ~/.porta/instances/
│
└── Config
    ├── porta.toml (declarative)
    ├── manifest.json (agent metadata)
    └── Capability profiles
```

## Install

```bash
# From source (requires Almide >= 0.12.0)
almide build
cp porta ~/.local/bin/
```

## Language Support

| Runtime | Status | Example |
|---------|--------|---------|
| Almide → WASM | Full support | `porta run agent.wasm` |
| Python 3.14 | Runs in WASM | `porta run python.wasm -- script.py` |
| Native commands | OS sandbox | `porta run-native claude -- --print "hi"` |

## License

MIT
