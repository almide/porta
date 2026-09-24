<!-- description: The escape corpus run against porta and four other sandbox tools, with every losing row -->
# porta against other sandbox tools

The [escape corpus](escapes.md) run under five sandboxing tools, each with the
same attempts, the same probes and the same verdicts. Where porta loses, the
row is published as it is. The raw results are in
[`2026-09-24-competitors/`](2026-09-24-competitors/), one JSON file per tool and
platform.

| Tool | Version | Platforms run |
|---|---|---|
| porta | 0.6.11 (2026-09-24) | macOS, Linux |
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
| **tried / escaped / not offered** | **22 / 0 / 0** | 17 / 5 / 5 | 14 / 5 / 8 | 13 / 5 / 8 |
| write outside every mount | held | held | held | held |
| rename the mount root away | held | held | held | **ESCAPED** |
| write a git hook inside a mount | held | held | held | **ESCAPED** |
| write through a symlink pointing outside the mount | held | held | held | held |
| read a secret through a symlink under strict | held | held | held | held |
| inherit an open file descriptor | held | held | held | — |
| read an SSH private key | held | **ESCAPED** | **ESCAPED** | held |
| read the GitHub CLI token | held | **ESCAPED** | **ESCAPED** | held |
| reach the SSH agent's socket | held | held | held | **ESCAPED** |
| read another process's arguments | held | **ESCAPED** | **ESCAPED** | **ESCAPED** |
| read the login Keychain | held | **ESCAPED** | **ESCAPED** | held |
| reach Launch Services | held | **ESCAPED** | **ESCAPED** | **ESCAPED** |
| reach a port the policy did not open | held | held | not offered | not offered |
| get a UDP answer under a TCP port rule | held | held | not offered | not offered |
| reach anything with no network granted | held | held | held | held |
| reach the cloud metadata endpoint via the proxy | held | held | held | held |
| open a raw socket | held | held | not offered | not offered |
| fork past `--max-procs` | held | not offered | not offered | not offered |
| grow a file past `--max-file-size` | held | not offered | not offered | not offered |
| burn CPU past `--max-cpu` | held | not offered | not offered | not offered |
| ignore SIGXCPU and keep burning | held | not offered | not offered | not offered |
| allocate past `--max-memory-mb` | held | not offered | not offered | not offered |

What the escapes are:

- **SSH key, GitHub CLI token, Keychain** (srt, Fence, defaults). Both tools
  allow reads everywhere unless told otherwise. `~/.ssh/id_*`,
  `~/.config/gh/hosts.yml` and `~/Library/Keychains/login.keychain-db` were
  read from a run that granted one writable directory and nothing else. nono
  and porta close credential stores in every mode; porta's list is a preset
  the caller can read, extend or replace.
- **Another process's arguments**. `sysctl(KERN_PROCARGS2)` on a process the
  sandbox did not start returned its command line, and with it the secret
  token that command line carried.
- **Launch Services**. The sandbox could reach `com.apple.lsd.mapdb`
  (nono also `modifydb`). That is the service `open(1)` asks to start an
  application, and the application runs outside the sandbox.
- **SSH agent** (nono). A listener at the path ssh-agent uses
  (`ssh-XXXX/agent.<pid>`) outside every grant accepted a connection: whoever
  reaches the agent signs with the caller's keys without reading them. srt,
  Fence and porta refuse the socket by default.
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
| **tried / escaped / not offered** | **28 / 0 / 0** | 23 / 2 / 5 | 21 / 3 / 7 | 24 / 5 / 3 | 22 / 7 / 6 |
| write outside every mount | held | held | held | held | held |
| rename the mount root away | held | held | held | **ESCAPED** | **ESCAPED** |
| write a git hook inside a mount | held | held | held | **ESCAPED** | **ESCAPED** |
| write through a symlink pointing outside the mount | held | held | held | held | held |
| read a secret through a symlink under strict | held | held | held | held | held |
| inherit an open file descriptor | held | held | held | — | held |
| read an SSH private key | held | **ESCAPED** | **ESCAPED** | held | **ESCAPED** |
| read the GitHub CLI token | held | **ESCAPED** | **ESCAPED** | held | held |
| reach the SSH agent's socket | held | held | **ESCAPED** | **ESCAPED** | **ESCAPED** |
| read /etc/shadow under strict | held | held | held | held | held |
| read another process's arguments (strict reads) | held | held | held | **ESCAPED** | held |
| read another process's arguments (default reads) | held | held | held | **ESCAPED** | held |
| reach a port the policy did not open | held | held | not offered | held | held |
| reach anything with no network granted | held | held | held | held | **ESCAPED** |
| reach the cloud metadata endpoint via the proxy | held | held | held | held | not offered |
| get a UDP answer from outside in proxy mode | held | held | held | held | **ESCAPED** |
| get a UDP answer under a TCP port rule | held | held | not offered | held | **ESCAPED** |
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

- **SSH key, GitHub CLI token** (srt, Fence, landrun). The row grants the
  home and runs with each tool's default reads. srt and Fence read everywhere
  unless told otherwise; landrun cannot close a path inside a grant, and held
  the token row only because that row grants no directory it lies in. nono
  closes credential stores; porta covers each path its preset names with an
  empty mount in the command's mount namespace. Before 2026-09-24 this row ran
  on Linux under strict reads, where every tool held it, and so hid that
  porta's own default reads left these stores open on Linux.
- **Mount root, git hook** (nono, landrun). Inside a directory granted for
  writing, both let `.git/hooks/pre-commit` be written and the directory be
  renamed away: Landlock grants a directory whole. porta bind-mounts the hooks,
  the repository config and the preset's protected names read-only onto
  themselves, and pins the mount root with a bind mount. srt and Fence held
  both, through bubblewrap's read-only binds.
- **SSH agent** (Fence, nono, landrun). A listener at ssh-agent's path
  outside every grant accepted a connection. Landlock has no rule over a Unix
  socket's `connect`; porta covers each credential socket its preset names
  with `/dev/null` in the command's mount namespace, and srt's bubblewrap
  mount leaves it out. Until 2026-09-24 porta closed these sockets on macOS
  only.
- **UDP** (landrun, in all three network rows). Landlock's network rules cover TCP
  only. Under a policy that granted no network, landrun refused a TCP
  connection to `1.1.1.1:53` but let a UDP DNS question to the same address
  get its answer; so did one under a TCP port rule. porta has the
  same Landlock limit, so it adds a seccomp filter that refuses every socket
  family a TCP rule cannot see, and under a port rule every internet socket
  that is not TCP (names then resolve over TCP 53). Until 2026-09-24 porta
  left UDP open under `--allow-net` too. srt, Fence and nono isolate the network in a
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

Apart from the credential rows, srt and Fence held every Linux row porta
held. The other differences on Linux are the resource ceilings and the
hardening row.

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

- **The network is open by default.** A run with no network flag reaches
  the network as a Docker container does; `--no-net`, `--allow-net` or
  `--proxy-allow` close it. srt and Fence start closed. The row "reach
  anything with no network granted" runs porta with `--no-net` and the others
  with their defaults.
- **A network namespace only under `--no-net`.** srt and Fence run every
  command in a network namespace of its own and let it out only through
  their proxy. porta gives the command its own namespace (loopback only)
  under `--no-net`. Under `--allow-net` and `--proxy-allow` it stays in the
  host's, where Landlock's TCP port rules and a seccomp filter refuse every
  other socket family, and under `--allow-net` every internet socket that is
  not TCP. No network row above shows an escape from that difference. The
  cost is name resolution: without UDP it goes over TCP 53, which a resolver
  that ignores `RES_OPTIONS` (musl, a static Go binary) does not do.
- **Hosts that refuse unprivileged user namespaces.** Examples are Ubuntu 23.10
  and later with `kernel.apparmor_restrict_unprivileged_userns=1`, and most
  containers. porta then cannot create the PID namespace. The run goes ahead
  in the host's, says so on stderr, and `porta check` shows it. There, the
  default read mode leaves other processes' `/proc` entries readable, and only
  `--read-policy strict` closes them; a writable mount's hooks and protected
  names are not protected; credential sockets stay reachable; and the preset's closed paths are carved out of a
  Landlock read grant, so a closed path inside a mount refuses the run. srt and Fence depend on bubblewrap, which
  needs the same kernel permission (or an AppArmor profile that grants it).
  On Ubuntu, `sudo bash scripts/apparmor-userns.sh "$(command -v porta)"`
  loads the profile Ubuntu documents for porta alone. CI runs both suites
  without it and then with it.
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
