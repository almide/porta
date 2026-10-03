# Job API and configuration, version 1

This is the contract of the evaluation build: the operator's **policy**, a
client's **job**, the **run record** porta returns, and the **HTTP API** that
carries them. It describes what `native/job_service/` implements; where this
page and the code disagree, the code is right and this page is a bug.

- Policy: `policy_version = 1` — [`native/job_service/policy.rs`](../../native/job_service/policy.rs)
- Job: `version = 1` — [`native/job_service/spec.rs`](../../native/job_service/spec.rs)
- Record: `record_version = 1` — [`native/job_service/supervisor.rs`](../../native/job_service/supervisor.rs)
- HTTP — [`native/job_service/http.rs`](../../native/job_service/http.rs)

## Commands

| Command | What it does | Exit code |
|---|---|---|
| `porta job-check --policy P` | Loads the policy, checks every module's digest, prints what a client may see. Nothing runs. | 0, or 1 when the policy is invalid |
| `porta job-check --policy P job.json` | Says whether the policy accepts the job and what it would be granted. Nothing runs. | 0 accepted, 2 refused |
| `porta job-run --policy P job.json` | Runs one job through the same supervisor and worker as the server; prints the finished record. | 0 succeeded, 2 refused, 1 anything else |
| `porta job-serve --policy P [--listen ADDR] [--no-auth]` | Serves the HTTP API until SIGTERM/SIGINT. | 0 after a signal |

`job-serve` reads its bearer token from `PORTA_JOB_TOKEN` (16 characters or
more) and removes it from its own environment. Without a token it refuses to
start unless `--no-auth` is given *and* `--listen` is a loopback address.
`--listen` defaults to `127.0.0.1:8640`.

## Policy (TOML)

Owned by the operator. Unknown keys are rejected. Relative paths resolve
against the policy file's directory. The policy's identity is its `label`
plus the SHA-256 of the file's bytes; both go into every record.

```toml
version = 1
label = "evaluation-2026-10"       # 1–128 chars; your name for this version
env_names = ["REPORT_CURRENCY"]    # variables a job may set; default none

[service]
records = "state/records"          # created 0700 if missing
workspaces = "state/workspaces"    # created 0700; must not contain/contain records
max_concurrent = 4                 # 1–64; default 2
retain_output = true               # default false: records keep only sizes and digests
max_request_bytes = 1048576        # 1 KiB–256 MiB; default 4 MiB
os_sandbox = "required"            # "required" (default) or "off"

[limits]                           # ceilings; every one is required
timeout_ms = 10000                 # ≤ 3 600 000
fuel = 100000000000                # wasmtime fuel units
memory_mib = 64                    # linear memory per store; ≤ 4096
max_output_bytes = 1048576         # stdout cap, and the /output total; ≤ 64 MiB
max_input_bytes = 524288           # input + files + args + env; ≤ 64 MiB

[defaults]                         # optional; each missing value = its ceiling
timeout_ms = 3000

[[modules]]                        # what may run; nothing else can
name = "sample-job"                # [A-Za-z0-9._-]{1,64}
wasm = "sample-job/sample-job.wasm"
sha256 = "…"                       # checked at load and again by every worker

[[directories]]                    # host directories a job may ask for
name = "catalog"                   # mounted at /data/catalog
host = "catalog"
access = "read"                    # "read" or "read-write": the most a job may ask
```

Load-time refusals (the service does not start): running as root; a
`records` or `workspaces` directory it cannot write; a module whose bytes do not
match its `sha256`; a directory that does not exist, overlaps `records` or
`workspaces`, or is writable and holds the policy file or a module; a default
above its ceiling; `os_sandbox = "required"` on a host where `porta run`
cannot apply the worker's sandbox (the message names what is missing).

## Job (JSON)

Sent by a client. Unknown fields are rejected — a job cannot carry a field the
broker would silently ignore, such as a network grant.

```json
{
  "version": 1,
  "module": "sample-job",
  "input": "text given to the module on stdin",
  "args": ["report"],
  "env": {"REPORT_CURRENCY": "JPY"},
  "files": {"orders.csv": "item,quantity\nwidget,4\n"},
  "directories": [{"name": "catalog", "access": "read"}],
  "output": true,
  "limits": {"timeout_ms": 2000, "fuel": 50000000, "memory_mib": 16, "max_output_bytes": 4096}
}
```

| Field | Default | Rule |
|---|---|---|
| `version` | — | must be 1 |
| `module` | — | a name in the policy's `[[modules]]` |
| `input` | `""` | becomes stdin |
| `args` | `[]` | ≤ 64; argv[0] is the module name |
| `env` | `{}` | names must be in `env_names`; values are never recorded |
| `files` | `{}` | ≤ 64; one path component of `[A-Za-z0-9._-]`; written to `/input`, read-only |
| `directories` | `[]` | names from `[[directories]]`; `read-write` only where the policy grants it |
| `output` | `true` | `false` gives the job no writable `/output` |
| `limits` | policy defaults | each ≤ its ceiling; a larger value is refused, not lowered |

### What the module sees

| Inside | Is | Access |
|---|---|---|
| stdin | `input` | read |
| stdout, stderr | memory pipes; stdout capped at `max_output_bytes`, stderr at min(that, 256 KiB) | write |
| `/input` | the job's `files`, a per-run copy | read |
| `/output` | an empty per-run directory | read-write |
| `/data/<name>` | a granted policy directory | as granted |
| environment | exactly the job's `env` | read |
| network | none | — |
| host functions | WASI only: preview 1 for a core module, 0.2 for a component | — |

Anything else — another path, `..` past a mount, a symlink out of one, an
import porta does not provide — fails inside the sandbox (a WASI error the
module sees) or before the module starts (`link_refused`).

## Run record (JSON)

One file per run, `records/<run_id>.json`, written when the run starts
(`status: running`) and replaced when it ends (`status: finished`). A refused
job gets a record too. `GET /v1/runs/{id}` returns it unchanged.

| Field | Meaning |
|---|---|
| `record_version` | 1 |
| `run_id` | `YYYYMMDDTHHMMSSZ-` + 12 random hex digits |
| `status` | `running` or `finished` |
| `outcome` | `succeeded`, `failed`, `stopped`, `refused` |
| `stop_reason` | see below |
| `detail` | a sentence for people; may include the trap message (≤ 2 KiB) |
| `exit_code` | the module's exit code, or `null` when it did not exit |
| `porta_version` | the binary that ran it |
| `policy` | `{label, sha256}` of the policy in force |
| `module` | `{name, sha256}` |
| `job_sha256` | SHA-256 of the accepted job as canonical JSON; of the raw request body for a refused one |
| `limits` | the limits applied after defaults |
| `grants` | input file names, `/output`, directories with access, env *names*, `network: none`, `host_functions: wasi` |
| `isolation` | `{wasm: "wasmtime", worker_process: true, os_sandbox: bool}` |
| `submitted_at`, `started_at`, `finished_at` | RFC 3339 UTC, milliseconds |
| `usage` | `fuel_consumed` (`null` after a deadline stop, which wasmtime does not report exactly), `peak_memory_bytes`, `memory_grow_denied`, `wall_ms` |
| `stdout`, `stderr` | `{bytes, sha256, capped}`, plus `text` (first 64 KiB) and `truncated` when the policy retains output |
| `output_files` | `[{path, bytes, sha256}]`; a symlink or special file is listed with `ignored` and never read |
| `workspace_removed` | whether the per-run workspace is gone |

### Outcomes and stop reasons

| `outcome` | `stop_reason` | When |
|---|---|---|
| succeeded | `exit` | the module returned or exited 0 |
| failed | `exit_nonzero` | the module exited with another code |
| failed | `trap` | the module trapped (unreachable, out-of-bounds, stack overflow…) |
| failed | `invalid_module` | not a WASM module or component, or a core module without `_start` |
| failed | `worker_crashed`, `worker_error`, `module_unavailable` | the worker ended without a result, failed itself, or could not read the module |
| stopped | `timeout` | the epoch deadline interrupted it, or the supervisor killed it 5 s later |
| stopped | `fuel_exhausted` | fuel ran out |
| stopped | `memory_limit` | it trapped or exited after a `memory.grow` past `memory_mib` was refused |
| stopped | `output_limit` | it trapped writing past the stdout cap, or exited 0 leaving more than `max_output_bytes` in `/output` |
| stopped | `cancelled` | a client cancelled it |
| stopped | `service_shutdown` | the service got SIGTERM/SIGINT while it ran |
| stopped | `interrupted` | the service died while it ran; found at the next start, never re-run |
| refused | `module_not_registered`, `directory_not_granted`, `access_exceeds_grant`, `limit_exceeds_ceiling`, `env_not_allowed`, `input_too_large`, `invalid_job`, `invalid_file_name`, `request_too_large`, `capacity`, `service_stopping` | the policy or the service refused it before anything was created |
| refused | `link_refused` | the module imports something porta does not provide (for a component, also: no `wasi:cli/run` export) |
| refused | `module_changed` | the module's bytes no longer match the pinned digest |
| refused | `sandbox_refused` | `porta run` refused to start the worker under the OS sandbox |

A module that keeps printing after its stdout cap may ignore the write errors
and run on; it then ends by its fuel or deadline, with `stdout.capped: true`.

### Event log

`records/events.jsonl` gets one line per change — `started`, `finished`,
`refused`, `stop_requested`, `interrupted`, `deleted` — with the time, run id,
outcome, stop reason, module name and policy digest. It never contains input,
output, environment values or the token. It is append-only by convention, not
tamper-evident.

## HTTP API

HTTP/1.1, one request per connection, JSON bodies, `Content-Length`
required for bodies (chunked is refused with 411). Every path except
`/healthz` needs `Authorization: Bearer <PORTA_JOB_TOKEN>`.

| Method and path | Success | Body |
|---|---|---|
| `GET /healthz` | 200 | `{"ok": true}` |
| `GET /v1/policy` | 200 | label, digest, ceilings, defaults, env names, modules (name, digest), directories (name, access), `network`, `os_sandbox`, `max_concurrent`, `retain_output` — never host paths |
| `POST /v1/runs` | 202 | the record with `status: running` |
| `POST /v1/runs?wait=S` | 200 / 202 | waits up to S seconds (≤ 300); 200 with the finished record, 202 if still running |
| `GET /v1/runs?limit=N` | 200 | `{"runs": [...]}`, newest first, N ≤ 1000 (default 50) |
| `GET /v1/runs/{id}?wait=S` | 200 | the record, waiting up to S seconds for the end |
| `POST /v1/runs/{id}/cancel` | 202 | `{"run_id", "status": "stopping"}`; 409 if finished |
| `GET /v1/runs/{id}/output/{path}` | 200 | a retained `/output` file the record lists |
| `DELETE /v1/runs/{id}` | 204 | removes the record and retained output; 409 while running |

Refusals of a job answer with its record: 400 (`invalid_job`,
`invalid_file_name`), 403 (policy refusals), 413 (`request_too_large`), 429
(`capacity`), 503 (`service_stopping`). Other errors are
`{"error": {"code", "message"}}`: 401, 404, 405, 411, 415, 431, 500, 503.

There is no TLS: terminate it at the platform's load balancer or keep the
listener on loopback. There is one token with full access; there are no
per-client identities or roles yet ([gaps](gaps.md)).
