<!-- description: Make run mode respect manifest capabilities like serve does -->

# Run Mode Manifest Capabilities

**Priority: High**

## Problem

`serve` reads manifest and uses `resolve_serve_caps(profile, m.capabilities)`. But `run_foreground` and `run_as_daemon` use `resolve_caps(profile, [])` — manifest capabilities are ignored.

## Fix

- In `run_foreground` and `run_as_daemon`, check for manifest.json alongside the wasm file
- If manifest exists, use its capabilities as the baseline (same as serve)
- CLI `--profile` still overrides

## Files
- `src/engine.almd` — run_foreground reads manifest
- `src/ops.almd` — run_as_daemon reads manifest
