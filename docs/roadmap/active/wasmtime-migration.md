<!-- description: Replace hand-rolled WASM interpreter with wasmtime via FFI -->
# Replace Interpreter with wasmtime

## Motivation

porta's hand-rolled WASM interpreter (`interp.almd`, `binary.almd`, `memory.almd`) reimplements the WASM spec independently from Almide's codegen. This has caused recurring bugs:

- `read_memarg` doesn't parse multi-memory encoding → string interpolation trap
- `pop_n` reverses function arguments → concat order reversal
- Opcode table has wrong mappings (0xAE vs 0xB0 for `i64.trunc_f64_s`)

These are symptoms of a structural problem: two independent implementations of the same spec will always drift apart.

## Architecture

```
porta (Almide)
  └── wasm_runtime (extern FFI → wasmtime C API)
        - instantiate(bytes, config) → instance
        - call(instance, func, args) → result
        - WASI host functions with capability filtering
        - fuel limit (step limit)
        - memory limit
```

### What stays

- `binary.almd` — simplified to only parse module structure (exports, imports, memory declarations) for inspect/manifest. No instruction-level parsing needed.
- All MCP / orchestration / sandbox policy logic

### What goes

- `interp.almd` — replaced entirely by wasmtime
- `memory.almd` — wasmtime manages linear memory
- Instruction parsing in `binary.almd` (`read_instr`, `read_fc_instr`, `read_fd_instr`)

## Implementation Plan

1. **Create `almide/wasmtime` package** — Almide extern FFI wrapper for wasmtime's C API (`wasmtime.h`)
   - `wasm_engine_new`, `wasm_store_new`, `wasm_module_new`, `wasm_instance_new`
   - WASI configuration (stdin/stdout/stderr, fs preopens, env vars)
   - Fuel-based step limiting (`wasmtime_store_set_fuel`)
   - Memory limiting (`wasmtime_store_limiter`)

2. **Wire porta's capability model to wasmtime's WASI** — Map porta's `CapabilitySet` to wasmtime's WASI configuration (which functions are available, which paths are preopened)

3. **Remove interpreter** — Delete `interp.almd`, `memory.almd`, instruction parsing from `binary.almd`

4. **Validate** — Run all existing tests + Almide's full WASM test suite through porta

## Future: Self-hosted WASM Runtime

Long-term goal: build an Almide-native WASM runtime that replaces wasmtime. This should happen after:

- Almide's type system and codegen are stable
- Performance-critical primitives (memory ops, JIT compilation) can be expressed efficiently in Almide
- The runtime can be verified against wasmtime's output as a reference

Until then, wasmtime is the correct choice — it's battle-tested and spec-complete.
