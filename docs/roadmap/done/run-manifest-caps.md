<!-- description: Make run mode respect manifest capabilities like serve does -->
<!-- done: 2026-04-08 -->

# Run Mode Manifest Capabilities

## Fixed

`run_foreground` and `run_as_daemon` now check for a manifest.json alongside the WASM file. If found, its capabilities are used as the baseline (same behavior as `serve`). CLI `--profile` still overrides.

Shared function `cli.resolve_run_caps()` handles the manifest lookup to avoid circular dependencies between engine and ops modules.

## Files
- `src/cli.almd` — resolve_run_caps: manifest-aware capability resolution
- `src/engine.almd` — run_foreground uses resolve_run_caps
- `src/ops.almd` — run_as_daemon uses resolve_run_caps
