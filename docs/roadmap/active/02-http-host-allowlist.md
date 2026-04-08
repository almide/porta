<!-- description: Make porta.http allowlist check hostname, not just port -->

# porta.http Host-Based Allowlist

**Priority: Critical**

## Problem

`is_host_allowed()` only checks port, not hostname. `["api.example.com:443"]` allows ANY https host. Empty `allowed_hosts` with `CapNet` allows everything — contradicts network deny-by-default.

## Fix

- Parse URL to extract hostname and port
- Match against `host:port` patterns in allowed list
- `*:443` means any host on port 443, `api.example.com:443` means only that host

## Files
- `src/mcp.almd` — rewrite is_host_allowed with URL parsing
