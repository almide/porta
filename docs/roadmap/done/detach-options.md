<!-- description: Forward all CLI options to detached daemon child process -->
<!-- done: 2026-04-08 -->

# Detach Option Forwarding

## Fixed

`run_detached()` now serializes all Options into the child process args: profile, entry, step-limit, max-memory, restart, manifest, env-file, env vars, secrets, preopen dirs, allow-net, allow-exec, and wasm args.

## Files
- `src/ops.almd` — serialize_opts helper, updated run_detached
