<!-- description: Tool call dispatch from MCP to WASM agent execution -->
# Tool Call Dispatch

Bridge between MCP protocol layer and WASM interpreter. Handles per-call instance lifecycle.

## Responsibilities

- Create fresh WASM instance per `tools/call`
- Serialize tool arguments as JSON → agent stdin (length-prefixed)
- Read agent stdout → deserialize JSON result
- Map `{"ok": ...}` / `{"err": ...}` to MCP response format
- Capture stderr for diagnostics
- Enforce timeout and memory limits
- Clean up instance after each call (no state leaks)

## Module

- `dispatch.almd` — Instance lifecycle, stdin/stdout pipe protocol
