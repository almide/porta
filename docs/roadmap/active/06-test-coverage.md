<!-- description: Add security-focused tests and integration tests for all enforcement paths -->

# Test Coverage for Security Paths

**Priority: Medium**

## Problem

Current 69 tests cover happy paths but not security enforcement:
- No test that `porta.exec` is denied when CapExec is missing
- No test that `porta.http` is denied when CapNet is missing
- No test for import validation on real WASM files
- No test for sandbox profile generation
- No integration test for MCP tools/call → sandbox enforcement

## New Tests Needed

### Security enforcement
- `porta.exec` with CapExec missing → error
- `porta.http` with CapNet missing → error
- `porta.exec` with command not in whitelist → error
- `porta.http` with URL not in allow-net → error

### Import validation
- Real WASM with FS imports + ai-agent profile → FAIL
- Real WASM with IO-only imports + ai-agent profile → PASS

### Sandbox profile
- Empty allowed_net → deny all outbound in profile
- `:ro` mount → no file-write rule in profile
- Sensitive dirs → deny-read rules present

### Integration (MCP e2e)
- tools/call porta.exec → actually runs + returns output
- tools/call porta.exec with denied cap → error response
- tools/call agent tool → dispatches to WASM correctly

## Files
- `src/sandbox_test.almd` — security enforcement tests
- `src/mcp_test.almd` — new file for MCP integration tests
