<!-- description: Enforce capability checks on porta.exec and porta.http builtin tools -->
<!-- done: 2026-04-08 -->

# Builtin Tool Security

**Priority: Critical**

## Problem

`porta.exec` and `porta.http` MCP builtin tools bypass the capability system entirely. They call `process.exec` and `http.request` directly without checking `CapExec` or `CapNet` against the instance config.

A manifest declaring `capabilities: ["io"]` should NOT be able to call `porta.exec`, but currently can.

## Fix

### porta.exec
- Check `CapExec` before executing
- Check command against `--allow-exec` whitelist (new config field)
- Enforce `cwd` restriction to preopen dirs only

### porta.http
- Check `CapNet` before executing
- Check URL against `--allow-net` host/port list
- Reject requests to non-allowed hosts

### Implementation
In `mcp.almd` `handle_tools_call`:
```
"porta.exec" => {
  if not sandbox.has_capability(server.config.capabilities, sandbox.CapExec)
    then error response
  if not is_command_allowed(server.config, cmd)
    then error response
  execute
}
```

## Files
- `src/mcp.almd` — add capability checks to builtin handlers
- `src/dispatch.almd` — add `allowed_commands: List[String]` to InstanceConfig
- `src/sandbox.almd` — add URL validation helper
