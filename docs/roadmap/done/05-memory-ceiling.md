<!-- description: A resident-memory ceiling for native runs through cgroup v2, fail-closed -->
<!-- done: 2026-09-23 -->
# Memory Ceiling for Native Runs

Shipped as `--max-memory-mb` (2026-09-23). The design below held with one
change: porta does not write the cgroup files itself. An unprivileged porta
usually lives in a login scope full of other processes, and cgroup v2 lets a
non-root cgroup hand a controller to its children only when it holds no
processes, so the delegated subtree is reached through the systemd user
manager instead — `StartTransientUnit` over the user bus with the stopped
child's pid, `MemoryMax` and `MemorySwapMax=0`, the call `systemd-run --user
--scope` makes for itself. The child cannot stop before exec (the parent's
`spawn` waits for an exec), so under a ceiling it starts as `/bin/sh -c 'kill
-STOP $$ && exec "$0" "$@"'` with Landlock and seccomp already on: the stub
stops, porta places that pid, reads `/proc/<pid>/cgroup` and `memory.max`
back, and only then SIGCONTs, whereupon the same pid becomes the command. Any
step short of that kills the child and refuses the run.
No user manager (a container, a bare CI runner, macOS) refuses the flag with
the reason. Proven in a systemd container: 200 MiB allocated under a 64 MiB
ceiling is OOM-killed (137); the same within 512 MiB runs; `porta check`
reports the primitive.

The plan as written before the work:

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
