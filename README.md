<p align="center">
  <img src="docs/assets/logo.png" alt="Porta" width="200">
</p>

<h1 align="center">Porta</h1>

<p align="center">
  Run an agent with the permissions you actually granted it.<br>
  OS-enforced limits for the CLI agents you already use, and a WASM runtime for the ones you build.
</p>

<p align="center">
  <a href="https://github.com/almide/almide">Almide</a> + <a href="https://wasmtime.dev">Wasmtime</a> · No Docker required
</p>

<p align="center">
  English · <a href="README.ja.md">日本語</a>
</p>

---

## Who this is for

- **You run a CLI agent** and want it unable to write outside one directory or
  reach hosts you did not allow — enforced by the OS, not by a prompt.
- **You hand agents to a team** and need a record of where they tried to connect.
- **You build agents** and need a run that can be resumed after a crash without
  repeating work, and a completion check the agent cannot talk its way past.

If none of these are your problem yet, Porta will not be interesting. It is a
runtime and a set of restrictions, not an agent.

## Install

```bash
bash scripts/install-almide.sh
.tools/almide/almide build src/mod.almd -o target/porta
cp target/porta ~/.local/bin/
```

Needs Almide 0.63.0, a Rust toolchain, Python 3 and curl. Full verification
steps and the compiler pin are in [Build from source](#build-from-source).

## Quick Start

### 1. Restrict an agent you already use

```bash
porta run claude --allow-net 'api.anthropic.com:443' -v ./project -e "HOME=$HOME" \
  -- --print "Fix the bug in main.rs"
```

`claude` runs normally, but it can write only inside `./project` and connect
only to the host you listed. No Docker daemon, no container image, no change to
the agent itself.

### 2. See a restriction actually stop something

A working restriction is invisible, so watch one fail:

```bash
porta run curl -- https://example.com                    # network open by default
porta run curl --allow-net '*:443' -- https://example.com # HTTPS allowed → works
porta run curl --allow-net '*:80'  -- https://example.com # → exit status 7, port 443 denied
```

Record where an agent tried to go:

```bash
porta run claude --proxy-allow 'api.anthropic.com' --proxy-audit egress.jsonl \
  -v ./project -- --print "..."
```

### 3. Keep the settings instead of retyping them

```bash
porta init native claude   # writes porta.toml
porta up -- --print "Fix the bug in main.rs"
```

### 4. Build an agent that runs inside WASM

```bash
.tools/almide/almide build examples/chat-agent/src/mod.almd --target wasm -o examples/chat-agent/agent.wasm
# Set your model endpoint and name in examples/chat-agent/agent.toml
porta agent examples/chat-agent/agent.toml -- "Add 20 and 22 using the tool."
```

The decision loop and its tools both run in WASM. Model credentials stay in the
host, budgets are shared across a delegating team, and tool arguments are
validated before anything executes. Record a run to survive a crash:

```bash
porta agent agent.toml --record run.jsonl -- "..."
porta agent-resume agent.toml run.jsonl   # completed writes are not repeated
porta agent-journal run.jsonl             # read-only, no code loaded
```

## What Porta enforces

| Concern | Mechanism |
|---|---|
| Writes outside the workspace | `-v` mounts; nothing is mounted implicitly |
| Network egress | `--allow-net` at the OS layer, `--proxy-allow` per host over HTTPS CONNECT |
| Secrets reaching the guest | model credentials are held by the host, never the agent |
| An agent declaring itself finished | [completion checks](docs/completion-checks.md): operator-owned WASM the guest cannot bypass |
| Prerequisites before an effect | [pre-tool checks](docs/before-tool-checks.md) run before writes, remote calls or delegation |
| Wrong or injected tool arguments | validated against the declared schema before execution |
| Crash in the middle of work | [journals](docs/agent-journals.md): resume without repeating completed writes; uncertain operations are never retried |
| Code drifting from what was reviewed | [artifact pins](docs/artifact-pins.md) bind WASM and delegated policies to SHA-256 values |

## Evidence

Every published number keeps its raw report, source hashes and an audit that
regrades it, and CI runs those audits. Results that do not favour Porta are
published in the same place.

- [Startup and memory](docs/benchmarks/startup-and-memory.md) — fixed-response
  measurements with explicit comparison limits, not task quality.
- [Containment and recovery](docs/benchmarks/containment-evaluation.md) — under
  hostile inputs, scored on what escaped rather than what succeeded. One of five
  scenarios separated the runtimes, and the containment cost task completion.
- [Real-task quality](docs/benchmarks/README.md) — small matched suites on a
  local model. Porta has not demonstrated general quality superiority, and the
  shared-compute follow-up was rejected rather than published as a win.

## Learn more

| Topic | Document |
|---|---|
| WASM agents, teams, grants, limits | [agent-runtime.md](docs/agent-runtime.md) |
| Checkpoint, resume, replay | [agent-journals.md](docs/agent-journals.md) |
| Remote MCP tools from an agent | [agent-mcp.md](docs/agent-mcp.md) |
| Offline team inspection | [agent-check.md](docs/agent-check.md) |
| Completion checks | [completion-checks.md](docs/completion-checks.md) |
| Pre-tool checks | [before-tool-checks.md](docs/before-tool-checks.md) |
| Artifact pins | [artifact-pins.md](docs/artifact-pins.md) |
| Bounded computation tool | [examples/compute](examples/compute/README.md) |
| Measurements | [docs/benchmarks](docs/benchmarks/README.md) |

## Limits

Native restrictions cover **macOS** and **Linux**, and not identically. macOS
uses `sandbox-exec`; Linux uses Landlock, which enforces writes and TCP ports
but **does not confine reads** — it is allow-list only and cannot express the
macOS denial of `~/.ssh` and `~/.gnupg` while leaving other reads permitted.
HTTPS proxy filtering stays macOS-only: it must also deny UDP and Unix sockets,
which Landlock cannot express, so proxy mode refuses to run on Linux rather than
enforce part of a policy. A kernel whose Landlock ABI cannot express a requested
rule refuses the run instead of widening it.

Native read access is broader than write access on both: this is not a container
filesystem or complete secret isolation. HTTPS proxy filtering controls
connection targets, not TLS contents. `porta run` and `porta serve` mount
nothing unless you pass `-v`.

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
network = ["*:443"]       # Restrict to these ports (empty = all open)

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
| `porta agent <agent.toml> [--record <journal>] -- <task>` | Run a WASM agent or team |
| `porta agent-resume <agent.toml> <journal>` | Continue a recorded run |
| `porta agent-replay <agent.toml> <journal>` | Verify a completed run offline |
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
| **FS write** | Denied everywhere except `-v` mounted dirs and `/tmp` |
| **FS read** | `~/.ssh` and `~/.gnupg` denied (cryptographic keys). Other readable host files remain accessible; `-v` controls writes |
| **Network** | Open by default. `--allow-net "*:443"` restricts to HTTPS only |
| **Read-only** | `-v ./data:ro` → read OK, write denied |

> Note: macOS sandbox-exec supports port-based filtering only. Host-based filtering (`api.example.com:443`) is enforced at the MCP layer for builtin tools.

### Host-filtered HTTPS

```bash
porta run claude -v . --proxy-allow "api.anthropic.com,*.anthropic.com"
```

Or add `[proxy]` with `allow = ["api.anthropic.com"]` to `porta.toml`.
Both `run` and `up` apply the same policy. The child can connect only to the
local CONNECT proxy; direct TCP, UDP, and Unix-socket egress are denied.
Only HTTPS CONNECT on port 443 is supported. Clients must respect `HTTPS_PROXY`.
This filters connection targets, not TLS contents. Deny lists are weaker than
explicit allow lists. It is not a credential broker or a private-address filter.

### Native Restrictions (Linux)

Uses Landlock, unprivileged and without namespaces or an external runtime:

| Control | Behavior |
|---------|----------|
| **FS write** | Denied everywhere except `-v` mounted dirs, `/tmp` and `/dev` |
| **FS read** | Not confined. Landlock is allow-list only, so the macOS denial of `~/.ssh` and `~/.gnupg` has no equivalent |
| **Network** | Open by default. `--allow-net '*:443'` restricts TCP connect by port, needing Landlock ABI 4 |
| **Proxy mode** | Refused. It must also deny UDP and Unix sockets, which Landlock cannot express |

A requested rule this kernel cannot express refuses the run rather than widening
it: partial enforcement is never silently accepted.

Native read access is broader than write access on both platforms: this is not a
container filesystem or complete secret isolation.

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
Direct `porta.exec_command` / `porta.http_request` WASM imports are rejected;
host execution and HTTP requests go through the checked MCP built-in tools.
`porta.http` accepts HTTP(S) URLs without userinfo and does not follow redirects
or inherit host proxy settings.

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
├── jsonrpc.almd        — Newline-delimited JSON-RPC
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

## Build from source

```bash
# From source (Almide 0.63.0, Rust toolchain, Python 3, curl)
bash scripts/install-almide.sh
.tools/almide/almide build src/mod.almd -o target/porta
.tools/almide/almide test --ci
python3 scripts/integration.py target/porta
cp target/porta ~/.local/bin/
```

The compiler target is **0.63.0**. As of 2026-09-19, the published artifact is
`v0.63.0-rc1` (reports `almide 0.63.0`); the installer pins that release and checks
its published SHA-256. Once the final tag is published, select it with
`ALMIDE_RELEASE_TAG=v0.63.0 bash scripts/install-almide.sh`.

## Language Support

| Runtime | Status | Example |
|---------|--------|---------|
| Almide → WASM | Full support | `porta run agent.wasm` |
| Python 3.14 | Runs in WASM | `porta run python.wasm -- script.py` |
| Native commands | OS restrictions | `porta run claude -- --print "hi"` |

## License

Apache-2.0
