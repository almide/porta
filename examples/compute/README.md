# Computation inside WASM

`compute.wasm` runs a bounded Rhai script on JSON data. It supports arithmetic,
loops, filtering, aggregation, string operations and structured transformations.
The Almide chat agent decides when to call it; a fresh WASI instance executes the
script. No filesystem mounts or host services are needed. The tool registers no
file, network, process or environment functions, and disables module loading and
clock functions. File writes remain separately granted tool operations.

## Build and run

With a Rust toolchain containing `wasm32-wasip1`:

```bash
cargo test --locked --manifest-path examples/compute/Cargo.toml --target-dir target/compute
cargo build --locked --manifest-path examples/compute/Cargo.toml --target wasm32-wasip1 --release --target-dir target/compute
cp target/compute/wasm32-wasip1/release/porta-compute.wasm examples/compute/compute.wasm
.tools/almide/almide build examples/chat-agent/src/main.almd --target wasm -o examples/chat-agent/agent.wasm
target/porta agent-check examples/chat-agent/compute.toml
```

Configure the model endpoint and name in
[compute.toml](../chat-agent/compute.toml), then run:

```bash
target/porta agent examples/chat-agent/compute.toml -- 'Use compute to add 0.1 and 0.2. Return the result.'
```

The configuration has no tool mounts. Add a separate `write_file` grant if an
agent should save results, and pass the returned `json` string as its content.
Keep validation/completion checks separate from this model-authored computation:
a successful script can still implement the wrong algorithm. Artifact SHA-256
pins and strict loading work exactly as for other WASM tools.

## Use from other MCP clients

The same WASM tool can be exposed over standard MCP stdio:

```bash
target/porta serve examples/compute/compute.wasm \
  --manifest examples/compute/compute.manifest.json \
  --step-limit 50000000 --max-memory 1024
```

Configure the client to launch that command and grant only `compute`. Use absolute
paths when the client's working directory differs from this repository. No model
configuration is required by this tool server. The manifest permits the WASI
imports emitted by the compiler, including environment enumeration and random
seed initialization; the WASI environment remains empty unless explicitly set.
Those import permissions do not add environment or clock functions to Rhai.
Neither filesystem mounts nor native execution/network capabilities are granted.

The MCP bridge preserves the entire `{"ok":true,"json":"..."}` result in a text
block. `{"ok":false,"error":...}` remains intact and sets `isError: true`. This
fixes the legacy bridge behavior that returned only `true` or `false` and lost
the computation or error details. The old single-key `{"ok":VALUE}` and
`{"err":ERROR}` envelopes keep their payload-unwrapping behavior.

`porta run WASM` and `porta serve WASM` no longer preopen the working directory
implicitly. For other tools needing files, explicitly grant a mount with `-v`;
declaring the `fs` capability alone does not expose the current directory.
The agent broker's existing explicit-mount behavior is unchanged.

The official MCP Python SDK integration verifies exact results, error status,
native-execution denial, and filesystem access with/without an explicit mount:

```bash
python scripts/compute_mcp_integration.py target/porta examples/compute/compute.wasm examples/demo-agent/agent.wasm
```

This test requires `mcp==1.30.0`. The shared evaluation adapter additionally checks
calling the stdio service from an already-running async event loop.

## Input and output

The existing Porta tool ABI is a little-endian u32 length followed by JSON:

```json
{"tool":"compute","arguments":{"script":"input.a + input.b","input_json":"{\"a\":0.1,\"b\":0.2}"}}
```

The response is one JSON object:

```json
{"ok":true,"json":"0.3"}
```

`input_json` and the returned `json` are strings on purpose. Raw JSON numbers in
broker messages may pass through floating-point representations. Keeping the
document as text preserves decimal and large-integer digits all the way to a
separately granted writer. Do not parse those strings into floating-point values
in an intermediary and then expect exact transport.

The variable `input` contains the parsed document; the final expression is the
result. JSON null maps to Rhai unit `()`. Arrays, objects, booleans, strings and
Unicode are supported. Returning a function or another unsupported value is an
error. `print` and `debug` output is discarded to preserve the tool protocol.

```rhai
let total = 0.0;
for row in input {
    if row.include { total += row.amount; }
}
total
```

For `[{"include":true,"amount":4.10},{"include":false,"amount":99},
{"include":true,"amount":-0.30}]`, this returns `{"ok":true,"json":"3.80"}`.
An unrelated JSON transformation can preserve nested values without rewriting
the entire document in model output:

```rhai
input.new_name = input.remove("old_name");
input
```

Rhai is its own language, not Python or JavaScript. The pinned engine enables
[`no_float` and `decimal`](https://rhai.rs/book/language/numbers.html), so decimal
literals use `rust_decimal` instead of binary floating point. Integers use i64;
larger input integers and fractional input numbers must fit the decimal type's
96-bit coefficient and scale of at most 28. Input mantissas are parsed exactly;
scientific notation also requires a representable mantissa and exponent result.
Unsupported input precision/range is rejected, never silently rounded into scope.

Arithmetic still has **finite decimal precision**. Division and other operations
can round; this is not arbitrary-precision mathematics. Integer division
truncates: use `input / 1000.0` when a fractional quotient is needed. Script
literals and explicit conversion functions follow Rhai/rust_decimal semantics;
the strict input-number rule does not make arbitrary script conversions lossless.

## Limits and errors

| Resource | Tool limit |
|---|---:|
| Framed request | 256 KiB |
| Script source | 16 KiB |
| Input JSON / serialized result | 128 KiB each |
| JSON conversion depth / nodes | 32 / 10,000 |
| String value / array / object | 64 KiB / 4,096 elements / 1,024 properties |
| Interpreter operations | 100,000 |
| Script call depth | 32 |
| Expression depth, global / function | 32 / 16 |
| Variables / functions | 256 / 64 |

Byte limits apply in the tool even when JSON Schema lengths count characters.
Normal validation, parsing and script errors return
`{"ok":false,"error":{"code":"compute_failed","message":"..."}}` so the model
can correct its next call. Input strings are data and are not evaluated unless
the model's script explicitly evaluates them.

Porta independently enforces WASM memory, fuel and deadlines. Those host limits
can terminate execution before the interpreter limit; a host trap fails the run
instead of returning a repairable result. Normal journal rules apply, including
refusing to retry an operation with a recorded intent and no result. Each call
gets a new engine and no retained scope; completed results reconstruct on resume
and replay without reevaluating the script.

```bash
python3 scripts/compute_integration.py target/porta examples/chat-agent/agent.wasm examples/compute/compute.wasm examples/demo-agent/agent.wasm
```

CI is configured to run native unit tests and real-WASM integration on Linux and macOS. The tests
cover numeric transport through the broker, nested transformations, rejected
host APIs, malformed inputs, arithmetic errors, separate writing, durable
resume/replay and host fuel exhaustion. Local successful tests do not establish
improved real-task pass rates against Docker Agent; that requires a new matched
comparison with the additional tool available to both runtimes.

A [local-model capability smoke](../../docs/benchmarks/compute-smoke.md) records
two correct computations out of three tasks, including a failed transformation
and every preceding tool error. It is separate from the comparative benchmark.
