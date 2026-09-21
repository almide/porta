# Architecture

Two sides. Almide decides policy; Rust applies it and talks to the kernel.

## `src/` — Almide

The CLI, the MCP protocol, capability checks, and what a run is allowed to do.

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

## `native/` — Rust

Wasmtime, the OS enforcement, and the broker that holds model credentials.

| | |
|---|---|
| `wasmtime_bridge.rs` | WASM instance lifecycle and the FFI surface |
| `sandbox_exec.rs`, `sandbox_profile.rs`, `landlock_policy.rs`, `landlock.rs`, `seccomp.rs` | one sandboxed request, the macOS profile, the Linux ruleset and the egress channels Landlock cannot reach |
| `sandbox_check.rs`, `denials.rs` | what this host can enforce; what the kernel refused during a run and which flag would have allowed it |
| `http_proxy.rs`, `proxy_egress.rs`, `proxy_audit.rs` | the loopback CONNECT proxy, its egress guard, and its decision trail |
| `agent_runtime.rs`, `agent_journal.rs`, `agent_mcp.rs` | the broker, durable run records, granted remote MCP calls |
| `http_client.rs`, `host_process.rs`, `wasm_inspect.rs` | one checked HTTP request, process helpers, module inspection |

porta has no WASM parser of its own: a module is read through the engine that
will run it, so `serve`, `validate` and `inspect` cannot disagree about one.

Almide `@extern(rs, "<module>", …)` declarations resolve against
`wasmtime_bridge` and `agent_runtime`, which re-export the rest; a `pub use`
satisfies the binding just as a definition does.
