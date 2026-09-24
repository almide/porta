<!-- description: What a run closes is a preset (data), not paths written into the core -->
<!-- done: 2026-09-24 -->
# Closures as a preset

## Why

The credential stores porta closed to reads, the names it protected inside a
writable mount and the credential sockets it refused were three lists in Rust:
`~/.aws` was in the enforcement code. A path the list did not name — the
GitHub CLI's token in `~/.config/gh` — was readable, and the only fix was a
release. The lists were also macOS-only: on Linux, default reads left every
credential store open, which the escape corpus hid by running its SSH row
under strict reads.

## What

- `native/presets/default.toml`, built into the binary: `[read] deny`,
  `[write] protect`, `[unix] deny`. The core holds only the mechanisms.
- `--deny-read`, `--protect`, `--deny-unix` add to it; `--preset none` drops
  it; `--preset <file>` replaces it. The same keys work in `porta.toml`.
  `porta explain` (and `--json`) prints what a run resolved.
- Linux enforces the same closures. In the command's mount namespace each
  closed directory is covered by an empty read-only tmpfs and each closed file
  by `/dev/null`; each protected name, `.git/hooks` and `.git/config` is
  bind-mounted read-only onto itself, and the mount root and `.git` are pinned
  by a bind mount so they cannot be renamed. Where the host refuses the
  namespace, Landlock grants reads everywhere except beneath the closed paths,
  and a closed path inside a grant refuses the run.

## Evidence

- Escape corpus: "read the GitHub CLI token" added; the SSH row runs in
  default reads on Linux; the rename and git-hook rows run on Linux.
- Integration: the preset closes by default, `--preset none` opens,
  `--deny-read` closes, bad values exit 125.
