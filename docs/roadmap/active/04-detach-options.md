<!-- description: Forward all CLI options to detached daemon child process -->

# Detach Option Forwarding

**Priority: High**

## Problem

`run_detached()` only passes `path` and `profile` to the daemon child. All other options are dropped: --env, --secret, --step-limit, --max-memory, --restart, -v, --allow-net, --allow-exec, -- args.

## Fix

Serialize the full Options into child process args so the daemon child has identical config.

## Files
- `src/ops.almd` — serialize all options in run_detached
