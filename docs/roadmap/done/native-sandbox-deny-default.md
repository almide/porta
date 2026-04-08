<!-- description: Make native sandbox truly deny-by-default for network and filesystem -->
<!-- done: 2026-04-08 -->

# Native Sandbox Deny-by-Default

**Priority: High**

## Problem

`build_sandbox_profile()` starts with `(allow default)`, which permits everything not explicitly denied. Network was only restricted when `--allow-net` was explicitly set — with no flag, all outbound traffic was allowed.

## Fix

### Network
Network outbound is now always denied by default. Only `(local udp)` (DNS) and `(remote unix-socket)` are permitted. Explicit `--allow-net` entries add specific TCP port allowances.

### FS
macOS sandbox-exec requires `(allow default)` for system library access. FS write is deny-by-default with explicit allow-list. FS read uses deny-list for sensitive directories (~/.ssh, ~/.gnupg, ~/.aws, etc.).

### Command whitelist
Already implemented in 01-builtin-tool-security via `--allow-exec` in mcp.almd.

## Files
- `src/wasm_rt.almd` — network deny-by-default in build_sandbox_profile
