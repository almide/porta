<!-- description: Escape attempts run against the porta binary, with results -->
# Escape corpus results

`scripts/escapes.py` run against the built binary. Regenerated per release;
this run is 2026-09-22, porta 0.6.2, macOS arm64
locally and Linux x86_64 on the CI runner (run 35703962916). A row is **held** when the escape was
stopped, **ESCAPED** when it got through (a finding, and a red build), and —
when the platform cannot host the attempt. Published whatever the result: a
corpus that hid its losses would not be evidence.

- **macOS arm64**: 17 tried, 0 escaped
- **Linux aarch64** (a systemd container with a user manager, 2026-09-23): 21 tried, 0 escaped
- **Linux x86_64** (the CI runner, no user manager, so the memory row is not
  hosted there): 20 tried, 0 escaped

## A correction to earlier runs

The runs published before this one (13/13 and 16/16, 2026-09-21) were not
evidence. The harness passed the target after `--`, porta answered with its
usage text and exit 0, and nothing ran; every row judged an escape by its
absence therefore passed. Found while adding the resource rows, whose positive
signals did not appear. The harness now puts the target where porta reads it,
treats a usage reply as a fatal harness error rather than a verdict, and
begins with a canary run that must write into the granted workspace and say
so before any row is scored. The numbers above are from that harness; the
earlier ones happened to match, which is luck, not proof, and is recorded here
so the claim and its evidence are not confused again.

A second correction followed on the first CI run of the repaired harness: the
Launch Services row read as an escape on the macOS runner. It was not one. The
row had judged `open(1)`'s messages and counted porta's own denial footer as
the held signal; `open` says "Unable to find application" whether or not its
lookup was denied, and when the footer lagged behind the unified log the row
misread. It now asks the bootstrap server for the Launch Services mach
services directly and reads the kernel's answer, and no row's verdict depends
on anything porta prints.

| Attempt | Category | macOS | Linux |
|---|---|---|---|
| write outside every mount | filesystem | held | held |
| rename the mount root away | filesystem | held | — |
| write a git hook inside a mount | filesystem | held | — |
| write through a symlink pointing outside the mount | filesystem | held | held |
| read a secret through a symlink under strict | credentials | held | held |
| inherit an open file descriptor from porta | processes | held | held |
| read an SSH private key | credentials | held | held |
| read /etc/shadow under strict | credentials | — | held |
| read another process's arguments | processes | held | held |
| read the login Keychain | credentials | held | — |
| start a program outside the sandbox | processes | held | — |
| reach a port the policy did not open | network | held | held |
| reach the cloud metadata endpoint via the proxy | network | held | held |
| open a UDP socket in proxy mode | network | — | held |
| open a socket without socket() via io_uring | network | — | held |
| exec a memory file (fileless) | processes | — | held |
| attach to another process (ptrace) | processes | — | held |
| enter a new user namespace | processes | — | held |
| reach the network over MPTCP | network | — | held |
| open a raw socket | network | held | held |
| fork past `--max-procs` | resources | held | held |
| grow a file past `--max-file-size` | resources | held | held |
| burn CPU past `--max-cpu` | resources | held | held |
| ignore SIGXCPU and keep burning | resources | held | held |
| allocate past `--max-memory-mb` | resources | — | held |

Where a row is — on one platform, the escape is not expressible there:
the mount-internal and Keychain protections are macOS-only (Landlock grants
a directory whole), and the syscall-level attempts are Linux-only (the macOS
profile closes those channels differently, tested in the integration suite).
The memory row is a cgroup v2 ceiling placed through the systemd user
manager, so it is Linux-only and needs a user manager; where there is none the
row is not hosted and the flag is refused. The other resource rows are kernel
rlimits and hold on both; on Linux the one that
ignores SIGXCPU ends in the kernel's SIGKILL at the hard limit (exit 137), on
macOS in porta's own kill (exit 152). The SIGXCPU row exists because
the first macOS run of the CPU ceiling showed a program that ignores the signal
running on: macOS never follows SIGXCPU with SIGKILL as Linux does, so porta's
supervisor now measures the process group's CPU and kills it at the ceiling.

Run it yourself against any build:

```bash
python3 scripts/escapes.py "$(command -v porta)"
```
