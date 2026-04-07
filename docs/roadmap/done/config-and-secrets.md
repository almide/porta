<!-- description: Environment variables, config injection, and secret management -->
<!-- done: 2026-04-07 -->

# Configuration & Secrets

Runtime configuration and sensitive value management for porta instances.

## Design Principle

Config and secrets are deliberately separated:

- **Config** — Normal values. Visible in `porta inspect`, logged freely.
- **Secret** — Sensitive values. Never shown in `porta inspect`, never logged, redacted in diagnostics.

Mixing the two is a common source of security incidents. porta makes the distinction structural, not convention-based.

## Environment Variables

### Injection

- `--env KEY=VALUE` — Set single variable
- `--env-file .env` — Load from file
- Defaults in `almide.toml`:

```toml
[env]
LOG_LEVEL = "info"
REGION = "ap-northeast-1"
```

### Precedence

`--env` flag > `--env-file` > `almide.toml` defaults

Runtime flags always override static configuration.

## Secret Management

### Injection

Two delivery mechanisms, both available simultaneously:

- **File mount**: `--secret api-key` → accessible at `/run/secrets/api-key`
- **Env binding**: `--secret api-key,env=API_KEY` → injected as environment variable

### Scope

- **build-time secret** — Available during `porta build`, never persisted in artifact
- **run-time secret** — Available during execution only

### Security Guarantees

- `porta inspect` never reveals secret values
- Secrets excluded from logs, tracing spans, and error messages
- Memory holding secrets is zeroed on instance cleanup
- Secret values never appear in recorded execution traces

### Rotation

Secret updates trigger a configurable action:

- `restart` — Restart instance with new secret value
- `reload` — Signal instance to re-read (if the module supports it)

In compose mode, rolling restart ensures zero-downtime rotation across scaled instances.

## Module

- Part of `sandbox.almd` — Secret injection and access control
- Part of `dispatch.almd` — Env/config assembly during instance creation
