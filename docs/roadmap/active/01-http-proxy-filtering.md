<!-- description: Host-level HTTP filtering via local CONNECT proxy -->

# HTTP Proxy Filtering

**Priority: High**

## Problem

sandbox-exec only supports port-based network filtering. `--allow-net '*:443'` allows ALL hosts on port 443. There is no way to say "allow api.anthropic.com but block google.com" at the OS sandbox level.

Additionally, naive proxy approaches are easy to bypass: if sandbox-exec still allows direct outbound on `*:443`, generated code using raw sockets skips the proxy entirely. The proxy must be the **only** egress path.

## Approach

porta runs a lightweight local CONNECT proxy and constrains sandbox-exec so the proxy port is the only outbound network rule permitted.

```
sandboxed process ──► porta proxy (127.0.0.1:N) ──► internet
                            │
                            ├─ CONNECT hostname check
                            └─ allow/deny decision + audit log
```

### How it works

1. porta starts a local TCP proxy on 127.0.0.1:<random> before launching the child
2. The sandbox-exec profile is rewritten so the only outbound rule is `*:<proxy-port>` (any user-supplied `--allow-net` is ignored with a warning)
3. `HTTP_PROXY` / `HTTPS_PROXY` env vars point the child at the proxy
4. For HTTPS: the proxy reads the CONNECT method's target hostname, matches against the allow/deny list, and either tunnels or returns 403
5. No MITM — TLS content is not inspected, only the connection target
6. Non-443 ports and non-CONNECT methods are rejected (HTTPS only in v1)
7. Every decision is logged to stderr and optionally appended as JSONL to `--proxy-audit <path>`

Because the Porta process must supervise the proxy thread, native execution switches from `exec()` replacement to `spawn() + wait()` (`wt_exec_supervised`). stdio is still inherited so interactive tools behave the same.

### Design decisions (v1)

- **Hostname matching**: exact match or `*.example.com` subdomain wildcard (proper dot-boundary match; `*.example.com` matches `example.com` and any subdomain but not `evilexample.com`).
- **DNS**: guest-side DNS is denied by the sandbox net policy. Well-behaved HTTPS clients (libcurl, requests, fetch) pass hostnames to the proxy via `HTTPS_PROXY` and never resolve locally; raw-socket bypasses fail fast.
- **Plaintext HTTP (port 80)**: denied. HTTPS-only in v1 simplifies parsing and enforces HTTPS defaults.
- **Audit**: structured stderr lines always; JSONL file optional via `--proxy-audit`.

### CLI

```bash
# Allow only Anthropic API (+ any subdomain)
porta run claude --proxy-allow 'api.anthropic.com,*.anthropic.com'

# Block specific hosts
porta run claude --proxy-deny 'google.com,*.google.com'

# Audit file
porta run claude --proxy-allow 'api.anthropic.com' --proxy-audit /tmp/porta-proxy.jsonl
```

### porta.toml

```toml
[proxy]
allow = ["api.anthropic.com", "*.anthropic.com"]
# deny = ["google.com", "*.google.com"]
# audit = "/tmp/porta-proxy.jsonl"
```

## Known limitations

- `*:<proxy-port>` still allows the sandboxed process to connect to any host on that specific high random port. In practice the port is bound by the proxy and unreachable remotely, but a tighter `remote ip` rule (`127.0.0.1:<port>` literal) would close the door philosophically. Deferred to v2.
- macOS only for the supervised exec path. Linux support tracked separately.
- Tools that ignore `HTTPS_PROXY` fail (by design — raw-socket outbound is denied by the sandbox).

## Files
- `native/wasmtime_bridge.rs` — `wt_proxy_start`, `wt_proxy_stop`, `wt_exec_supervised`
- `src/wasm_rt.almd` — extern declarations for the above
- `src/proxy.almd` — config type, start/stop wrappers, endpoint helpers
- `src/engine.almd` — `run_native_proxied` branch in `run_native`
- `src/cli.almd` — `--proxy-allow`, `--proxy-deny`, `--proxy-audit`
- `src/config.almd` — `[proxy]` section in porta.toml
- `src/proxy_test.almd` — pure-Almide unit tests (config, validation, formatting)

## Next iterations

- v2: tighten sandbox net rule to `127.0.0.1:<port>` literal; Linux support
- v3: credential broker — proxy injects scoped tokens so the agent never holds secrets
- v4: method/path-level policy (may subsume into credential scoping)
