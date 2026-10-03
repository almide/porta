# Distribution

## What is distributed

| Artifact | Built by | Contains |
|---|---|---|
| `porta` binary | `almide build src/main.almd -o target/porta` (or the release workflow) | porta, including the job service, statically linking its Rust dependencies |
| Evaluation image | `docker build -t porta-eval .` ([`Dockerfile`](../../Dockerfile)) | `debian:trixie-slim`, CA certificates, the binary, the sample module and catalog, two policies, `LICENSE`, `THIRD_PARTY_LICENSES.md` |
| Sample job | `almide build examples/enterprise/sample-job/src/main.almd --target wasm` | `sample-job.wasm`, committed; its digest is pinned in both example policies |
| Deployment templates | — | [`deploy/`](../../deploy/) |

The image is built from source in its first stage, so `docker build` on a
clean checkout is the whole build. The `test` stage runs the job suite as an
unprivileged user: `docker build --target test .`. The image refuses to build
if a policy's pinned digest does not match the module it ships.

## Reproducibility — what holds and what does not

- **Pinned exactly**: the Almide compiler (a release, checksum-verified by
  `scripts/install-almide.sh`) and jsonschema (`=0.56.0`).
- **Pinned to a minor version only**: wasmtime and wasmtime-wasi.
  `almide.toml` says `47.0.3`, which Cargo reads as `^47.0.3`; the build that
  produced `THIRD_PARTY_LICENSES.md` resolved **47.0.4**.
- **Not pinned**: the rest of the Rust dependency graph. Almide generates the
  crate and resolves `almide.toml`'s semver ranges at build time; there is no
  committed `Cargo.lock`, so two builds on different days can link different
  patch versions of, for example, `reqwest`, `rustls` or `serde_json`.
  `THIRD_PARTY_LICENSES.md` is generated from one such resolution and records
  exactly which versions that build used.
- **Not pinned**: base images are referenced by tag (`rust:1-trixie`,
  `debian:trixie-slim`), not digest.

So the build is repeatable but not bit-for-bit reproducible. Fixing that is
item 3 in [gaps](gaps.md): commit the generated lockfile (needs an Almide
option to use one) and pin base images by digest.

## Licences

porta is Apache-2.0 ([`LICENSE`](../../LICENSE)).

[`THIRD_PARTY_LICENSES.md`](../../THIRD_PARTY_LICENSES.md) lists the 300
Rust crates in the binary for Linux (x86-64, arm64) and macOS (arm64), with
their licence expressions. All are permissive or offer a permissive option:
MIT, Apache-2.0 (with or without the LLVM exception), BSD-2/3-Clause, ISC,
Zlib, Unicode-3.0, BSL-1.0, MIT-0, Unlicense, CDLA-Permissive-2.0. Two crates
are dual `GPL-2.0-only OR BSD-3-Clause`; porta uses them under BSD-3-Clause.
None is copyleft-only.

Redistribution in a commercial product is compatible with these licences on
the usual conditions: ship the licence texts and notices (Apache-2.0 §4,
the BSD and MIT notice clauses, Unicode-3.0's notice). The image ships
`LICENSE` and the list; it does **not** yet ship each crate's full licence
text, which a release should (`cargo about` or similar can collect them).
This is item 6 in [gaps](gaps.md). This page is an engineering inventory, not
legal advice.

Run-time library not compiled in: glibc (LGPL-2.1+), dynamically linked
from the image or host. TLS is rustls; porta links no OpenSSL. The base image's
Debian packages carry their own copyright files under `/usr/share/doc`.

Regenerate the list after a dependency change:

```bash
# after `almide build`, in the generated crate directory almide reports
cargo metadata --format-version 1 --locked \
  --filter-platform x86_64-unknown-linux-gnu --filter-platform aarch64-unknown-linux-gnu \
  --filter-platform aarch64-apple-darwin > meta.json
python3 scripts/third_party_licenses.py meta.json > THIRD_PARTY_LICENSES.md
```

## Releases

The binary follows porta's existing release process ([CLAUDE.md](../../CLAUDE.md#releasing)):
tagged builds, signed, with provenance, verified by `scripts/verify_release.sh`.
The evaluation image is not published to any registry; publishing it is a
decision for whoever owns the product (see [gaps](gaps.md)).
