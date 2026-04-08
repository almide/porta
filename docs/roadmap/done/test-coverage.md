<!-- description: Add security-focused tests and integration tests for all enforcement paths -->
<!-- done: 2026-04-08 -->

# Test Coverage for Security Paths

**Priority: Medium**

## Added Tests (16 new, 74→90 total)

### Sandbox profile (wasm_rt_test.almd, +5)
- Empty allowed_net → deny all outbound
- allowed_net → TCP allow rules
- :ro mount → no file-write rule
- Writable mount → file-write rule
- Sensitive dirs → deny-read rules

### MCP security enforcement (mcp_test.almd, +11, new file)
- is_host_allowed: https/443, wildcard port, wrong port, empty list, multiple entries
- porta.exec: ai-agent denied, full allowed, from_manifest
- porta.http: ai-agent denied, worker denied, from_manifest

## Files
- `src/wasm_rt_test.almd` — sandbox profile generation tests
- `src/mcp_test.almd` — new file for URL filtering and capability enforcement tests
