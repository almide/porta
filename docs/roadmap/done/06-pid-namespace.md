<!-- description: Run the Linux command in its own PID and mount namespace, so /proc shows only it -->
<!-- done: 2026-09-23 -->
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

## How it was built

`native/pid_namespace.rs`. Three processes, all made between fork and exec.
A is the one porta spawned. It enters the user, mount and PID namespaces and
maps its own uid and gid. B is pid 1 inside: it mounts the new `/proc`, reaps
orphans, and hands the command's wait status to A. C is the command: it takes
the ceilings, Landlock and seccomp as before, then execs.

- **pid 1 and signals.** B is pid 1, not the command. A ignores the signals a
  process group gets (INT, TERM, HUP, QUIT), so it can report how C ended. It
  then ends the same way, by the same exit code or by re-raising the same
  signal without a core dump. Timeout (124), SIGXCPU (152), SIGKILL at the
  memory ceiling (137) and a plain signal (143) all come out as before.
- **std's exec pipe.** A and B never exec, so each closes every descriptor
  above stderr once it has forked. C keeps the pipe, so a policy that cannot
  be applied, or an exec that fails, still reaches `spawn` as an error: a
  command without execute permission reports "Permission denied", exit 126.
- **Paths.** `run` (supervised) and the MCP `porta.exec` path both go through
  `spawned_command`. `wt_exec_replace` is declared but no command calls it; it
  is unchanged.
- **Ceilings.** Inside the namespace the kernel counts the namespace's
  processes against RLIMIT_NPROC, A and B included (measured: with the limit
  at 3, C's first fork was refused; at 4 it went through). porta adds those
  two, so `--max-procs N` now means the command's own N processes, not every
  process of the user. The memory ceiling no longer needs the self-stopping
  shell stub here. A sends its pid through a pipe and waits at a gate before
  it forks. porta places A in the scope from a thread while `spawn` waits, so
  everything the run starts is born inside the ceiling.
- **Hosts that refuse.** `pid_namespace::available()` asks once, by trying in
  a throwaway child. Where the answer is no, the run goes ahead without the
  namespace, prints one line in the default read mode, and `porta check`
  lists the primitive as absent with the reason. Checked with
  `user.max_user_namespaces=0`: both suites pass, and the new corpus row is
  reported as not hosted.

## Verified

In the Linux container (aarch64, kernel 6.12) with and without user
namespaces, on 2026-09-23:
- integration suite 23/23;
- escape corpus 22/22, including the new row "read another process's
  arguments (default reads)";
- by hand: the other process's `cmdline` read "No such file or directory",
  and `/proc` listed the run's own pids only.
