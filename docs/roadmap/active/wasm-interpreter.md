<!-- description: WASM interpreter for Almide-compiled agent binaries -->
# WASM Interpreter

Pure Almide implementation of a WASM interpreter. Only needs to support the instruction subset that Almide's `--target wasm` codegen emits.

## Modules

- `wasm/binary.almd` — .wasm binary format parser (magic, version, sections, LEB128)
- `wasm/memory.almd` — Linear memory management (load/store, grow, dual memory)
- `wasm/interp.almd` — Instruction interpreter (stack machine execution)
- `wasm/validate.almd` — Module validation (type checking, import verification)
- `wasm/wasi.almd` — WASI Preview 1 host function implementations

## Scope

Not a general-purpose WASM runtime. Supports:
- Almide-emitted instructions only (no SIMD, no threads, no GC proposal)
- Tail calls (`return_call` / `return_call_indirect`)
- Multi-memory (memory 0 + memory 1 scratch buffer)
- WASI Preview 1 (fd_read, fd_write, path_open, etc.)

## Non-goals

- JIT/AOT compilation
- Full WASM spec compliance
- Component Model / WIT
