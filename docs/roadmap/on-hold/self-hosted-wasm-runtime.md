<!-- description: Build Almide-native WASM runtime to eventually replace wasmtime -->
# Self-Hosted WASM Runtime

Build a WASM runtime written in Almide that can replace wasmtime as porta's execution engine.

## Prerequisites

- Almide's type system and codegen are stable (no major breaking changes)
- `extern` FFI is mature enough to implement performance-critical memory operations
- wasmtime is running in production as a reference implementation to validate against

## Scope

- WASM 2.0 core spec (MVP + multi-memory, bulk memory, reference types)
- WASI preview1 (sufficient for agent use case)
- Interpreter-first, JIT later
- Validation against wasmtime output for correctness

## Why Wait

The current hand-rolled interpreter proved that Almide can express a WASM VM. But shipping it prematurely caused bugs that blocked agent development. The right approach: use wasmtime now, build the self-hosted runtime when the language is mature enough to do it properly.
