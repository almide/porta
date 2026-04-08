<!-- description: OCI-compatible image push/pull for porta agent distribution -->

# Image Distribution

Push and pull porta agent packages to/from OCI-compatible registries (ghcr.io, Docker Hub, etc.).

## Package Format

A porta image is a tar archive containing:

```
agent.wasm          — Compiled WASM binary
manifest.json       — Tool/resource/prompt definitions + capabilities
```

Stored as an OCI artifact with media type `application/vnd.porta.agent.v1+tar`.

## CLI

```bash
# Push to registry
porta push agent.wasm --tag ghcr.io/user/my-agent:v1

# Pull from registry
porta pull ghcr.io/user/my-agent:v1

# Run directly from registry (pull + run)
porta run ghcr.io/user/my-agent:v1
```

## Implementation

### Dependencies

- `porta.http` builtin tool (already implemented) for registry API calls
- OCI Distribution Spec v2 API client (Rust, in native bridge)

### Registry API Flow

**Push:**
1. Check if blobs exist (HEAD /v2/<name>/blobs/<digest>)
2. Upload .wasm as blob (POST + PUT /v2/<name>/blobs/uploads/)
3. Upload manifest.json as blob
4. Put OCI manifest (PUT /v2/<name>/manifests/<tag>)

**Pull:**
1. Get OCI manifest (GET /v2/<name>/manifests/<tag>)
2. Download blobs (GET /v2/<name>/blobs/<digest>)
3. Extract .wasm + manifest.json to local cache

### Authentication

- Token-based auth (Bearer token from /v2/token)
- `porta login <registry>` to store credentials
- Credentials stored in `~/.porta/auth.json`

### Local Cache

```
~/.porta/cache/
  ghcr.io/user/my-agent/
    v1/
      agent.wasm
      manifest.json
```

## Future Integration

- `porta push --sign` — Sign on push (integrates with supply-chain.md)
- `porta pull --verify` — Reject unsigned images
- `porta compose` — Pull images referenced in compose file
