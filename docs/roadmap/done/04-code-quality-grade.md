<!-- description: Raise the codopsy project grade from B to A -->
<!-- done: 2026-09-20 -->
# Code Quality Grade

`codopsy analyze .` scores the repository **92 (A)** with the pinned v2.2.0
release. It scored 75 (B) when this item was opened. Grade A starts at 90, and
the CI floor stays there: the gate exists to catch a fall out of A, not to
ratchet a number.

The two numbers differ because the newer build fails to parse seven `.almd`
files outright and drops them from the report, while the pinned release parses
them partially and includes them. CI installs the pinned release through
`scripts/install-codopsy.sh` and fails below 90, so the floor is the grade-A
boundary rather than a number that moves with the analyzer.

## How the number is built

`overall = weighted_mean(file scores) - min(sqrt(total_issues) * 0.8, 15)`,
where each file's weight is `sqrt(function_count + 1)`, so files with many
functions dominate the mean. Each file score is complexity (35) + issues (40)
+ structure (25). The complexity component subtracts, per function,
`min((cyclomatic - 10) * 2, 15) + min((cognitive - 15) * 1.5, 12)`; the
structure component subtracts 10 for a file over 300 lines, up to 12 for
excess nesting depth and up to 10 for excess parameters. `max-*` warnings do
not touch the issues component but do count toward the global penalty.

Measured with the pinned v2.2.0 release:

| | Start | Now |
|---|---:|---:|
| Weighted mean | 86.1 | 98.1 |
| Issues | 176 | 56 |
| Penalty | 10.6 | 6.0 |
| **Overall** | **75 (B)** | **92 (A)** |

## What moved it

| Change | Effect |
|---|---|
| `locking::locked` replacing 40 `lock().unwrap()` | native 27 → 51 |
| Owned descriptors and one FFI boundary in `landlock.rs` | 81 → 91 |
| Deleting the dead WASM instruction decoder | `binary.almd` 64 → 87 |
| One module per thing the broker owns (`agent_runtime/`) | 42 → 100 |
| Splitting the FFI bridge by host function | `wasmtime_bridge.rs` 35 → 100 |
| One sandbox request document instead of six strings | `sandbox_exec.rs` 60 → 100 |
| Naming the stages in the journal replay and CONNECT handler | both → 100 |
| Per-event functions in the guest loop | `chat-agent` 53 → 99 |
| Option table and separate help module | `cli.almd` 61 → 97 |
| Each builtin's guards beside the builtin | `mcp.almd` 61 → 93 |
| Deleting the second WASM parser and the dead WASI interpreter | 776 lines |
| One job per function in three audit and eval helpers | all three → 100 |
| Disabling `no-assert`, `no-print`, `no-printf`, `no-println` | 515 issues |

The rule exclusions are in `.codopsyrc.json`, with the reason beside them:
standard output is this repository's interface, not its debugging channel.
Every Python file here is an integration test, an evaluation harness or an
auditor, where `assert` is the verification and `print` is the result; every
`println!` is either a WASM guest answering the host that reads its stdout, or
the proxy's audit line, which has to be on stderr because stdout carries
newline-delimited MCP JSON. No complexity threshold was relaxed.

Every step kept `almide test --ci` (110 tests), the macOS integration suites,
the official MCP SDK suite and the Linux container run passing before it was
committed. The two largest steps were removals, not rewrites: reading a module
through the engine that will run it left nothing calling the hand-rolled
parser, and the WASI implementation beside it had been unreferenced since the
wasmtime migration.

## What is left, and why

56 issues remain. 29 of them are one per `.almd` file: codopsy's Almide
grammar cannot parse every construct and reports the coverage gap as a
`syntax-error`. That is a property of the analyzer, not of this repository,
and it puts a floor of about 4.3 on the penalty. Worse, where coverage is poor
the reported function boundaries are wrong: `src/agent.almd` is 56% unparsed
and `src/engine.almd` 46%, so everything after the first unparsed region is
counted inside one function. That is why `engine.almd` is reported as holding
a function of cyclomatic complexity 23 when its largest is a short `validate`.
Splitting those files does not move the number, which is why the remaining
`.almd` complexity warnings stay.

Of the rest:

- **5 `no-unsafe`** — the Landlock syscall wrapper, `prctl`, `from_raw_fd`,
  `pre_exec`, and one in the bridge. Each is the minimum unsafe needed to
  reach the kernel, and each sits behind a checked boundary.
- **`max-complexity` on flat dispatch** — `parse_capability`, `print_help`,
  `apply_option`, the CLI command match in `mod.almd`. A flat match over names
  is already the clearest expression of what those do; grouping their arms to
  move a counter would make them worse.
- **`max-lines` on four evaluation harnesses** — `evaluate_containment.py`,
  `evaluate_agents.py`, `journal_integration.py` and `mcp_agent_integration.py`
  are each one linear scenario driver around one fixture service. Splitting a
  harness to move a line count makes it harder to audit, which is the one thing
  it exists for. Two of them are not run by CI at all, because they need a live
  model, so a refactor of those could not even be checked.
- **One `max-params`** — a test fixture whose five arguments all have defaults.
  Every way of reaching four makes the fixture harder to read.

## Rule

Raise a file when the refactor makes it easier to read. Do not split a
function whose shape is already the clearest expression of what it does, and
do not relax a threshold to clear a warning. A grade bought either way is not
evidence of anything.
