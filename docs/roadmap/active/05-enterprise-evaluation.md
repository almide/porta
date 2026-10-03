<!-- description: Porta Enterprise evaluation build: job API, run records, container -->
# Porta Enterprise evaluation build

Working name: Porta Enterprise. Annual licence plus updates and support, with
limited paid deployment help. Price, concurrency tiers, evaluation period and
licence enforcement are undecided; nothing for billing, expiry or remote
shutdown is built ahead of that decision.

## Smallest unit that proves the product

One environment (a Linux container, or a macOS/Linux host), one use: an
operator runs `porta job-serve` with a policy file; a client submits a job —
an operator-registered, hash-pinned WASM module, its input, the files and
directories it may touch, and its limits — and gets back the result, why it
stopped, and a run record.

Why this unit: Porta already enforces WASM isolation (wasmtime, WASI preopens,
fuel, memory, epoch deadline) and native process isolation. What a customer
cannot evaluate today is a service boundary: an API that takes untrusted work
and a permission set, and returns evidence. `porta agent` needs a model
endpoint, so it is the second step, not the first.

## Plan

1. Job spec v1 and policy v1: what a job may ask for, what the operator allows,
   limit ceilings, the version and digest of the policy applied.
2. `porta job-run` (one job, one process) and `porta job-check` (validation
   only), both on the same code path the server uses.
3. Each job in its own worker process: wasmtime with fuel, a memory ceiling,
   an epoch deadline, WASI preopens only for granted directories, no network,
   no host functions beyond WASI; the supervisor kills the process at a hard
   deadline or on cancel.
4. Run records: run id, policy version and digest, module digest, start/end,
   outcome, stop reason, resource use; input and output contents only as
   digests and sizes unless retention is configured. Restart marks unfinished
   runs `interrupted` and never re-runs them.
5. `porta job-serve`: a minimal HTTP API (submit, get with wait, cancel, list,
   delete, policy, health), bearer token required.
6. Sample job and evaluation script: success, refused grant, denied access
   inside the sandbox, infinite loop stopped, memory ceiling, cancellation,
   concurrency isolation, restart handling.
7. Container image, local container test, cloud templates (unverified on real
   clouds), threat model, licence inventory, gaps ranked by evidence.

Status of each step lives in `docs/enterprise/`.
