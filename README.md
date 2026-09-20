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

`porta run` takes either a native command or a `.wasm` module, so anything that
compiles to WASI runs under it — Almide, and Python 3.14 via `python.wasm`.

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

### 4. Run an agent whose decision loop is WASM

```bash
.tools/almide/almide build examples/chat-agent/src/mod.almd --target wasm -o examples/chat-agent/agent.wasm
# Set your model endpoint and name in examples/chat-agent/agent.toml
porta agent examples/chat-agent/agent.toml -- "Add 20 and 22 using the tool."
```

[Agents you build](#agents-you-build) is what this one is.

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

## Agents you build

`porta run` restricts an agent someone else wrote. `porta agent` runs one whose
decision loop is itself WASM, which is where the rest of this README's
guarantees come from.

```toml
# agent.toml
[agent]
wasm = "agent.wasm"
instruction = "Use tools to solve the task."

[model]
endpoint = "https://api.example.com/v1/chat/completions"
name = "your-model"
token_env = "MODEL_TOKEN"        # read by the host, never handed to the guest

[limits]
max_model_calls = 20

[[tools]]
name = "write_file"
wasm = "tools/write.wasm"
sha256 = "..."                    # pinned: this exact reviewed artifact or no run
mounts = [{ host = "workspace", guest = ".", read_only = false }]
```

The loop and every tool run in separate WASM instances. Neither inherits your
environment or a directory. Credentials stay in the host, so an agent that is
talked into printing its own configuration has nothing to print.

Each guarantee in [What Porta enforces](#what-porta-enforces) links to how it
works. Three things that table does not cover:

| You want | Read |
|---|---|
| Teams, delegation, shared budgets | [agent-runtime.md](docs/agent-runtime.md) |
| Remote MCP tools, granted explicitly | [agent-mcp.md](docs/agent-mcp.md) |
| Inspect a team without running it | [agent-check.md](docs/agent-check.md) |

```bash
porta agent agent.toml --record run.jsonl -- "..."
porta agent-resume agent.toml run.jsonl   # completed writes are not repeated
porta agent-journal run.jsonl             # read-only metadata, no code loaded
```

## Limits

What Porta does **not** do, stated here rather than discovered later:

- **It is not a container or a VM.** Restrictions are applied to a process on
  your kernel. A kernel that can be exploited is a kernel both sides share.
- **Read access is broader than write access** unless you pass
  `--read-policy strict`, and even then the system directories a command needs
  to start stay readable. This is not complete secret isolation.
- **Proxy filtering controls connection targets, not TLS contents**, and it
  does not stop a child from listening on a port.
- **macOS and Linux only**, and not identically — see
  [Native Restrictions](#native-restrictions) for exactly where they differ.
  Anywhere else, native execution fails closed rather than running unrestricted.
- **Nothing is mounted implicitly.** `porta run` and `porta serve` see no
  directory until you pass `-v`, which is a limit in the useful direction.

A rule the platform cannot express refuses the run instead of widening it.
That is the rule the rest of this README is written against.

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
| `--allow-net <host:port>` | Allow outbound TCP by port (repeatable). The host part is not enforced at the OS layer — use `--proxy-allow` for that |
| `--proxy-allow <hosts>` | Route egress through porta's CONNECT proxy and allow only these hosts |
| `--proxy-deny <hosts>` | Same, denying these hosts |
| `--proxy-audit <path>` | Append every proxy decision to a JSONL file |
| `--read-policy <open\|strict>` | `strict` confines reads to your mounts and the system directories (default `open`) |
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

1. **OS layer** — `sandbox-exec` on macOS, Landlock plus a seccomp filter on Linux. Filesystem and network restrictions the child process cannot lift, because they are applied to it before it starts.
2. **MCP layer** — Application-level host+port URL filtering and capability checks on `porta.exec` and `porta.http` builtin tools.

### Native Restrictions

One table, because the two platforms differ and reading two near-identical ones
does not show you where.

| Control | macOS (`sandbox-exec`) | Linux (Landlock + seccomp) |
|---|---|---|
| **Write** | denied outside `-v` mounts, `/tmp` | denied outside `-v` mounts, `/tmp`, `/dev` |
| **Read, default** | `~/.ssh` and `~/.gnupg` denied; everything else readable | not confined |
| **Read, `--read-policy strict`** | your mounts plus `/usr`, `/System`, `/bin`, `/sbin`, `/etc`, `/tmp`, `/dev` | your mounts plus `/usr`, `/lib`, `/bin`, `/sbin`, `/etc`, `/proc`, `/tmp`, `/dev` |
| **Read-only mount** | `-v ./data:ro` → read yes, write no | same |
| **Network by port** | `--allow-net '*:443'` | same, needs Landlock ABI 4 |
| **Network by host** | `--proxy-allow` only, never `--allow-net` | same |
| **Proxy mode** | enforced by the profile | enforced by Landlock (the TCP port) plus seccomp (everything else) |

Under `strict`, every home directory is closed — and so is the command itself if
it lives outside those directories. A toolchain under `/opt` needs `-v` on its
own installation; porta says which grant is missing rather than failing with a
bare `Permission denied`.

Linux uses Landlock unprivileged, without namespaces and without an external
runtime. A rule the running kernel cannot express refuses the run rather than
widening it: partial enforcement is never silently accepted.

### Host-filtered HTTPS

```bash
porta run claude -v . --proxy-allow "api.anthropic.com,*.anthropic.com"
```

Or add `[proxy]` with `allow = ["api.anthropic.com"]` to `porta.toml`. Both
`run` and `up` apply the same policy. The child can reach only the local CONNECT
proxy; direct TCP, UDP and Unix-socket egress are denied, on Linux by a seccomp
filter that also refuses `io_uring`, because a ring can open a socket without
ever asking for one.

Only HTTPS CONNECT on port 443 is supported, and clients must respect
`HTTPS_PROXY`. Deny lists are weaker than explicit allow lists, and this is not
a credential broker or a private-address filter. See [Limits](#limits).

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

Two sides. Almide decides policy; Rust applies it and talks to the kernel.

**`src/` — Almide.** The CLI, the MCP protocol, capability checks, and what a
run is allowed to do.

| | |
|---|---|
| `mod.almd`, `cli.almd`, `help.almd` | command dispatch, options, help |
| `engine.almd`, `dispatch.almd` | serve / run / validate / inspect, and the WASM instance lifecycle |
| `mcp.almd`, `mcp_builtins.almd`, `mcp_content.almd`, `jsonrpc.almd` | the MCP session, `porta.exec` and `porta.http`, resources and prompts, framing |
| `sandbox.almd`, `wasm_imports.almd` | capability sets, and the import shape they are checked against |
| `agent.almd`, `proxy.almd` | WASM agents and teams, the CONNECT proxy's configuration |
| `config.almd`, `manifest.almd`, `project.almd`, `build.almd` | porta.toml, manifest.json, `init` / `up` |
| `ops.almd`, `observability.almd`, `util.almd` | daemons, metrics, helpers |
| `wasm_rt.almd` | every `@extern` into the Rust side |

**`native/` — Rust.** Wasmtime, the OS enforcement, and the broker that holds
model credentials.

| | |
|---|---|
| `wasmtime_bridge.rs` | WASM instance lifecycle and the FFI surface |
| `sandbox_exec.rs`, `sandbox_profile.rs`, `landlock_policy.rs`, `landlock.rs`, `seccomp.rs` | one sandboxed request, the macOS profile, the Linux ruleset and the egress channels Landlock cannot reach |
| `http_proxy.rs`, `proxy_audit.rs` | the loopback CONNECT proxy and its decision trail |
| `agent_runtime.rs`, `agent_journal.rs`, `agent_mcp.rs` | the broker, durable run records, granted remote MCP calls |
| `http_client.rs`, `host_process.rs`, `wasm_inspect.rs` | one checked HTTP request, process helpers, module inspection |

porta has no WASM parser of its own: a module is read through the engine that
will run it, so `serve`, `validate` and `inspect` cannot disagree about one.

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

## License

Apache-2.0
