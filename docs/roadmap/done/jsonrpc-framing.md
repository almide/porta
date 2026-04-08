<!-- description: Fix JSON-RPC to actually use Content-Length for message framing -->
<!-- done: 2026-04-08 -->

# JSON-RPC Proper Framing

**Priority: High**

## Problem

`jsonrpc.almd` reads `Content-Length` header but ignores the value. It reads the body with `io.read_line()` which breaks on:
- Messages containing newlines
- Multi-line JSON bodies
- Binary content

MCP clients that send properly framed messages (Content-Length + body) may produce incorrect parsing.

## Fix

1. Parse `Content-Length` header value as integer
2. Read exactly that many bytes with `io.read_n_bytes(length)`
3. Parse the body as JSON

## Files
- `src/jsonrpc.almd` — fix read_message to use Content-Length
