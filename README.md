<p align="center">
  <img src="docs/assets/logo.png" alt="Porta" width="200">
</p>

<h1 align="center">Porta</h1>

<p align="center">
  Run a command — or an AI agent — with only what you granted it,<br>
  enforced by the kernel on macOS and Linux. No container, no daemon.
</p>

<p align="center">
  <a href="README.md"><img alt="English" src="https://img.shields.io/badge/English-24292f?style=flat-square"></a>
  <a href="README.ja.md"><img alt="日本語" src="https://img.shields.io/badge/%E6%97%A5%E6%9C%AC%E8%AA%9E-d0d7de?style=flat-square"></a>
  <a href="README.zh-CN.md"><img alt="简体中文" src="https://img.shields.io/badge/%E7%AE%80%E4%BD%93%E4%B8%AD%E6%96%87-d0d7de?style=flat-square"></a>
</p>

---

```text
$ porta run sh -v ./work --no-net -- -c 'echo ok > out.txt; cat ~/.ssh/id_ed25519; echo x > ~/elsewhere.txt; curl https://example.com'
cat: /Users/me/.ssh/id_ed25519: Operation not permitted
sh: /Users/me/elsewhere.txt: Operation not permitted
curl: (6) Could not resolve host: example.com
[porta] the sandbox refused this run 3 times; what each would have needed:
  file-read-data /Users/me/.ssh/id_ed25519
    → closed by the preset or --deny-read; no mount reopens it
  file-write-create /Users/me/elsewhere.txt
    → -v /Users/me
  …
```

`out.txt` was written; the key, the file outside and the network were not.

Porta wraps one process and everything it starts. Writes go only to the
directories you mount, credential stores stay unreadable, the network is
narrowed to the ports or hosts you name, and CPU, memory, processes and time
are capped. It is enforced by Seatbelt on macOS and by Landlock, seccomp and
namespaces on Linux — not by a wrapper that asks nicely — and a rule the
kernel cannot express refuses the run instead of weakening it.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/almide/porta/main/scripts/install.sh | bash
```

| Where | How |
|---|---|
| macOS (Apple silicon), Linux (x86-64, arm64; glibc 2.31+: Debian 11, Ubuntu 20.04 and later) | the script above: checks the SHA-256, and the Sigstore signature where `cosign` is installed |
| Debian, Ubuntu | `sudo apt install ./porta_<version>_amd64.deb` from a [release](https://github.com/almide/porta/releases); on Ubuntu 23.10+ it also loads the AppArmor profile porta's namespaces need |
| Ubuntu, installed any other way | `sudo porta setup` once, for the same profile |
| GitHub Actions | `- uses: almide/porta@v0.6.16`, then `porta run …` in any step |
| Anything else (Intel Mac…) | `almide install github.com/almide/porta --branch main` builds from source |

Every release is signed and carries build provenance —
[verifying a release](docs/cli.md#verifying-a-release).

## Confine the coding agent you already use

```bash
cd my-project
porta init claude            # or: porta init codex
porta up -- -p "Fix the failing test"
```

`porta init` writes a commented `porta.toml`, measured to what that agent
needs: the project and the agent's own directory writable, its settings,
hooks and global instructions there unwritable — so one session cannot plant
something for the next — and its API key or token passed by name. On macOS
the Keychain is closed, so the login goes in as `ANTHROPIC_API_KEY` or
`CLAUDE_CODE_OAUTH_TOKEN` (`claude setup-token`).

Any other command works the same way, with flags:

```bash
porta run ./installer -v ./sandbox --allow-net '*:443' --timeout 120 --max-procs 500
```

Everything before `--` is porta's; everything after is the command's.

## What it stops

| | macOS | Linux |
|---|---|---|
| **Writes** | only the `-v` mounts, `/tmp`, `/dev` | same |
| **Inside a writable mount** | `.git/hooks`, `.git/config`, `.envrc`, shell rc files, agent settings and the like stay unwritable and cannot be renamed away | same (needs user namespaces — see below) |
| **Reads** | everything but credential stores: `~/.ssh`, `~/.aws`, `~/.config/gh`, cloud and registry tokens, the Keychain, browser profiles; `--read-policy strict` confines reads to the mounts | same, minus the Keychain |
| **Credential sockets** | SSH agent, gpg-agent, `docker.sock` refused unless `--allow-unix` | same |
| **Network** | open, or TCP by port (`--allow-net`), or by host through porta's proxy (`--proxy-allow`), or none (`--no-net`) | same; UDP closed under a port rule |
| **Other processes** | their arguments and environment unreadable; `open(1)` / Launch Services closed | not visible: own PID namespace |
| **Resources** | `--timeout`, `--max-cpu`, `--max-procs`, `--max-file-size`, `--max-memory-mb` | same (memory through cgroup v2) |
| **Environment** | starts empty; `PATH`, `HOME`, locale and terminal cross, the rest only by `-e` / `--env-pass` | same |

What is closed is a preset — [`native/presets/default.toml`](native/presets/default.toml),
shown by `porta explain` — not a list in code: `--deny-read`, `--protect` and
`--deny-unix` add to it, `--preset <file>` replaces it. The full table, with
the reasons, is in [enforcement](docs/enforcement.md).

## When something is refused

```bash
porta explain ./tool -v ./work --allow-net '*:443'   # the policy, in words; nothing runs
porta run ./tool -v ./work --why                      # afterwards: each refusal and the flag it needed
```

```text
[porta] the sandbox refused this run 2 times; what each would have needed:
  file-write-data /home/me/notes/out.txt
    → -v /home/me/notes
  network-outbound remote:*:80
    → --allow-net '*:80'
  to run again with those granted:
    porta run ./tool -v ./work -v /home/me/notes --allow-net '*:80'
```

On macOS this comes from the kernel's denial log after any failed run; on
Linux, `--why` traces the run with `strace` from outside the sandbox.

## Undo what it changed

```bash
porta run ./agent -v ./project --snapshot     # copies the mounts first; lists what changed after
porta rollback --yes                          # puts them back
```

A clone on APFS, so the copy costs next to nothing there. Snapshots live
outside every mount, so the command cannot rewrite its own undo.

## How it compares

The same [escape corpus](docs/benchmarks/escapes.md) — real ways out, scored
on what got through — run under five tools, each with its own defaults
([full comparison](docs/benchmarks/competitors.md), losing rows included):

| | porta | [srt](https://github.com/anthropics/sandbox-runtime) | [Fence](https://github.com/fencesandbox/fence) | [nono](https://github.com/nolabs-ai/nono) | [landrun](https://github.com/Zouuup/landrun) |
|---|---|---|---|---|---|
| macOS: tried / escaped | **22 / 0** | 17 / 5 | 14 / 5 | 13 / 5 | — |
| Linux: tried / escaped | **28 / 0** | 23 / 2 | 21 / 3 | 24 / 5 | 22 / 7 |

What the others do that porta does not: srt and Fence close the network by
default; Fence filters commands by name; nono can ask for more access
mid-run. What a command pays under porta: about 16 ms on macOS, 2–6 ms on
Linux ([overhead](docs/benchmarks/overhead.md)).

## Limits

- **The network is open by default.** Close it with `--no-net`, `--allow-net`
  or `--proxy-allow`.
- **On Linux, some protections need user namespaces** — the hooks inside a
  mount, credential sockets, hiding other processes. Ubuntu 23.10+ restricts
  them until the `.deb` or `sudo porta setup` loads a profile; without one
  porta runs and says what is left open.
- **Same kernel.** This is a process sandbox, not a VM: a kernel exploit is
  out of scope. The [threat model](docs/threat-model.md) lists what is and is
  not defended.
- **Tested by its authors.** The corpus, the fuzzer and the monkey test are
  public and run in CI; there has been no third-party audit.

## Also in the box

- **WASM with no ambient authority.** `porta run plugin.wasm --profile worker`
  runs a core module or a WASI 0.2/0.3 component with no host filesystem or
  network and bounded fuel, memory and time; every import needs a capability
  you granted.
- **Agents whose loop is WASM.** `porta agent agent.toml` runs the decision
  loop and each tool in separate instances, keeps model credentials in the
  host, and resumes a crashed run without repeating completed writes
  ([agent runtime](docs/agent-runtime.md)).
- **One API and a log.** `--proxy-allow api.example.com --proxy-audit
  egress.jsonl` lets through only that host and records every decision.
- **Keys the command cannot read.** `--credential ANTHROPIC_API_KEY=api.anthropic.com`
  hands the command a placeholder; porta's proxy puts the real key on requests
  to that host alone, and refuses the placeholder anywhere else it looks
  ([enforcement](docs/enforcement.md#credentials-as-placeholders)).

## Docs

- [CLI reference](docs/cli.md) — every command, flag, `porta.toml` key and exit code
- [Enforcement](docs/enforcement.md) — exactly what each platform stops, and how
- [Threat model](docs/threat-model.md) — what it defends against, and what it does not
- [Benchmarks](docs/benchmarks/README.md) — the escape corpus, the comparison, overhead
- [Architecture](docs/architecture.md) — Almide decides policy, Rust enforces it

Built with [Almide](https://github.com/almide/almide) and
[Wasmtime](https://wasmtime.dev). Apache-2.0.
