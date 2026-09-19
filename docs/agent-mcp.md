# MCP tools for WASM agents

A WASM agent can use explicitly granted remote MCP tools alongside its WASM tools
and delegated agents. The guest controls the reasoning loop; the host owns the
endpoint, credentials, schema validation, budgets and durable tool records.

```toml
[[mcp_tools]]
name = "search_docs"
remote_name = "search"
endpoint = "https://mcp.example.com/mcp"
token_env = "DOCS_MCP_TOKEN"
description = "Search the documentation."
input_schema = { type = "object", properties = { query = { type = "string", minLength = 1 } }, required = ["query"], additionalProperties = false }
```

`name` is the function exposed to the model and guest. `remote_name` is sent in
`tools/call`. Both names and the endpoint are operator configuration. Names must
be unique across WASM tools, MCP tools and delegated agents. Supply the remote
tool's input schema explicitly; discovery does not automatically grant tools.
The schema is compiled at launch and checked before opening an MCP connection.

`token_env` is optional. Its value becomes an Authorization bearer header only
for that MCP endpoint. It is never supplied to WASM or the model, or included in
the recorded request. Results and conversations can still contain sensitive
information returned by the service. Keep journals private.

HTTPS is required except on explicit loopback addresses. Endpoint credentials,
query strings and fragments are rejected. Redirects, inherited HTTP proxies and
HTTP-library retries are disabled. A guest cannot replace a URL through arguments.
The remote service can perform actions according to its own permissions: its code
does **not** run inside Porta's WASM sandbox. Configure only the services and tools
whose effects the agent is allowed to request.

## Transport and lifecycle

The client implements a bounded [Streamable HTTP](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)
tool-call path with JSON and SSE responses. It negotiates protocol versions
2025-11-25, 2025-06-18 or 2025-03-26, sends the initialized notification, carries
session/version headers, and attempts session deletion after the call. Each new
external tool call starts its own MCP session. State shared through a persistent
MCP session is not currently supported.

Responses are limited to 1 MiB and SSE streams to 128 dispatched events. Each
request observes the remaining agent/team deadline, capped at 30 seconds. Best
effort session deletion is capped at one second and the remaining deadline.
Notifications are ignored. No roots, sampling, elicitation or other server-to-client
capabilities are advertised; requests for them fail the run. Tool results,
including `isError`, are delivered to the guest for further reasoning. Protocol,
transport and invalid-result failures stop the run.

There is no background GET stream, SSE reconnect, legacy HTTP+SSE endpoint,
OAuth discovery, task polling, dynamic tool-list refresh, or stdio subprocess
client in this agent broker. Porta's existing stdio MCP **server** remains a
separate interface. This is not a claim of full MCP feature coverage or support
for every protocol revision.

## Durable effects

`--record` persists an intent before MCP initialization and the tool request.
The result is made durable before the next guest decision. Missing credentials
fail before intent, allowing a later resume after credentials are supplied.
Completed MCP calls are replayed from the journal, without a connection or token.
The current broker fingerprint is version 5; earlier broker journals are refused
rather than resumed under changed execution semantics.

An interruption, timeout, protocol error or crash before the result is durable
leaves an uncertain intent. Resume refuses to repeat it, even if initialization
failed before the actual tool request. Porta cannot infer whether a remote effect
happened and does not promise distributed exactly-once execution. Inspect the
remote service before starting a separate run. Session IDs are transient and are
not persisted or reconnected during recovery.

## Verification

```bash
python3 scripts/mcp_agent_integration.py target/porta examples/chat-agent/agent.wasm
python3 -m venv /tmp/porta-mcp-sdk
/tmp/porta-mcp-sdk/bin/python -m pip install mcp==1.30.0
/tmp/porta-mcp-sdk/bin/python scripts/mcp_sdk_integration.py target/porta examples/chat-agent/agent.wasm
```

The first suite covers scoped credentials, schemas, JSON/SSE, Unicode, sessions,
redirects, response limits, malformed replies, ungranted server capabilities,
timeouts, offline replay and SIGKILL during a remote effect. The second uses the
[official Python SDK](https://github.com/modelcontextprotocol/python-sdk/tree/v1.x)
with real file writes, in JSON/SSE and stateful/stateless modes, and shuts down
the MCP server before replay. Model replies in these tests are deterministic
fixtures; they do not measure model quality.

## Model-visible text in the included WASM guest

The Almide chat guest displays one plain MCP text block directly as a tool
message when structured content is absent, is the matching SDK string wrapper
`{result: text}`, or parses to the same serialized JSON value. This removes the
redundant transport envelope and duplicate text from model context. It preserves
newlines, Unicode, quotes and backslashes. JSON object order can conservatively
prevent compaction; this is a display optimization, not semantic canonicalization.

Errors, conflicting representations, multiple blocks, images, missing text, and
annotated text blocks retain the JSON envelope. This guest does not implement
multimodal model requests. Tool text remains in the `tool` role and does not
become a system instruction or grant capabilities.

The host retains the complete original MCP result in its journal and completion
verification history. In particular, the guest's compact display cannot hide a
conflict from an operator-owned verifier. Updating the guest changes its artifact
hash; retain the original artifact to replay older journals. Integration tests
exercise these display cases and verify real official-SDK writes, resume without
duplicate effects, completion gating, and offline replay.
