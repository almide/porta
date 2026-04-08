<!-- description: Make native sandbox truly deny-by-default for network and filesystem -->

# Native Sandbox Deny-by-Default

**Priority: High**

## Problem

`build_sandbox_profile()` starts with `(allow default)`, which permits everything not explicitly denied. This contradicts porta's "deny-by-default" security message.

Specific gaps:
- **Network**: When `allowed_net` is empty, all outbound traffic is allowed (no deny rule emitted)
- **FS read**: `(allow default)` means all files are readable; only a deny-list of sensitive dirs is applied
- **Exec**: No command whitelist; any binary can be spawned

## Fix

### Network
When `--allow-net` is NOT specified, default to **deny all outbound**:
```
if list.len(allowed_net) == 0 then
  p + "(deny network-outbound)\n(allow network-outbound (local udp))\n"
```

### FS Read
Move from deny-list to allow-list where feasible. At minimum, clearly document that `(allow default)` means read access is broad.

### Command whitelist
Add `--allow-exec cmd1,cmd2` that restricts which binaries `porta.exec` and child processes can run.

## Files
- `src/wasm_rt.almd` — fix build_sandbox_profile default network behavior
- `src/mod.almd` — add --allow-exec flag, --deny-net flag
- `src/mcp.almd` — enforce command whitelist in porta.exec
