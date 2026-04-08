<!-- description: Remove or implement config options that exist but don't work -->
<!-- done: 2026-04-08 -->

# Config Honesty

**Priority: High**

## Problem

Several config/flag options are declared but have no effect.

## Fixed

| Option | Fix |
|--------|-----|
| `max_memory_pages` | Wired to wasmtime StoreLimits via wt_set_max_memory |
| `entry` | Passed to wasmtime via wt_set_entry, no longer hardcoded to _start |
| `cwd` in wt_exec_command | Shell wrapper for non-default cwd |
| `env_json`/`cwd` in wt_exec_sandboxed | /usr/bin/env for env vars, sh -c wrapper for cwd |
| `secrets.from-env` in config.almd | Reads from host environment when {from-env = true} |

## Files
- `native/wasmtime_bridge.rs` — PortaCtx wrapper, wt_set_max_memory, wt_set_entry, entry point from config
- `src/wasm_rt.almd` — extern declarations, cwd/env in exec functions
- `src/dispatch.almd` — configure_instance helper wires max_memory and entry
- `src/config.almd` — from-env secret resolution
