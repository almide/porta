<!-- description: Host-level HTTP filtering via local CONNECT proxy -->

# HTTP Proxy Filtering

**Priority: High**

## Problem

sandbox-exec only supports port-based network filtering. `--allow-net '*:443'` allows ALL hosts on port 443. There is no way to say "allow api.anthropic.com but block google.com" at the OS sandbox level.

This means porta cannot selectively block web search, analytics, or other HTTPS services while allowing an API endpoint on the same port.

## Approach

porta runs a lightweight local proxy that filters HTTPS connections by hostname.

```
sandboxed process → porta proxy (127.0.0.1:N) → internet
                         ↓
                  CONNECT hostname check
                  allow/deny decision
```

### How it works

1. porta starts a local TCP proxy before exec
2. Injects `HTTP_PROXY` / `HTTPS_PROXY` env vars into the sandboxed process
3. For HTTPS: reads the CONNECT method's target hostname, allows or denies
4. No MITM — encrypted content is not inspected, only the connection target

### CLI

```bash
# Allow only Anthropic API
porta run claude --proxy-allow 'api.anthropic.com'

# Block specific hosts
porta run claude --proxy-deny 'google.com,bing.com'

# Combine with existing sandbox
porta run claude -v . --allow-net '*:443' --proxy-allow 'api.anthropic.com'
```

### porta.toml

```toml
[proxy]
allow = ["api.anthropic.com"]
# deny = ["google.com", "bing.com"]
```

## Design notes

- Proxy only activates when `--proxy-allow` or `--proxy-deny` is specified
- `--proxy-allow` is allowlist mode: only listed hosts pass
- `--proxy-deny` is denylist mode: everything passes except listed hosts
- Cannot combine both — pick one mode
- Proxy binds to 127.0.0.1 with a random port
- Works for any tool that respects HTTP_PROXY/HTTPS_PROXY (most do)

## Files
- `src/proxy.almd` — new module: TCP proxy with CONNECT filtering
- `src/engine.almd` — start proxy before exec, inject env vars
- `src/cli.almd` — --proxy-allow, --proxy-deny flags
- `src/config.almd` — [proxy] section in porta.toml
