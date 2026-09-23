<!-- description: The escape corpus run against porta and four other sandbox tools, with every losing row -->
# porta against other sandbox tools

The [escape corpus](escapes.md) run under five sandboxing tools, each with the
same attempts, the same probes and the same verdicts. Where porta loses, the
row is published as it is. The raw results are in
[`2026-09-23-competitors/`](2026-09-23-competitors/), one JSON file per tool and
platform.

| Tool | Version | Platforms run |
|---|---|---|
| porta | 0.6.4 (2026-09-23) | macOS, Linux |
| [srt](https://github.com/anthropics/sandbox-runtime) (Anthropic sandbox-runtime) | 0.0.77 (npm) | macOS, Linux |
| [Fence](https://github.com/fencesandbox/fence) | 737751a (2026-09-11) | macOS, Linux |
| [nono](https://github.com/nolabs-ai/nono) | 0.78.0 | macOS, Linux |
| [landrun](https://github.com/Zouuup/landrun) | 0.1.18, 811cfff (2026-07-23) | Linux only |

- **macOS**: 26.3, arm64.
- **Linux**: Ubuntu 24.04 in a privileged Docker container on the same Mac,
  kernel 6.12 (linuxkit), aarch64, Landlock ABI 6, systemd user manager
  running, no Yama.

Every tool ran as the same unprivileged user.

## How to read it

A row states its policy in porta's flags. For each other tool,
[`scripts/escape_runners.py`](../../scripts/escape_runners.py) translates those
flags into that tool's nearest setting. Each translation is written out in
that file's header. A row gets one of four results:

- **held**: the tool stopped the attempt.
- **ESCAPED**: the attempt got through. On Linux, one row asks whether a user
  namespace can be entered. That is not a way out of the policy; it is a step
  an escape would start from (new namespaces expose kernel code an
  unprivileged process otherwise cannot reach). It is labelled *hardening*.
- **not offered**: the tool has no setting for the row's policy. Examples are
  a CPU ceiling, or a TCP port rule on a tool without port grants. Such a row
  is not held: nothing was enforced.
- **—**: the row does not apply, or the tool's design makes it meaningless.
  nono hands its child a socket to its own supervisor on purpose, so the
  descriptor-inheritance row does not score it.

A row that runs with no flags runs each tool **with its own defaults**. That
measures what a user gets out of the box. It is not a claim that a tool
cannot be configured to hold the row. srt and Fence, for example, can be told
to deny reads of `~/.ssh`, but they do not deny it unless told.

Every tool is started in the row's first writable mount, or in an empty
directory when the row grants none.

## macOS arm64

| Attempt | porta | srt | Fence | nono |
|---|---|---|---|---|
| **tried / escaped / not offered** | **18 / 0 / 0** | 13 / 4 / 5 | 11 / 4 / 7 | 10 / 4 / 7 |
| write outside every mount | held | held | held | held |
| rename the mount root away | held | held | held | **ESCAPED** |
| write a git hook inside a mount | held | held | held | **ESCAPED** |
| write through a symlink pointing outside the mount | held | held | held | held |
| read a secret through a symlink under strict | held | held | held | held |
| inherit an open file descriptor | held | held | held | — |
| read an SSH private key | held | **ESCAPED** | **ESCAPED** | held |
| read another process's arguments | held | **ESCAPED** | **ESCAPED** | **ESCAPED** |
| read the login Keychain | held | **ESCAPED** | **ESCAPED** | held |
| reach Launch Services | held | **ESCAPED** | **ESCAPED** | **ESCAPED** |
| reach a port the policy did not open | held | held | not offered | not offered |
| reach the cloud metadata endpoint via the proxy | held | held | held | held |
| open a raw socket | held | held | not offered | not offered |
| fork past `--max-procs` | held | not offered | not offered | not offered |
| grow a file past `--max-file-size` | held | not offered | not offered | not offered |
| burn CPU past `--max-cpu` | held | not offered | not offered | not offered |
| ignore SIGXCPU and keep burning | held | not offered | not offered | not offered |
| allocate past `--max-memory-mb` | held | not offered | not offered | not offered |

What the escapes are:

- **SSH key, Keychain** (srt, Fence, defaults). Both tools allow reads
  everywhere unless told otherwise. `~/.ssh/id_*` and
  `~/Library/Keychains/login.keychain-db` were read from a run that granted
  one writable directory and nothing else. nono and porta close credential
  stores in every mode.
- **Another process's arguments**. `sysctl(KERN_PROCARGS2)` on a process the
  sandbox did not start returned its command line, and with it the secret
  token that command line carried.
- **Launch Services**. The sandbox could reach `com.apple.lsd.mapdb`
  (nono also `modifydb`). That is the service `open(1)` asks to start an
  application, and the application runs outside the sandbox.
- **Mount root, git hook** (nono). Inside a directory granted for writing,
  nono let `.git/hooks/pre-commit` be written and the directory itself be
  renamed away. A written hook runs as the operator at the next `git commit`,
  unsandboxed. porta keeps a mount's hooks, repository config and a host tool's
  trusted root files unwritable, and pins the mount root in place. srt and Fence
  held both rows.
- **Ports** (Fence, nono on macOS). Neither can grant a single TCP port on
  macOS. nono refuses the request itself ("Seatbelt cannot filter by TCP
  port").

## Linux aarch64

| Attempt | porta | srt | Fence | nono | landrun |
|---|---|---|---|---|---|
| **tried / escaped / not offered** | **22 / 0 / 0** | 17 / 0 / 5 | 16 / 0 / 6 | 18 / 2 / 3 | 16 / 1 / 6 |
| write outside every mount | held | held | held | held | held |
| write through a symlink pointing outside the mount | held | held | held | held | held |
| read a secret through a symlink under strict | held | held | held | held | held |
| inherit an open file descriptor | held | held | held | — | held |
| read an SSH private key | held | held | held | held | held |
| read /etc/shadow under strict | held | held | held | held | held |
| read another process's arguments (strict reads) | held | held | held | **ESCAPED** | held |
| read another process's arguments (default reads) | held | held | held | **ESCAPED** | held |
| reach a port the policy did not open | held | held | not offered | held | held |
| reach the cloud metadata endpoint via the proxy | held | held | held | held | not offered |
| get a UDP answer from outside in proxy mode | held | held | held | held | **ESCAPED** |
| open a socket without `socket()` via io_uring | held | held | held | held | held |
| exec a memory file (fileless) | held | held | held | held | held |
| attach to a process outside the sandbox (ptrace) | held | held | held | held | held |
| *hardening:* enter a new user namespace | held | **gap** | **gap** | **gap** | **gap** |
| reach the network over MPTCP | held | held | held | held | held |
| open a raw socket | held | held | held | held | held |
| fork past `--max-procs` | held | not offered | not offered | held | not offered |
| grow a file past `--max-file-size` | held | not offered | not offered | not offered | not offered |
| burn CPU past `--max-cpu` | held | not offered | not offered | not offered | not offered |
| ignore SIGXCPU and keep burning | held | not offered | not offered | not offered | not offered |
| allocate past `--max-memory-mb` | held | not offered | not offered | held | not offered |

What the escapes are:

- **UDP** (landrun). Landlock's network rules cover TCP only. Under a policy
  that granted no network, landrun refused a TCP connection to `1.1.1.1:53`
  but let a UDP DNS question to the same address get its answer. porta has the
  same Landlock limit, so it adds a seccomp filter that refuses every socket
  family a TCP rule cannot see. srt, Fence and nono isolate the network in a
  namespace or proxy.
- **Another process's arguments** (nono, in both read modes).
  `/proc/<pid>/cmdline` of a process outside the sandbox was readable. The
  other four hide it in different ways. srt, Fence and porta run the command
  in a PID namespace of its own, where that process does not exist. landrun
  grants nothing under `/proc`, so all of it is closed.
- **User namespaces** (hardening gap in all four). `unshare(CLONE_NEWUSER)`
  succeeded inside the sandbox. Fence refuses the `unshare` *command* by name,
  but the same call made from Python went through. porta's seccomp baseline
  refuses the syscall.

Apart from those, srt and Fence held every Linux row porta held. The
differences on Linux are the resource ceilings and the hardening row.

**Ceilings.** nono has two: `--memory` and `--max-processes`, both through
cgroup v2. Both held. They need the run started inside the systemd user
manager's cgroup, as a desktop terminal is. From a session outside it (SSH,
`docker exec`), nono refuses to start, which is the right failure. The
corpus ran nono under `systemd-run --user --scope` so the rows could be
tried. porta asks the user manager for a scope itself and needs no wrapper.
On macOS nono refuses both ("resource limits are only enforced on Linux").
Fence documents CPU, memory, disk and fork bombs as out of scope. srt and
landrun have no ceiling settings.

landrun runs with `--best-effort`. It targets Landlock ABI 9 and otherwise
refuses a kernel older than that. A row in proxy mode runs landrun with no
network grant at all, which is stricter than a proxy. The one row whose point
is allowing a host (cloud metadata) is not offered.

## Where porta loses

- **No network namespace.** srt and Fence run the command in a network
  namespace of its own, and it reaches the network only through their proxy.
  porta leaves it in the host's network namespace. Landlock's TCP port rules
  and a seccomp filter refuse every other socket family and every other
  address. The network rows above show no escape from that difference. It is
  still a layer porta does not have.
- **Hosts that refuse unprivileged user namespaces.** Examples are Ubuntu 23.10
  and later with `kernel.apparmor_restrict_unprivileged_userns=1`, and most
  containers. porta then cannot create the PID namespace. The run goes ahead
  in the host's, says so on stderr, and `porta check` shows it. There, the
  default read mode leaves other processes' `/proc` entries readable, and only
  `--read-policy strict` closes them. srt and Fence depend on bubblewrap, which
  needs the same kernel permission (or an AppArmor profile that grants it).
- **Features the others have and porta does not.** Fence refuses commands by
  name, a policy layer above the kernel. nono has a supervisor that can grant
  the sandbox more access at run time after a prompt, rollback sessions, and
  detachable sessions. Fence and srt can expose an inbound port to the
  sandboxed command. porta has none of these.

This section named one more loss when first published on 2026-09-23. In
porta's default read mode on Linux, a sandboxed `cat /proc/<pid>/cmdline`
read another process's command line, token included, where srt and Fence
hid it. porta now runs the command in PID, mount and user namespaces of its
own on Linux, with a fresh `/proc` (see
[the roadmap item](../roadmap/done/06-pid-namespace.md)). The corpus row
"read another process's arguments (default reads)" was added to keep it
closed.

## Run it yourself

```bash
python3 scripts/escapes.py "$(command -v porta)"
python3 scripts/escapes.py "$(command -v srt)"     --runner srt
python3 scripts/escapes.py "$(command -v fence)"   --runner fence
python3 scripts/escapes.py "$(command -v nono)"    --runner nono
python3 scripts/escapes.py "$(command -v landrun)" --runner landrun   # Linux
```

Install each tool somewhere outside `$HOME`. Rows under strict reads hide the
home directory, and a tool installed there cannot load its own helpers. srt's
seccomp helper is one example.
