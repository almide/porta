<!-- description: Put the execution engine behind a seam so it can be replaced -->
# Replaceable WASM Engine

Make the execution engine a replaceable component instead of a fixed one, so a
second engine can be added without touching the broker, journals or MCP layers.
This is the seam, not a rewrite: `done/wasmtime-migration.md` chose wasmtime for
good reasons and nothing here proposes dropping it.

This is unrelated to the wasmtime CLI that `almide test` uses. That one is
already optional: Almide falls back to a native build when it is absent, both
paths run the same tests, and CI treats a failed download as slow rather than
red.

## Measured coupling

The engine is already close to isolated. Of 2118 lines of native code:

| File | wasmtime references |
|---|---:|
| `native/wasmtime_bridge.rs` (1069 lines) | 7 |
| `native/agent_runtime.rs` | 4 |
| `native/agent_mcp.rs` | 0 |
| `native/agent_journal.rs` | 0 |

The API surface is about ten types: `Engine`, `Module`, `Store`, `Linker`,
`Func`, `Memory`, `Config`, `WasiCtxBuilder`, `DirPerms`/`FilePerms`,
`StoreLimits`.

## What an engine has to provide

The obstacle is not the API, it is that the security invariants are written
against engine features:

- Fuel metering (`consume_fuel`, `set_fuel`, `get_fuel`). Recorded as `steps=`
  in the published measurements.
- Memory ceilings through a store limiter.
- WASI preopen permissions, which the mount policy maps onto.
- A refusal to deserialize precompiled modules, so an agent-writable sidecar can
  never become native code.

An engine that cannot express all four changes what `docs/` is allowed to claim,
not just how the bridge is written.

## Candidates

| Engine | Fuel | WASI | Note |
|---|---|---|---|
| wasmi | yes | yes | interpreter; the realistic second engine for hosts that forbid JIT, and much slower |
| Wasmer | via middleware | yes | lateral move; no motivating requirement |
| WasmEdge | different model | yes | higher integration cost |

## Why wait

No requirement drives it yet. The history also argues for care: a hand-rolled
interpreter (`done/wasm-interpreter.md`) was replaced precisely because two
independent implementations of one spec drift apart. A second engine must
therefore be validated against wasmtime's output rather than trusted, which is
the same bar `on-hold/self-hosted-wasm-runtime.md` sets for itself.

Take this up when something concrete asks for it: a target that forbids JIT, a
platform wasmtime does not support, or the self-hosted runtime becoming real —
that one needs this seam too, so doing it first makes that item smaller.

## Measurement caveat

Fuel semantics are engine-specific. Swapping engines makes previously published
`steps=` figures incomparable, so the seam has to record which engine produced a
measurement, and `docs/benchmarks/` has to carry that field before a second
engine is ever used for a published number.
