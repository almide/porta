<!-- description: Fix JSON-RPC to actually use Content-Length for message framing -->

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

```almide
if string.starts_with(trimmed, "Content-Length:") then {
  let len_str = string.trim(string.replace(trimmed, "Content-Length:", ""))
  let len = util.parse_int(len_str)
  skip_headers()
  let body_bytes = io.read_n_bytes(len)
  let body = string.from_bytes(body_bytes)
  parse_and_wrap(body)
}
```

## Files
- `src/jsonrpc.almd` — fix read_message to use Content-Length
