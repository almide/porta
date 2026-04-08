<!-- description: Align native sandbox messaging with actual enforcement level -->
<!-- done: 2026-04-08 -->

# Sandbox Honesty

## Fixed

- `porta.exec` tool description changed from "Execute a shell command in the sandbox" to "Execute a command with filesystem and network restrictions"
- `build_sandbox_profile` documented as constrained environment, not full deny-by-default sandbox
- CLAUDE.md description ("secure MCP bridge") is accurate — refers to WASM sandbox boundary, not native sandbox

## Files
- `src/mcp.almd` — accurate tool description
- `src/wasm_rt.almd` — honest documentation on sandbox profile
