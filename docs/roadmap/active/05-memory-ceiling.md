<!-- description: A resident-memory ceiling for native runs through cgroup v2, fail-closed -->
# Memory Ceiling for Native Runs

The one resource a native run cannot be bounded in today. `--timeout` bounds
wall-clock; `--max-cpu`, `--max-procs` and `--max-file-size` are rlimits the
kernel enforces on every descendant (2026-09-22). Memory has no rlimit that
means what people want: `RLIMIT_AS` caps virtual address space, which modern
runtimes reserve by the gigabyte, and `RLIMIT_RSS` is not enforced on Linux.
A resident-memory ceiling is a cgroup v2 `memory.max`, and only that.

## Design

- **Flag**: `--max-memory-mb <n>` for native runs (the WASM `--max-memory` is
  in pages and stays as it is). Carried by `explain`, `--json`, `--save` and
  `porta.toml` like the other ceilings.
- **Mechanism, Linux**: create a child cgroup under the subtree delegated to
  this user, write `memory.max` (and `memory.swap.max = 0`, so the ceiling is
  not paged around), move the child into it between fork and exec through
  `cgroup.procs`, and remove the cgroup after the wait. The delegated subtree
  is found from `/proc/self/cgroup` and `/sys/fs/cgroup/<path>`, and it is
  usable only when that directory is writable by this user — which systemd
  gives a login session (`user@UID.service` with `Delegate=yes`) and a
  container may or may not.
- **Fail closed**: no delegated subtree, or `memory` missing from
  `cgroup.controllers` there, refuses the run with the reason and what would
  fix it (`systemd-run --user --scope`, or a manager that delegates). Never
  silently run without the ceiling; never escalate to get one. This is the
  same rule as a Landlock ABI the kernel lacks.
- **macOS**: no cgroup, no equivalent the kernel enforces. The flag is refused
  there with that reason, the way `--allow-net` is refused where Landlock has
  no ABI 4. A supervisor that polls resident size through libproc and kills at
  the ceiling is possible, the way the CPU ceiling is done, and is the second
  step if the first proves useful; it is a kill after the fact, not a limit,
  and would be described as such.
- **`porta check`**: a Linux primitive row for "cgroup v2 delegated subtree
  with memory" — present or not, and what is refused without it.

## Proof

- Integration: an allocation loop under `--max-memory-mb 64` ends (OOM-killed,
  exit 137) with the host's memory untouched; a run within the ceiling keeps
  its exit code; a host without delegation refuses with exit 125 and names it.
- Corpus rows: "allocate past `--max-memory-mb`" and "fork to allocate past it"
  (the ceiling is the cgroup's, so children share it — the one ceiling that is
  per run, not per process).
- CI: the Linux runner's delegation state has to be established first; if the
  runner does not delegate, the test proves the refusal and the enforcement is
  proven in a container started with a delegated cgroup.

## Why not now

It is Linux-only, environment-gated and needs the delegation story tested on
real hosts before a flag is promised. The rlimit ceilings shipped first because
they hold everywhere porta runs.
