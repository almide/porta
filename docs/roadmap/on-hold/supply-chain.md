<!-- description: Image signing, provenance attestation, SBOM, and dependency locking -->

# Supply Chain Security

Integrity and provenance guarantees for porta modules, from build to deployment. WASM's safety narrative is strengthened considerably when the supply chain is also verifiable.

## Image Signing & Verification

OCI-compatible signing for published modules:

- `porta push --sign` — Sign artifact on push
- `porta verify <image:tag>` — Verify signature
- `porta pull --verify` — Reject unsigned/unverified images

### Digest Pinning

- `porta run registry.example.com/app@sha256:abc123...` — Pin to exact digest
- `porta-compose.toml` supports digest references alongside tags

Digest pinning ensures that what was tested is exactly what runs in production.

## Provenance Attestation

Build provenance records:

- Source commit hash
- Builder identity
- Build timestamp
- Build inputs (dependencies, flags)
- Reproducibility claim

Attestations follow SLSA framework where applicable.

## SBOM

Software Bill of Materials:

- `porta sbom <image>` — Generate SBOM for a module
- Formats: SPDX, CycloneDX
- Includes: Almide dependencies, WASI imports, capability requirements

## Reproducible Build

Deterministic builds ensure the same source produces the same artifact:

- `almide build --target wasm` is deterministic given the same inputs
- `porta build` records all inputs for reproducibility verification
- `porta verify --reproducible <image>` — Rebuild and compare against published artifact

## Dependency Locking

- `almide.lock` — Dependency hash pinning (exact versions + integrity hashes)
- Target ABI pinning (WASM feature set version)
- Host capability compatibility check at build time

Lock file ensures CI and production use identical dependency trees.

## Module

- Part of CLI subcommands (`push`, `verify`, `sbom`)
- `registry.almd` — OCI registry interaction, signing, verification
