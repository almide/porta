# CLI reference

Every command, flag, config key and exit code. For what Porta is and what to
reach for it, start at the [README](../README.md); for what each platform
actually enforces, see [enforcement.md](enforcement.md).

Everything before `--` is an option for porta; everything after it belongs to
the command. An argument in the wrong place is refused with the fixed command
line, never dropped.

## Commands

### Project

| Command | Description |
|---------|-------------|
| `porta init [native\|wasm] [cmd]` | Create porta.toml |
| `porta up [-- args...]` | Run from porta.toml |

### Runtime

| Command | Description |
|---------|-------------|
| `porta run <target>` | Execute a `.wasm` module or a native command |
| `porta explain <command> [options]` | Print the policy `run` would apply with the same options, and run nothing |
| `porta check` | Say what this host can enforce and what porta will refuse here |
| `porta agent <agent.toml> [--record <journal>] -- <task>` | Run a WASM agent or team |
| `porta agent-resume <agent.toml> <journal>` | Continue a recorded run |
| `porta agent-replay <agent.toml> <journal>` | Verify a completed run offline |
| `porta agent-journal <journal>` | Read-only run metadata, no code loaded |
| `porta run -d <agent.wasm>` | Run WASM as a background daemon |
| `porta serve <agent.wasm>` | Start an MCP server on stdio |

### Development

| Command | Description |
|---------|-------------|
| `porta build <agent.wasm>` | Generate manifest.json |
| `porta inspect <agent.wasm>` | Show module info |
| `porta validate <agent.wasm>` | Check WASI imports against the profile |

### Instances

| Command | Description |
|---------|-------------|
| `porta ps` | List instances |
| `porta stop <id>` | Stop instance (SIGTERM) |
| `porta kill <id>` | Kill instance (SIGKILL) |
| `porta logs <id>` | View instance logs |
| `porta rm <id>` | Remove a stopped instance |

## Options

| Flag | Description |
|------|-------------|
| `-e`, `--env <KEY=VALUE>` | Set an environment variable |
| `--env-file <path>` | Load env vars from a file |
| `--secret <KEY=VALUE>` | Inject a secret as an env var |
| `-v <path>` | Mount a directory (writable) |
| `-v <path>:ro` | Mount a directory (read-only) |
| `--allow-net <host:port>` | Allow outbound TCP by port (repeatable). The host part is not enforced at the OS layer — use `--proxy-allow` for that |
| `--proxy-allow <hosts>` | Route egress through porta's CONNECT proxy and allow only these hosts |
| `--proxy-deny <hosts>` | Same, denying these hosts |
| `--proxy-audit <path>` | Append every proxy decision to a JSONL file |
| `--read-policy <open\|strict>` | `strict` confines reads to your mounts and the system directories (default `open`) |
| `--allow-root` | Run as root anyway. Refused by default: for root, the file permissions this policy leans on separate nothing |
| `--env-pass <NAME,...>` | Copy these host variables into the command. The child starts from an empty environment plus `PATH`, `HOME`, `USER`, `SHELL`, `TERM` and the locale; nothing else of your shell crosses unless `-e` or this names it |
| `--allow-unix <path>` | Let the command connect to this Unix socket. The SSH agent, gpg-agent and the container runtimes' sockets are closed by default (repeatable) |
| `--no-net` | No network at all: no TCP or UDP to anywhere, and not the host's loopback either. On Linux the command gets a network namespace of its own holding only a loopback interface it can use itself; where the host refuses one, Landlock closes every TCP port and seccomp every other socket family, and a kernel that can do neither refuses the run. Refused beside `--allow-net`, `--allow-bind` or a proxy |
| `--allow-bind <port>` | Let the command listen on this TCP port. Once `--allow-net` is in force, a granted port is a port to reach, not one to serve on (repeatable) |
| `--timeout <secs>` | Kill the command and everything it started after this many seconds, reporting exit 124. `0` (the default) sets no limit |
| `--max-cpu <secs>` | CPU seconds each process may use before the kernel ends it with SIGXCPU (exit 152). Inherited by everything the command starts. Ignoring the signal buys nothing: Linux kills at the hard limit a second later, and on macOS porta measures the process group's CPU and kills it at the ceiling |
| `--max-procs <n>` | Process ceiling while the command runs; a fork past it fails. On Linux, inside the command's own namespaces, it counts the command's processes (the command itself included); where the host refuses the namespaces, and on macOS, the kernel counts every process of your user, so set it above what you already have |
| `--max-file-size <MiB>` | Largest file the command may write; the write past it ends the process with SIGXFSZ (exit 153) and the file stops there |
| `--max-memory-mb <MiB>` | Resident memory for the command and everything it starts, together. Linux: a cgroup v2 ceiling with swap closed, placed through the systemd user manager, and the kernel OOM-kills the run past it (exit 137); needs a user manager, refused otherwise. macOS: porta polls the group's footprint every quarter second and ends it at the ceiling (exit 137), which a burst can pass briefly |
| `--allow-exec <cmd,...>` | Allow specific commands (comma-separated) |
| `--profile <name>` | Capability profile: `ai-agent`, `worker`, `full` |
| `--step-limit <n>` | Max WASM instructions |
| `--max-memory <pages>` | Max WASM memory pages |
| `--restart <policy>` | `no`, `on-failure`, `always` |
| `-d`, `--detach` | Run as a background daemon |
| `--json` | Machine-readable output for `explain` and `check` |
| `--save <path>` | `explain --save` writes these flags as a `porta.toml` you can commit |
| `--help`, `-h` | Show help for any command |

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
# no-net = true           # No network at all (instead of network)
# read-policy = "strict"  # Confine reads to mounts + system dirs
# timeout = 300           # Kill the run after N seconds (exit 124)
# max-cpu = 60            # CPU seconds per process (SIGXCPU past it)
# max-procs = 500         # Process ceiling for your user; stops fork bombs
# max-file-size = 100     # Largest file, in MiB (SIGXFSZ past it)
# max-memory-mb = 512     # Resident memory for the whole run (Linux, cgroup v2)
# env-pass = ["CI"]       # Copy these host variables in by name
# unix = ["/run/…"]       # Credential sockets the command may reach
# bind = ["8080"]         # TCP ports the command may listen on

[proxy]
# allow = ["api.example.com"]   # egress only through the CONNECT proxy, these hosts
# audit = "egress.jsonl"

[env]
NODE_ENV = "production"

[secrets]
API_KEY = "literal-value"
# Or read from the host environment:
# API_KEY = { from-env = true }
```

```bash
porta init native claude   # Generate porta.toml
porta up                   # Run from porta.toml
porta up -- --print "hi"   # Pass arguments to the command
```

`porta explain <command> [flags] --save porta.toml` writes the flags you used
as a `porta.toml`, so an invocation you converged on becomes the project's
checked-in policy. Secrets and `-e` values are left out on purpose.

## When a run is refused

A refusal looks exactly like a broken tool — `Operation not permitted` — until
someone says which flag it would have needed. After a run that exits non-zero,
porta reads the kernel's denial records for that run (macOS; each deny rule
carries a per-run tag, so other processes' denials are not mixed in) and says
so:

```
[porta] the sandbox refused this run 2 times; what each would have needed:
  file-write-create /Users/me/notes/out.txt
    → -v /Users/me/notes
  network-outbound remote:*:443
    → --allow-net '*:443'
```

Some refusals have no flag — a credential store, another process's arguments,
`open(1)` — and the footer says that instead. The footer is read from the
unified log, which macOS writes asynchronously; porta asks it a few times over
a few seconds, and on a heavily loaded machine a denial can still arrive after
that, in which case the footer is missing for that run, never wrong. `PORTA_DENIALS=always` asks after
every run, including ones that exited 0; `PORTA_DENIALS=never` keeps the footer
away. On Linux the footer is not available yet: it needs Landlock ABI 7's audit
records.

Exit codes tell a script what happened:

| Exit | Meaning |
|---|---|
| the command's own | the command ran; this is what it returned (128 + signal if a signal ended it) |
| 124 | the run hit its `--timeout` and was killed |
| 152, 153 | the kernel ended the command at its `--max-cpu` (SIGXCPU) or `--max-file-size` (SIGXFSZ) ceiling |
| 137 | the kernel killed the command: past its `--max-memory-mb` ceiling, or past the CPU hard limit after it ignored SIGXCPU |
| 125 | porta refused the run before it began — a rule this kernel cannot express, a missing mount, root without `--allow-root` |
| 126 | the command exists but the policy leaves it unrunnable (an interpreter outside the strict read set, say) |
| 127 | the command was not found |

`porta explain <command> [same options]` prints the policy a run would apply
without applying it; `porta check` prints what this host can enforce at all.
Add `--json` to either for a machine-readable form: `explain --json` gives the
effective policy (command, mounts, reads, network, listen ports, Unix sockets,
timeout, resource limits, backend), or `{"refused": "..."}` for a run porta would decline;
`check --json` gives the host's primitives and whether each is present. Both
let a CI step gate on the policy without parsing prose.

## MCP server

```bash
porta serve agent.wasm --profile full
```

### Built-in tools

| Tool | Requires | Description |
|------|----------|-------------|
| `porta.exec` | `CapExec` + `--allow-exec` | Execute a command with filesystem and network restrictions |
| `porta.http` | `CapNet` + `--allow-net` | Make HTTP requests to allowed hosts |
| Agent tools | — | Dispatched to the WASM agent |

`porta.http` accepts HTTP(S) URLs without userinfo and does not follow
redirects or inherit host proxy settings.

### Supported MCP methods

`initialize`, `tools/list`, `tools/call`, `resources/list`, `resources/read`,
`prompts/list`, `prompts/get`, `ping`

### Claude Code integration

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

## Build from source

```bash
# Almide 0.63.0, a Rust toolchain, Python 3, curl
bash scripts/install-almide.sh
.tools/almide/almide build src/mod.almd -o target/porta
.tools/almide/almide test --ci
python3 scripts/integration.py target/porta
cp target/porta ~/.local/bin/
```

The compiler target is **0.63.0**. As of 2026-09-19 the published artifact is
`v0.63.0-rc1` (reports `almide 0.63.0`); the installer pins that release and
checks its published SHA-256. Once the final tag is published, select it with
`ALMIDE_RELEASE_TAG=v0.63.0 bash scripts/install-almide.sh`.
