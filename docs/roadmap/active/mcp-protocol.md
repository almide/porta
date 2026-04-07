<!-- description: MCP protocol implementation over JSON-RPC 2.0 stdio -->
# MCP Protocol Implementation

JSON-RPC 2.0 / stdio transport implementing MCP spec (2025-03-26).

## Modules

- `jsonrpc.almd` — JSON-RPC 2.0 parser/serializer with Content-Length framing
- `mcp.almd` — MCP protocol state machine (initialize → tools/list → tools/call → ping)
- `manifest.almd` — manifest.json parser and tool definition extraction

## Protocol Methods

- `initialize` / `notifications/initialized`
- `tools/list` (with cursor pagination)
- `tools/call` (dispatch to WASM agent)
- `ping`

## Transport

stdio only (v1). Streamable HTTP is future scope.
