<!-- description: Make porta.http allowlist check hostname, not just port -->
<!-- done: 2026-04-08 -->

# porta.http Host-Based Allowlist

## Fixed

`is_host_allowed()` now parses URL to extract hostname and port, matching against `host:port` patterns:
- `api.example.com:443` — only that specific host on HTTPS
- `*:443` — any host on HTTPS
- `localhost:8080` — specific host and port
- `api.example.com:*` — that host on any port

## Files
- `src/mcp.almd` — rewritten is_host_allowed with URL host+port parsing
- `src/mcp_test.almd` — 7 host-based allowlist tests
