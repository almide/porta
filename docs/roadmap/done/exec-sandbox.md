<!-- description: Make porta.exec truly sandboxed and fix command injection in sh -c -->
<!-- done: 2026-04-08 -->

# porta.exec Sandboxing

## Fixed

- `porta.exec` now routes through `wt_exec_sandboxed` with instance's preopen_dirs, allowed_hosts, and env
- Eliminated `sh -c` string concatenation from both `wt_exec_command` and `wt_exec_sandboxed` — no command injection
- All exec paths use array-based argument passing

## Files
- `src/mcp.almd` — handle_builtin_exec passes config to wt_exec_sandboxed
- `src/wasm_rt.almd` — removed sh -c from wt_exec_command and wt_exec_sandboxed
