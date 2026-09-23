<!-- description: Run the Linux command in its own PID and mount namespace, so /proc shows only it -->
# Own PID namespace on Linux

## Why

The [competitor comparison](../../benchmarks/competitors.md#where-porta-loses)
shows the one row where porta does worse than srt and Fence. In the default
read mode on Linux, a sandboxed `cat /proc/<pid>/cmdline` reads another
process's command line, secrets in it included. srt and Fence start the
command in its own PID namespace, where that process does not exist. porta
closes `/proc` only under `--read-policy strict`, and closes all of it, which
some programs cannot run without.

Landlock cannot do this. It is an allow-list: a grant on `/` covers every
`/proc/<pid>`, and granting `/proc` minus the pid directories also takes away
`/proc/self`. `/proc/self` resolves to the caller's own pid directory, which
is not known when the rules are built.

## What

Where the kernel lets an unprivileged user create a user namespace, the
command runs in new user, PID and mount namespaces, with a fresh `/proc`
mounted in the mount namespace. It sees itself and its descendants and
nothing else, and `/proc/self` works. The uid map maps the caller's uid to
itself, not to root, so after exec the command holds no capability, and the
seccomp baseline still refuses every further namespace.

Checked in the Linux container on 2026-09-23 with `unshare --user --pid
--fork --mount --mount-proc`. As uid 1001, `/proc` listed four pids, and
another process's `cmdline` was "No such file or directory".

## Design problems to solve first

- **pid 1 ignores signals it has no handler for.** Signals from inside the
  namespace are dropped, and from an ancestor namespace only SIGKILL and
  SIGSTOP get through. That includes the kernel's soft-limit SIGXCPU.
  Running the command itself as pid 1 would make Ctrl-C and `--max-cpu`
  behave differently. A small init must be pid 1 instead: fork the command,
  forward signals, reap orphans, and leave with the command's status.
- **The fork has to happen inside `pre_exec`.** `unshare(CLONE_NEWPID)` moves
  only later children. The process std spawned has to fork once more and wait,
  without allocating. It must also close its copy of std's exec-status pipe,
  or `spawn()` blocks until the whole run ends.
- **Every Linux path.** `run` (supervised), `exec` (replace, which has no fork
  today) and the captured path used by MCP all share one policy generation
  (security invariant) and must all get the same isolation.
- **Ceilings.** RLIMIT_NPROC is counted per user namespace since 5.14. The
  memory ceiling places the stopped stub's pid in a scope. Both need
  re-checking against the extra processes.
- **Where user namespaces are refused.** Ubuntu 23.10 and later ship
  `kernel.apparmor_restrict_unprivileged_userns=1`, and some kernels have
  `unprivileged_userns_clone=0`. The run then either carries on without the
  namespace, or refuses. It is defence in depth, not a flag the caller asked
  for, so refusing would put porta out of reach on a stock Ubuntu desktop.
  The default: carry on, say so on stderr once, and report it in
  `porta check`. `--read-policy strict` still closes `/proc` there.

## Done when

- A corpus row, in default read mode, "read another process's arguments"
  holds on Linux wherever user namespaces are available. Where they are not,
  the row is not hosted.
- Ctrl-C, `--timeout`, `--max-cpu` (both SIGXCPU rows) and
  `--max-memory-mb` give the same exit codes as today.
- `porta check` says whether this host gives the namespace.
