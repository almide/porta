<!-- description: Raise the codopsy project grade from B to A -->
# Code Quality Grade

**Priority: Medium**

`codopsy analyze .` scores the repository **75 (B)**. Grade A starts at 90.

## How the number is built

`overall = weighted_mean(file scores) - min(sqrt(total_issues) * 0.8, 15)`, where
each file's weight is `sqrt(function_count + 1)`, so large files with many
functions dominate. Today: weighted mean **86.1**, 176 issues, penalty **10.6**.

Reaching 90 therefore needs the mean near 97 *and* issues near 40. Fixing only
the worst files is not enough: with the two lowest-scoring files at 90 and
issues at 100, the projection is 83.

## What has been done

| Change | Effect |
|---|---|
| `locking::locked` replacing 40 `lock().unwrap()` | native 27 → 51 |
| Owned descriptors and one FFI boundary in `landlock.rs` | 81 → 88 |
| Deleting the dead WASM instruction decoder | `binary.almd` 64 → 87 |
| Splitting `wasmtime_bridge.rs` into proxy and sandbox modules | 35 → 51, no F files left |
| Disabling `no-assert`, `no-print`, `no-printf` | 510 issues that were all in `scripts/` |

The rule exclusions are in `.codopsyrc.json` and apply to Python only. Every
Python file in this repository is an integration test, an evaluation harness or
an auditor, where `assert` is the verification and `print` is the output.
Thresholds were left at their defaults; none of the complexity limits were
relaxed.

## What remains, and the honest cost

The remaining deficit is a long tail rather than a few outliers:

| File | Score | What it would take |
|---|---:|---|
| `native/agent_runtime.rs` | 41 | `load` (cyclomatic 42) and `tick` (26) split into stages |
| `native/sandbox_exec.rs` | 60 | `build_sandbox_profile_rs` (cognitive 22) split per control |
| `src/mcp.almd` | 61 | dispatch complexity |
| `examples/chat-agent/src/mod.almd` | 53 | the guest loop |
| `scripts/mcp_agent_integration.py` | 55 | long linear test bodies |
| ~20 more files at 60–85 | | mostly `max-complexity` and `max-depth` |

Much of this is worth doing on its own merits — `load` and `tick` are the two
functions a reader of the broker has to understand first. Some of it is not:
splitting a linear integration test into helpers to move a depth counter makes
the test harder to read, and that is the point where chasing the grade would
start costing more than it returns.

## Rule

Raise a file when the refactor makes it easier to read. Do not split a function
whose shape is already the clearest expression of what it does, and do not relax
a threshold to clear a warning. A grade bought either way is not evidence of
anything.
