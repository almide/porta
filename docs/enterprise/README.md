# Porta Enterprise — evaluation build

*Working name. This is an evaluation build, not a product release: there is
no licence key, expiry, billing or remote shutdown, and none is planned
before pricing and terms are decided.*

Porta runs WebAssembly jobs you do not trust under a policy you write. A
client submits a job — which registered module to run, its input, the
directories it may touch, its limits — over an HTTP API; porta checks it
against the policy, runs it in WebAssembly inside its own process, stops it
at its limits, and returns the result, why it stopped, and a run record.

[日本語の評価手順](README.ja.md)

| Document | What it covers |
|---|---|
| [API and configuration](api.md) | policy, job, record, HTTP endpoints, stop reasons — the contract |
| [Threat model](threat-model.md) | what an untrusted module can and cannot do, and how each claim is tested |
| [Compatibility](compatibility.md) | where it has run, where it has not, and what each platform allows |
| [Operations](operations.md) | install, remove, upgrade, records, logs, limits of the evaluation build |
| [Distribution](distribution.md) | building it, the container image, licences of what is inside |
| [Gaps](gaps.md) | what a product still needs, ranked, with the evidence for each |
| [Cloud templates](../../deploy/) | ECS, Cloud Run, Container Apps, Cloudflare — written, not deployed |

## Evaluate it in ten minutes

You need Docker. Nothing leaves your machine.

```bash
git clone https://github.com/almide/porta && cd porta
docker build -t porta-eval .                          # builds porta from source; ~10 min the first time
export PORTA_JOB_TOKEN=$(openssl rand -hex 24)
docker run -d --name porta-eval --read-only --tmpfs /tmp \
  -v porta-eval-state:/var/lib/porta -p 127.0.0.1:8080:8080 \
  -e PORTA_JOB_TOKEN porta-eval
python3 examples/enterprise/evaluate.py --url http://127.0.0.1:8080
```

`evaluate.py` (standard library only) submits the sample jobs and prints
`PASS`/`FAIL` for each expectation:

```text
PASS  granted work succeeds
PASS  asking for write access the policy does not grant is refused
PASS  asking for a directory the policy does not list is refused
PASS  asking past a limit ceiling is refused
PASS  ungranted reads and writes are denied inside the sandbox
PASS  an infinite loop stops at its deadline
PASS  an infinite loop stops when its fuel runs out
PASS  unbounded allocation stops at the memory ceiling
PASS  unbounded output stops at its cap
PASS  cancel stops a running job
PASS  every run above has a finished record (10 listed)
```

Then look for yourself:

```bash
H="Authorization: Bearer $PORTA_JOB_TOKEN"
curl -s -H "$H" http://127.0.0.1:8080/v1/policy                       # the rules in force, and their digest
curl -s -H "$H" 'http://127.0.0.1:8080/v1/runs?limit=5'               # newest runs
curl -s -H "$H" http://127.0.0.1:8080/v1/runs/<run_id>                # one full record
docker exec porta-eval tail -3 /var/lib/porta/records/events.jsonl    # the event log
docker stop porta-eval && docker start porta-eval                     # records survive; a job running at stop ends as service_shutdown
```

Remove everything:

```bash
docker rm -f porta-eval && docker volume rm porta-eval-state && docker rmi porta-eval
```

### Without Docker

On macOS (Apple silicon) or Linux, with the pinned Almide:

```bash
bash scripts/install-almide.sh
.tools/almide/almide build src/main.almd -o target/porta
target/porta job-check --policy examples/enterprise/policy.toml     # the policy, and every module's digest
target/porta job-run --policy examples/enterprise/policy.toml examples/enterprise/jobs/report.json
target/porta job-run --policy examples/enterprise/policy.toml examples/enterprise/jobs/probe.json
export PORTA_JOB_TOKEN=$(openssl rand -hex 24)
target/porta job-serve --policy examples/enterprise/policy.toml &   # 127.0.0.1:8640
python3 examples/enterprise/evaluate.py --url http://127.0.0.1:8640
```

Records go to `examples/enterprise/state/`; delete that directory to start over.

## The sample job

[`examples/enterprise/sample-job`](../../examples/enterprise/sample-job/src/main.almd)
is an order report written in Almide and compiled to WASI: it reads
`/input/orders.csv` (the job's own file), prices from `/data/catalog`
(a directory the policy grants read-only), writes `/output/report.json`. The
same module misbehaves on request — `probe` tries six reads and writes it was
not granted, `spin` loops, `grow` allocates, `flood` prints — because the
point is to watch porta contain it. Porta treats every module as untrusted.

Bring your own: any WASI preview 1 module with `_start`, or WASI 0.2 command
component. Add it to the policy with its SHA-256 (`shasum -a 256 x.wasm`).

## What is proven, and how

| Claim | Status | Evidence |
|---|---|---|
| Granted work succeeds; its result, output files and digests are in the record | implemented, tested locally | `scripts/job_integration.py`, `evaluate.py` |
| A job asking beyond the policy is refused and recorded | implemented, tested locally | 9 refusal cases |
| Ungranted file access fails inside the sandbox | implemented, tested locally | `probe` (6 attempts), symlink fixture |
| Ungranted host functions are refused before the module starts | implemented, tested locally | `host-import` fixture |
| Deadline, fuel, memory and output limits stop a job with their own reason | implemented, tested locally | `spin`, `fuel`, `grow`, `flood` |
| Cancel, SIGTERM and a killed service each leave a correct record and no process | implemented, tested locally | integration suite, container stop/kill |
| Concurrent jobs see only their own input | implemented, tested locally (2 jobs) | integration suite |
| Every worker also runs under porta's OS sandbox | implemented, tested on macOS 26 and Linux 6.12 (Docker) | `isolation.os_sandbox` in the record |
| Runs on AWS, Google Cloud, Azure, Cloudflare | **templates written, never deployed** | [compatibility](compatibility.md) |
| Network grants, per-client identity, TLS, signed records | **not implemented** | [gaps](gaps.md) |

"Tested locally" means on the machines in [compatibility](compatibility.md),
by the scripts named; there has been no third-party audit, and nothing here
claims the isolation is complete.

## Running the test suites

```bash
python3 scripts/job_integration.py target/porta     # 84 checks: policy, worker, records, API
docker build --target test .                        # the same suite on Linux, as an unprivileged user
```
