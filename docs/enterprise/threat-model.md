# Threat model: untrusted WASM jobs

This extends the [general threat model](../threat-model.md) to the job
service: a customer runs `porta job-serve` and submits WASM modules it did not
write, or that were written by something it does not trust (a model, a
third party). It says what the evaluation build defends, how, how that is
tested, and what it does not defend. "Tested" means a check in
[`scripts/job_integration.py`](../../scripts/job_integration.py) that runs the
real binary; nothing here has had a third-party audit.

## Parties and trust

| Party | Trusted? | Controls |
|---|---|---|
| Operator | yes | the policy file, the host, the module files, the token |
| API client | authenticated, not trusted with more than the policy | job requests: which registered module, input, files, which granted directories, limits up to the ceilings |
| Module (guest) | **no** | everything its code does at run time, including its trap messages and output |
| Worker process | trusted code, untrusted data | runs one guest; may be compromised if wasmtime is |

The module is the adversary. It is assumed to try to read or write beyond its
grants, reach the network, exhaust CPU, memory, disk or output, outlive its
deadline, poison what the next job sees, or make the host misreport what it
did.

## Boundaries

1. **WASM (primary).** wasmtime compiles the module; the guest can only call
   what is linked. Only WASI is linked (preview 1 for a core module, 0.2 for a
   component); porta links none of its own host functions. Directories are
   WASI preopens with read-only or read-write permissions; wasmtime-wasi
   confines `..` and symlinks to the preopen. Fuel, an epoch deadline and a
   per-store memory limiter bound CPU, time and linear memory.
2. **Worker process.** Each job runs in its own process
   (`porta __job-worker`): an empty environment, no stdin, its own process
   group, `RLIMIT_CPU` past the deadline, `RLIMIT_FSIZE` at the output limit,
   no core dumps, and on Linux `PR_SET_PDEATHSIG`. The supervisor kills the
   group at the deadline plus 5 s or on cancel.
3. **OS sandbox (defence in depth, `os_sandbox = "required"`).** The worker
   runs under `porta run`: Seatbelt on macOS; Landlock, seccomp and, where the
   host allows, namespaces on Linux. Writes only to its workspace and granted
   read-write directories; reads confined to those, the module directory and
   porta's own; no network. This holds if boundary 1 fails; it is porta's
   existing sandbox, tested by its [escape corpus](../benchmarks/escapes.md),
   not by a WASM escape (none is known to us to test with).

## Threats and controls

| # | Threat | Control | Evidence |
|---|---|---|---|
| T1 | Read host files outside the grant (`/etc/passwd`, `..` traversal, the worker's own `job.json`) | WASI preopens only; wasmtime-wasi path resolution; OS sandbox strict reads | `probe` job: 3 reads denied; tested |
| T2 | Write to a read-only grant or outside every mount | preopen `DirPerms::READ`/`FilePerms::READ`; no preopen elsewhere; OS sandbox | `probe` job: 3 writes denied, catalog unchanged; tested |
| T3 | Link or rename a read-only file into `/output` and write through it | wasmtime-wasi requires mutate permission on *both* directories for `link_at` / `rename_at` | source reading of wasmtime-wasi 47.0.3 `filesystem.rs`; not a test |
| T4 | Leave a symlink in `/output` so the host reads a host file into the record | the supervisor lists `/output` with `symlink_metadata`, opens with `O_NOFOLLOW`, and reports symlinks as ignored | `symlink-out` fixture; tested |
| T5 | Call a host function to run commands or make requests | nothing but WASI is linked; an unknown import fails instantiation | `host-import` fixture → `link_refused`; tested |
| T6 | Reach the network | WASI p1 has no outbound sockets; for a 0.2 component, socket and name-lookup permissions are left denied; OS sandbox `--no-net` | p1: by construction. Component sockets: **not tested** (no component fixture yet) |
| T7 | Read host credentials or environment | the worker starts from an empty environment; the guest gets only the job's `env`, whose names the policy lists; `PORTA_JOB_TOKEN` is removed from the server's environment | probe of HOME absent; `env_not_allowed` refusal; tested |
| T8 | Run forever | epoch deadline; supervisor kill at deadline + 5 s; `RLIMIT_CPU` | `spin` → `timeout` at ~2 s; tested |
| T9 | Exhaust CPU within the deadline | fuel | `spin` with small fuel → `fuel_exhausted`; tested |
| T10 | Exhaust memory | per-store memory limiter (`memory_mib`), instance/table/memory counts | `grow` → `memory_limit`, peak ≤ ceiling; tested |
| T11 | Flood output | stdout/stderr memory pipes with caps; `/output` total checked after exit | `flood` → `capped: true`; tested |
| T12 | Fill the disk through `/output` or a writable grant | `RLIMIT_FSIZE` per file only | **not controlled in total** — see limits |
| T13 | Outlive the server / leave processes behind | process group kill; PDEATHSIG (Linux); a parent watchdog in the worker (all platforms) | killed server → no worker left; tested |
| T14 | See or change another job's data | per-run workspace (0700) under its own run id; a fresh process and store per job; no shared writable grant unless the operator lists one | two concurrent jobs see only their own input; tested |
| T15 | Persist state into the next job | workspaces are removed after every run, including stopped ones; records are outside every mount | `workspace_removed`; tested |
| T16 | Replace the module after it was approved | SHA-256 pinned in the policy, checked at load and by every worker on the bytes it compiles | `module_changed`; tested |
| T17 | Make the record lie | the record is written by the supervisor, not the guest; the guest controls only its own output and trap text, which are bounded and labelled as such | design; partially tested |
| T18 | Escalate through the policy | a writable grant may not contain the policy or a module; state directories may not overlap a grant | load-time refusal; not yet a test |
| T19 | Unauthenticated API use | bearer token, constant-time compare; no-token mode only on loopback | tested |
| T20 | Oversized or slow requests | header ≤ 16 KiB, body ≤ `max_request_bytes`, 30 s socket timeouts, ≤ 64 concurrent connections | oversized body → 413; tested |

## What is not defended

- **A wasmtime or Cranelift bug** that lets a guest escape linear memory. The
  OS sandbox narrows what such an escape reaches; with `os_sandbox = "off"` it
  reaches whatever the service user can.
- **Kernel bugs.** Same kernel as the host; no VM.
- **Side channels** (timing, Spectre-class) between jobs on one host.
- **Total disk use** by a job: `RLIMIT_FSIZE` bounds each file, not their
  number. Put `workspaces` and any writable grant on a size-limited volume.
- **Host memory outside linear memory**: wasmtime's own allocations, compiled
  code and the worker process are not capped by `memory_mib`. Cap the
  container or service instead.
- **CPU fairness** between concurrent jobs; `max_concurrent` bounds how many.
- **A malicious operator or client with the token.** There is one token with
  full access, no per-client identity, no roles, no rate limit, no TLS.
- **Record integrity** against someone with write access to `records/`. The
  event log is not signed or chained.
- **Data at rest**: records and retained output are plain files; encryption
  is the volume's job.
- **Network grants.** There are none; a job that needs the network cannot run.
