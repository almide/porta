# Cloudflare Containers

**Status: template written, not deployed, not type-checked.** Nothing here
has run on Cloudflare. `package.json` leaves `@cloudflare/containers`
unpinned (`*`); pin the version `npm install` resolves on first use.

Cloudflare runs a container behind a Worker and a Durable Object
([container package](https://developers.cloudflare.com/containers/container-package/),
[architecture](https://developers.cloudflare.com/containers/platform-details/architecture/)).
This is an adapter, not just a manifest: [`src/index.ts`](src/index.ts)
routes every request to one named container instance and hands it the token
from a Worker secret; [`wrangler.jsonc`](wrangler.jsonc) builds the
repository's Dockerfile.

Differences from the other targets:

- **amd64 only**; wrangler builds the image.
- **Each instance is a Firecracker microVM with its own kernel.** Whether it
  takes Landlock and porta's seccomp filter is undocumented; if `policy.toml`
  refuses to start, set `entrypoint` / args to the wasm-only policy.
- **The disk is fresh after every sleep.** Records vanish when the instance
  sleeps (`sleepAfter`). Durable records would need the snapshot beta or an
  R2 FUSE mount — neither is wired up here.
- **SIGTERM, then up to 15 minutes.**
- `max_instances: 1` keeps every run on one instance; more instances would
  each have their own records.

## Steps (need approval: Workers Paid plan, billable usage)

```bash
cd deploy/cloudflare-containers
npm install
npx wrangler secret put PORTA_JOB_TOKEN
npx wrangler deploy
PORTA_JOB_TOKEN=... python3 ../../examples/enterprise/evaluate.py --url https://porta-eval.<SUBDOMAIN>.workers.dev
# Remove
npx wrangler delete
```

The Worker is public at its workers.dev URL; porta's token is the only gate.
Add Cloudflare Access in front for anything beyond a short evaluation.
