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

## Compiler Blockers

1. **`import http` + `@extern(rs)` で `Write` trait 重複** — codegen の use dedup 漏れ
2. **`json.as_map` 未定義** — Value から Map への変換 stdlib 関数が必要
3. **`process.pid()` 未定義** — 現在のプロセスの PID 取得が必要
4. **`effect fn` → 非 effect 呼び出しの型不一致** — effect fn が Result を返すが、既存の呼び出し側が plain 値を期待

全てコンパイラ側で対応すれば移行可能。

## Goal

`native/wasmtime_bridge.rs` should contain ONLY wasmtime API calls (~200 lines). Everything else in Almide.
