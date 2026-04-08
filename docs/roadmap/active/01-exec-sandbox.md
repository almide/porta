<!-- description: Make porta.exec truly sandboxed and fix command injection in sh -c -->

# porta.exec Sandboxing

**Priority: Critical**

## Problem

1. `porta.exec` claims "Execute a shell command in the sandbox" but actually calls `process.exec()` directly — no sandbox involved
2. `wt_exec_command` uses `sh -c "cd " + cwd + " && exec " + cmd` — command injection via untrusted cwd/cmd
3. MCP builtin exec should route through `wt_exec_sandboxed`, not bare `process.exec`

## Fix

- Route `porta.exec` through `wt_exec_sandboxed` using the instance's preopen_dirs and allowed_net
- Eliminate `sh -c` string concatenation in `wt_exec_command` — use array-based exec
- Pass all sandbox config (dirs, net, env) from InstanceConfig through to the sandboxed execution

## Files
- `src/mcp.almd` — route porta.exec through sandboxed exec
- `src/wasm_rt.almd` — fix command injection in wt_exec_command
