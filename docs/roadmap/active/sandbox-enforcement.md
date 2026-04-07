<!-- description: Runtime capability enforcement and WASI sandbox (Layer 3) -->
# Sandbox Enforcement

Runtime security layer (Layer 3 of Almide's three-layer defense).

## Responsibilities

- Pre-opened directory scoping (`--dir` flag)
- Environment variable filtering (`--env` flag)
- Memory limits (`--max-memory` flag)
- Timeout enforcement (epoch-based interruption)
- WASI import validation against manifest capabilities

## Module

- `sandbox.almd` — Capability enforcement, directory permission mapping
