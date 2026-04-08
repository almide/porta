<!-- description: Remove or implement config options that exist but don't work -->

# Config Honesty

**Priority: High**

## Problem

Several config/flag options are declared but have no effect:

| Option | Status | Action |
|--------|--------|--------|
| `max_memory_pages` | In InstanceConfig, not enforced at runtime | Wire to wasmtime fuel/memory limit |
| `entry` | In config, not passed to wt_run | Pass to wt_set_args or wasmtime entry point |
| `cwd` in wt_exec_command | Accepted but ignored | Use `process.exec` with cwd support, or remove param |
| `env_json`/`cwd` in wt_exec_sandboxed | Accepted but ignored | Pass to sandbox-exec via env injection |
| `secrets.from-env` in config.almd | Comment exists, not implemented | Implement or remove |

## Principle

**If a flag/config exists, it must work. If it can't work yet, remove it.**

No phantom options. Users must be able to trust that every declared setting has an effect.

## Files
- `src/dispatch.almd` — wire max_memory_pages, entry
- `src/wasm_rt.almd` — fix cwd/env in exec functions
- `src/config.almd` — implement or remove from-env secrets
- `native/wasmtime_bridge.rs` — pass entry point, memory limits to wasmtime
