<!-- description: CLI entry point with serve, inspect, validate subcommands -->
# CLI

Command-line interface for porta.

## Subcommands

- `porta serve agent.wasm` — Start MCP server on stdio
  - `--dir <path>` — Pre-open directory for WASI filesystem access (repeatable)
  - `--env <KEY=VALUE>` — Pass environment variable to agent (repeatable)
  - `--timeout <duration>` — Per-tool-call timeout (default 30s)
  - `--max-memory <size>` — Memory limit per instance (default 256MB)
- `porta inspect agent.wasm` — Print manifest in human-readable form
- `porta validate agent.wasm` — Check manifest ↔ binary consistency

## Module

- `main.almd` — Argument parsing, subcommand dispatch
