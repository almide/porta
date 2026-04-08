<!-- description: Migrate Rust bridge functions to pure Almide where possible -->

# Full Almide Migration

Move non-wasmtime Rust code from `native/wasmtime_bridge.rs` to pure Almide. Reduce Rust surface area to wasmtime API calls only.

## Currently in Rust (can move to Almide)

| Function | Lines | Migration path |
|----------|-------|----------------|
| `wt_http_request` | ~40 | Almide HTTP stdlib (when available) |
| `wt_exec_command` | ~25 | `process.exec(cmd, args)` |
| `wt_exec_sandboxed` | ~80 | `process.exec("sandbox-exec", ["-p", profile, ...])` |
| `wt_home_dir` | ~3 | `process.exec("sh", ["-c", "echo $HOME"])` |
| `wt_spawn` | ~15 | Needs Almide process spawn support |
| `wt_kill` | ~8 | Needs Almide signal support |

## Must stay in Rust (wasmtime API)

| Function | Reason |
|----------|--------|
| `wt_create` | Engine, Module, compilation cache |
| `wt_run` | Store, Linker, WASI context, host function injection |
| `wt_set_*` / `wt_get_*` | Instance pool mutation |
| `wt_inspect` | Module import/export reflection |
| `wt_destroy` | Instance cleanup |
| linker host functions | `porta.http_request`, `porta.exec_command` in wasmtime |

## Prerequisites

- Almide HTTP stdlib module (for `wt_http_request` migration)
- Almide process spawn without wait (for `wt_spawn`)
- Almide signal sending (for `wt_kill`)

## Goal

`native/wasmtime_bridge.rs` should contain ONLY wasmtime API calls (~200 lines). Everything else in Almide.
