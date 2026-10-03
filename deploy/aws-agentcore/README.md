# Amazon Bedrock AgentCore Runtime (additional candidate)

**Status: not implemented. This page is the adapter design only.**

AgentCore Runtime runs one container per session in its own microVM, but its
contract is not porta's API
([HTTP protocol contract](https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/runtime-http-protocol-contract.html)):

| AgentCore requires | porta job-serve has |
|---|---|
| arm64 image, port 8080 on 0.0.0.0 | either architecture, any port |
| `POST /invocations`, JSON in, JSON or SSE out | `POST /v1/runs?wait=S` |
| `GET /ping` → `{"status": "Healthy" \| "HealthyBusy"}` | `GET /healthz` → `{"ok": true}` |
| inbound auth (SigV4 or JWT) at the AWS edge | its own bearer token |
| session storage under `/mnt/<name>` (preview) | `records` / `workspaces` paths from the policy |

The adapter would be a `--protocol agentcore` mode of `job-serve`:
`/invocations` = submit and wait, `/ping` = health (`HealthyBusy` while a job
runs). The open question is authentication: AgentCore authenticates the
caller before the request reaches the container, and porta today refuses to
listen on a non-loopback address without its own token. Relaxing that only
for AgentCore needs a decision on whether the edge's authentication is
sufficient for the customer — so it is left undone rather than guessed.

Limits to plan around: 2 vCPU / 8 GB per session, 15 minutes per
synchronous request, sessions of up to 8 hours
([limits](https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/bedrock-agentcore-limits.html)).
