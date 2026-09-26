<!-- description: scripts/monkey.py: random sequences of real operations, checked after every step -->
<!-- done: 2026-09-26 -->
# Monkey testing

## Why

The fuzzer throws one hostile input at one part: a mount name, a CONNECT
request, an argument vector. What it cannot reach is state carried from one
operation to the next — a snapshot, then another run, then a rollback; an
init, then an up; a run killed while a child is still alive.

## What

`scripts/monkey.py target/porta [--steps N] [--seed S]` plays in a fake home
with planted credentials, two writable mounts (one a git repository), a
read-only mount and a sentinel tree beside them, and takes random steps:
runs with random grants and random misbehaviour (writes everywhere, reading
the credentials, renaming the mount, writing through a symlink, a hook,
background processes, large output), `--snapshot` and `rollback`,
`explain --json`, `init` and `up`, and runs killed by a signal part way.

After every step: porta did not crash; the sentinel tree, the read-only
mount and the repository's hooks are unchanged unless the step dropped the
preset; no planted credential reached the output; `explain --json` is JSON;
a timed-out or killed run left no process; a rollback put the mounts back
exactly. With the sandbox bypassed the oracles catch a leaked credential, a
write outside and a changed hook within the first seeds.

## Found

- Two snapshots taken in the same second: their ids carried only the second,
  so `rollback` could restore the older one. Ids now carry the nanosecond
  (seed 13, on macOS and Linux).

CI runs seed 13 and one fresh seed per push.
