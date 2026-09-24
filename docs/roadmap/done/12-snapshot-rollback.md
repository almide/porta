<!-- description: --snapshot and porta rollback: see what a run changed, and put it back -->
<!-- done: 2026-09-24 -->
# Putting a run's writes back

## Why

A mount is writable because the agent has to change it, and the policy
cannot tell a good change from a bad one. nono can roll a session back;
porta could only say afterwards, at best, which writes it refused.

## What

- `porta run … --snapshot` copies every writable mount to
  `~/.porta/snapshots/<id>` before the command starts: one `clonefile(2)` on
  APFS, so a large tree costs next to nothing; file by file on Linux, where
  `std::fs::copy` reflinks on filesystems that can.
- After the run porta lists what it added (`+`), changed (`~`) and removed
  (`-`) in each mount.
- `porta rollback [id]` shows the same against the snapshot (the latest by
  default); with `--yes` it removes what was added and restores what was
  changed or removed.
- The snapshots are outside every mount, so the run cannot rewrite its own
  undo; a mount that holds `~/.porta` refuses `--snapshot`. The ten latest
  are kept.

## Evidence

- Integration: a run edits, deletes and adds; the report lists each;
  `rollback --yes` restores the tree; the run's attempt to write into the
  snapshot directory is refused.
- Unit: every kind of change, including a directory removed with its
  contents and a symlink retargeted, described and put back.
