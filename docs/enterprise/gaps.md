# What a product still needs

The evaluation build proves the core: a policy-checked job API whose limits
and denials hold and whose records say what happened. It is not something a
customer can buy yet. This is what is missing, ranked by how directly it
blocks a paying customer, with the evidence for each. "Evidence" points at
code, a test, or a measurement — not at an opinion.

## P0 — blocks a first paid deployment

| # | Gap | Why it blocks | Evidence |
|---|---|---|---|
| 1 | **No real-cloud run.** Every cloud target is a template. | A customer deploying to their cloud is the product. Whether the OS sandbox starts there is undocumented by every vendor. | [compatibility](compatibility.md): all five platforms "template" or "design only"; vendor docs say ND for Landlock/seccomp |
| 2 | **One shared token, no TLS.** No per-client identity, roles, rate limits or audit of *who* submitted a run. | Enterprise security review asks "who ran this" first. The record cannot answer it. | `http.rs`: one `PORTA_JOB_TOKEN`; record has no caller field |
| 3 | **Builds are not bit-for-bit reproducible.** Rust dependencies resolve at build time; wasmtime resolved to 47.0.4 while `almide.toml` names 47.0.3. | A supported product needs to rebuild the exact binary a customer runs, to ship a security fix against it. | [distribution](distribution.md); no committed `Cargo.lock`; base images by tag |
| 4 | **No network grants.** A job cannot reach any host at all. | Most agent work calls an API. Today such a job cannot run here; porta's proxy (`--proxy-allow`) exists for native commands but is not wired to jobs. | `spec.rs` rejects a `network` field; [threat model](threat-model.md) T6 |
| 5 | **Agents are not jobs yet.** `porta agent` (WASM decision loop, model broker, tools, journals) runs from the CLI only. | "WASM agent runtime" is the product's name; the API runs single modules. | `docs/agent-runtime.md`; no agent path in `native/job_service` |

## P1 — needed before a second customer

| # | Gap | Evidence |
|---|---|---|
| 6 | Full licence texts of all 312 crates are not shipped in the image, only the list. | [distribution](distribution.md) § Licences |
| 7 | Total disk use per job is not capped (only per file). | threat model T12 |
| 8 | Host memory beyond linear memory is not capped per job. | threat model, "not defended"; `memory_mib` is a store limit |
| 9 | No queue: the job after `max_concurrent` is refused with 429. | `supervisor.rs` `admit` |
| 10 | Records are plain files on one instance; no shared store, no retention policy, no export. Two instances cannot share a state directory. | [operations](operations.md) |
| 11 | Event log is not tamper-evident (no hash chain or signature). | threat model T17 |
| 12 | No metrics endpoint; health does not cover disk or sandbox after start. | [operations](operations.md) § Health |
| 13 | Component network denial is untested (no WASI 0.2 socket fixture). | threat model T6 |
| 14 | Concurrency isolation is tested with 2 jobs, not under load; no soak or fuzz of the HTTP parser. | `job_integration.py` |
| 15 | Linux x86-64 has not run the job suite (arm64 only). | [compatibility](compatibility.md) |

## P2 — product and commercial

| # | Gap | Note |
|---|---|---|
| 16 | Pricing, concurrency tiers, evaluation period, licence enforcement | Undecided by design; nothing built ahead of the decision. |
| 17 | Support process: severity levels, response targets, a security contact for the product | `SECURITY.md` covers the open-source project. |
| 18 | Customer-facing release notes and an upgrade compatibility promise for `record_version` / policy / job formats | All are version 1; [operations](operations.md) § Upgrade |
| 19 | Japanese and English contract-grade documentation, reviewed by someone other than the authors | These pages were written by the implementers. |
| 20 | Third-party security assessment | None has happened. Required before any claim stronger than "tested by its authors". |
| 21 | AgentCore adapter | Needs a decision on edge-only authentication ([note](../../deploy/aws-agentcore/README.md)). |

## Recommended order

1 → 2 → 4 → 3 → 5. A one-hour cloud run per platform (item 1) is cheap and
removes the largest unknown; identity (2) and network grants (4) are what an
evaluator will ask for next; reproducibility (3) must exist before the first
support contract; agents as jobs (5) is the product's headline but builds on
all of the above.

## Decisions that are not engineering's

- Approval and budget for the real-cloud runs in [compatibility](compatibility.md).
- Whether to publish the evaluation image to a registry, and under which name
  ("Porta Enterprise" is a working name; it has not been checked for
  trademark conflicts).
- Whether AgentCore's edge authentication may replace porta's token (item 21).
- Licence terms for the product, given porta itself is Apache-2.0.
