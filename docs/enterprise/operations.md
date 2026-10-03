# Operations

How to install, run, upgrade and remove the evaluation build, and what an
operator has to know that the API page does not say.

## Install

**Container** (recommended for evaluation): build the image from this
repository (`docker build -t porta-eval .`). It runs as uid 10001, needs no
capabilities, works with a read-only root filesystem, and keeps all state in
`/var/lib/porta` — mount a volume there. See [distribution](distribution.md).

**Binary on a host**: install porta as for its other uses
([install](../../README.md#install)) or build it
(`almide build src/main.almd -o target/porta`). Then:

```bash
sudo useradd --system --home-dir /var/lib/porta --create-home porta
sudo install -d -o root -m 0755 /etc/porta /opt/porta/modules
sudo cp policy.toml /etc/porta/ && sudo cp *.wasm /opt/porta/modules/
sudo -u porta porta job-check --policy /etc/porta/policy.toml     # refuses a wrong digest or an unsafe grant
```

A minimal systemd unit:

```ini
[Service]
User=porta
Environment=HOME=/var/lib/porta
EnvironmentFile=/etc/porta/token.env          # PORTA_JOB_TOKEN=..., mode 0600, owned by root
ExecStart=/usr/local/bin/porta job-serve --policy /etc/porta/policy.toml --listen 127.0.0.1:8640
KillSignal=SIGTERM
TimeoutStopSec=15
Restart=on-failure
```

Rules that matter:

- `job-run` and `job-serve` refuse to run as root. Run them as a dedicated user.
- `HOME` must be set: the OS sandbox resolves the preset's `~/` paths.
- The policy file and module directory should be writable by root only. The
  policy refuses a *granted* directory that holds them, but cannot see who
  else can write them.
- Keep `--listen` on loopback and put TLS in front (a reverse proxy or the
  platform's load balancer). porta speaks plain HTTP.

## Remove

```bash
sudo systemctl disable --now porta-jobs      # or: docker rm -f porta-eval
sudo rm -rf /var/lib/porta /etc/porta /opt/porta   # records, workspaces, policy, modules
docker volume rm porta-eval-state && docker rmi porta-eval   # container installs
```

Nothing else is written outside these paths. porta keeps no global state
for the job service; `porta setup`, if it was ever run for other uses, has its
own `porta setup --undo`.

## Upgrade

1. Read the release notes for changes to `record_version`, the policy or the
   job format. In the evaluation build all three are version 1; a change in
   any of them is a breaking change and will be called out.
2. Stop the service with SIGTERM. Running jobs end as `service_shutdown`
   within 8 seconds and their records are complete. (A job is never resumed
   or re-run by an upgrade.)
3. Replace the binary or image. Run `porta job-check --policy …` with the new
   binary before starting it: it re-checks the policy and every module digest.
4. Start the service. Records written by the old version stay readable; each
   carries the `porta_version` that wrote it.

To roll back, do the same with the old binary. The record format has not
changed, so records written by the new version remain readable.

Changing the **policy** is the same procedure: edit, `job-check`, restart.
The new digest appears in every later record, so a record always says which
rules it ran under. The service does not reload the policy while running.

Changing a **module**: replace the file, update its `sha256` in the policy,
restart. Replacing the file without updating the policy makes every run of
that module `refused` / `module_changed` — by design.

## Records and logs

| Path | Content | Contains job data? |
|---|---|---|
| `records/<run_id>.json` | the run record | sizes and digests always; stdout/stderr text only with `retain_output = true` |
| `records/<run_id>/` | retained stdout, stderr, `/output` files | yes, with `retain_output = true` |
| `records/events.jsonl` | one line per state change | no: ids, outcome, reason, module name, policy digest |
| `workspaces/<run_id>/` | input copy, output, worker files | yes, while the run lasts; removed after it |
| stderr of the service | start line, shutdown line | no |

Never logged anywhere: the token, environment values, job input. Input file
*names* and environment variable *names* are in the record.

Retention is manual: `DELETE /v1/runs/{id}` removes a record and its retained
output (the event log keeps a `deleted` line). There is no automatic expiry.
Size `records/` for the number of runs you keep; with `retain_output` each
run can hold up to `max_output_bytes` of stdout plus the same in `/output`.

## Capacity and limits

- `max_concurrent` jobs run at once; the next is refused with 429 (no queue).
- Each job is one process. Memory per job is about its `memory_mib` plus
  wasmtime's own use plus compiled code — measure with your modules before
  setting the container's memory limit. `memory_mib` alone does not bound
  the process.
- Compiling a module happens in every run (a whole run of the 50 KB sample
  takes about 50–70 ms end to end on Apple silicon, compilation included). There is no compiled-code cache, deliberately:
  porta never loads native code from files a job could influence.
- Disk: a job can fill `/output` and any writable grant up to the volume's
  size (each file is capped at `max_output_bytes` + 1 MiB by `RLIMIT_FSIZE`,
  the number of files is not). Put `workspaces` on a size-limited volume.
- One service instance owns its `records` and `workspaces`. Two instances
  must not share them: each would mark the other's running jobs
  `interrupted` at start.

## Health and monitoring

`GET /healthz` answers `{"ok": true}` without a token while the HTTP loop
runs. It does not check the disk or the OS sandbox; the sandbox is checked
once at start, and the service refuses to start without it when the policy
requires it. Metrics endpoints do not exist yet; `events.jsonl` is the
machine-readable trail.
