<!-- description: --why: what the sandbox refused, on Linux too, traced with strace -->
<!-- done: 2026-09-24 -->
# Why a run was refused, on Linux

## Why

After a failed run on macOS porta names each refusal and the flag it
needed, read from the unified log. Linux had nothing: Landlock logs its
denials (from ABI 7) only to the audit subsystem, which an unprivileged
process cannot read, so a refused write looked like a broken tool.

## What

- `--why` on Linux runs the command under `strace -f -Z`, which traces from
  outside the sandbox and keeps only the calls that failed. The traced
  process is porta again (`__sandboxed`), which applies the same request to
  itself and becomes the command, so the policy is the one an untraced run
  gets.
- The failed file, connect, bind and socket calls become the same refusals
  the macOS footer lists, with the same advice (`denial_advice.rs`, now
  shared). A call the file permissions refused is dropped: porta asks the
  same question outside the sandbox and keeps only what it is allowed.
- On macOS `--why` prints the footer whatever the exit code.
- After a failed Linux run at a terminal, porta says `--why` is there.
- `sandbox_exec.rs` was split first (checks, command, explain, linux,
  macos, why), with the profile, seccomp and namespace modules, so the
  quality grade had room for it.

## Evidence

- Integration: `--why` names a refused write with its `-v` grant and passes
  the exit code through; a clean run says nothing was refused.
- Unit: strace lines (split calls, relative paths, sockets, a permission
  error dropped).
