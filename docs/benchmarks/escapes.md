<!-- description: Escape attempts run against the porta binary, with results -->
# Escape corpus results

`scripts/escapes.py` run against the release binary. Regenerated per
release; this run is 2026-09-21, porta 0.6.0, on GitHub's arm64 runners.
A row is **held** when the escape was stopped, **ESCAPED** when it got
through (a finding, and a red build), and — when the platform cannot host
the attempt. Published whatever the result: a corpus that hid its losses
would not be evidence.

- **macOS arm64**: 9 tried, 0 escaped
- **Linux arm64**: 12 tried, 0 escaped

| Attempt | Category | macOS | Linux |
|---|---|---|---|
| write outside every mount | filesystem | held | held |
| rename the mount root away | filesystem | held | — |
| write a git hook inside a mount | filesystem | held | — |
| read an SSH private key | credentials | held | held |
| read /etc/shadow under strict | credentials | — | held |
| read another process's arguments | processes | held | held |
| read the login Keychain | credentials | held | — |
| start a program outside the sandbox | processes | held | — |
| reach a port the policy did not open | network | held | held |
| open a UDP socket in proxy mode | network | — | held |
| open a socket without socket() via io_uring | network | — | held |
| exec a memory file (fileless) | processes | — | held |
| attach to another process (ptrace) | processes | — | held |
| enter a new user namespace | processes | — | held |
| reach the network over MPTCP | network | — | held |
| open a raw socket | network | held | held |

Where a row is — on one platform, the escape is not expressible there:
the mount-internal and Keychain protections are macOS-only (Landlock grants
a directory whole), and the syscall-level attempts are Linux-only (the macOS
profile closes those channels differently, tested in the integration suite).

Run it yourself against any build:

```bash
python3 scripts/escapes.py "$(command -v porta)"
```

