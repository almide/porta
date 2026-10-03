# Compatibility

Three columns, kept apart on purpose: **ran here** (the suite passed on that
machine), **template** (a deployment file exists and has never been applied),
**not started**. Nothing in the second or third column is "supported".

## Where it has run

| Environment | Build | `job_integration.py` (84 checks) | `evaluate.py` | OS sandbox (`required`) |
|---|---|---|---|---|
| macOS 26.3, Apple silicon (arm64), native binary | `almide build` | passed | passed | applied (Seatbelt) |
| Linux 6.12 (Docker Desktop 29.6, linuxkit), arm64, `rust:1-trixie`, unprivileged user | `docker build --target test` | passed | — | applied (Landlock + seccomp; user namespaces refused by Docker's default profile, which porta reports and proceeds without) |
| Same kernel, `porta-eval` runtime image (`debian:trixie-slim`), `--read-only` root, non-root uid 10001 | `docker build` | — | passed with `policy.toml` and with `policy-wasm-only.toml` | applied |

Also checked: a fresh `git clone` of the commit, `docker build --no-cache`
(3 minutes on this machine) and the README's run steps verbatim — all 11
`evaluate.py` checks passed. In the container: either policy refuses to start
as root; `docker stop` during a run → the run ends as
`service_shutdown`; `docker kill` during a run → after `docker start` the run
is `interrupted`, its workspace removed, nothing re-run; records persist on
the volume across restarts; without `PORTA_JOB_TOKEN` the image refuses to
start.

Not run: Linux x86-64 (the code is the same; porta's CI builds it, but the job
suite has not run there), Intel macOS, any distribution other than Debian
trixie, a kernel without Landlock, gVisor.

## Cloud platforms

All facts below are from vendor documentation read on 2026-10-03; "ND" means
the documentation says nothing. No resource was created on any of them.

| | AWS ECS Fargate | Google Cloud Run | Azure Container Apps | Cloudflare Containers | AgentCore Runtime |
|---|---|---|---|---|---|
| **Status** | template | template | template | template + Worker adapter | design note only |
| Files | [`deploy/aws-ecs`](../../deploy/aws-ecs/) | [`deploy/gcp-cloud-run`](../../deploy/gcp-cloud-run/) | [`deploy/azure-container-apps`](../../deploy/azure-container-apps/) | [`deploy/cloudflare-containers`](../../deploy/cloudflare-containers/) | [`deploy/aws-agentcore`](../../deploy/aws-agentcore/) |
| Image arch | arm64 or amd64 | amd64 | amd64 | amd64 | arm64 only |
| Isolation of the container | "isolated virtual environment" | gen1 gVisor / gen2 microVM | ND | Firecracker microVM | microVM per session |
| porta's OS sandbox | ND — the service probes at start; fall back to `policy-wasm-only.toml` if refused | ND on gen2; pin gen2 | ND | ND | ND |
| Fits porta's API as is | yes | yes | yes | needs the Worker in front | no: `/invocations` + `/ping` contract, edge auth |
| Records survive restart | with EFS (untested) | with GCS FUSE / NFS, gen2 (untested) | with Azure Files (untested) | no (fresh disk after sleep) | session storage (preview) |
| Stop signal and grace | SIGTERM, `stopTimeout` ≤ 120 s | SIGTERM, 10 s | SIGTERM, 30 s | SIGTERM, up to 15 min | ND |
| Request timeout to keep `?wait=` under | load balancer's | `timeoutSeconds` (≤ 60 min) | 240 s default | ND | 15 min sync |

Sources: [ECS task definitions](https://docs.aws.amazon.com/AmazonECS/latest/developerguide/task_definition_parameters.html),
[Fargate security](https://docs.aws.amazon.com/AmazonECS/latest/developerguide/fargate-security-considerations.html),
[Cloud Run container contract](https://docs.cloud.google.com/run/docs/container-contract),
[Cloud Run execution environments](https://docs.cloud.google.com/run/docs/about-execution-environments),
[Container Apps containers](https://learn.microsoft.com/en-us/azure/container-apps/containers),
[Container Apps lifecycle](https://learn.microsoft.com/en-us/azure/container-apps/application-lifecycle-management),
[Cloudflare Containers architecture](https://developers.cloudflare.com/containers/platform-details/architecture/),
[Cloudflare container package](https://developers.cloudflare.com/containers/container-package/),
[AgentCore HTTP contract](https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/runtime-http-protocol-contract.html).

### What a real-cloud check would need (awaiting approval)

Each step creates billable resources and publishes an image to a registry,
so none has been done. Proposed order, cheapest first, one hour each, then
deleted:

| Step | Creates | Rough cost of one hour (list prices, verify before approving) |
|---|---|---|
| Cloud Run gen2, 1 vCPU / 1 GiB, max 1 instance | Artifact Registry repo, secret, service | under USD 1; often inside the free tier |
| ECS Fargate arm64, 0.5 vCPU / 1 GiB | ECR repo, secret, task definition, service (no ALB: test through a bastion or ECS Exec) | under USD 1, plus Secrets Manager's monthly per-secret charge |
| Azure Container Apps, 0.5 vCPU / 1 GiB, 1 replica | ACR, Key Vault secret, environment, app | under USD 1 plus ACR Basic's daily charge |
| Cloudflare Containers, standard-1 | Worker, container image, secret | requires the Workers Paid plan (monthly subscription) plus usage |

For each: run `evaluate.py` against it, record whether `os_sandbox =
"required"` started, `docker stop`-equivalent behaviour (scale to zero),
and delete everything with the commands in that directory's README. The
result goes into the table above as "ran here", with the date.
