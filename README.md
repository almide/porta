<p align="center">
  <img src="docs/assets/logo.png" alt="Porta" width="200">
</p>

<h1 align="center">Porta</h1>

<p align="center">
  Run an agent with the permissions you actually granted it.<br>
  OS-enforced limits for the CLI agents you already use, and a WASM runtime for the ones you build.
</p>

<p align="center">
  <a href="https://github.com/almide/almide">Almide</a> + <a href="https://wasmtime.dev">Wasmtime</a> · No Docker required
</p>

<p align="center">
  English · <a href="README.ja.md">日本語</a>
</p>

---

## What Porta is

Letting an AI agent run commands on your machine puts your keys, tokens and
network within its reach. Porta runs the command and lets the OS kernel stop
it — writes, reads, network and sockets enforced by Seatbelt on macOS or
Landlock + seccomp on Linux, not by a wrapper or a prompt. A restriction the
kernel cannot express refuses the run rather than weakening it (**fail-closed**).

Shown, not claimed: a published jailbreak
[corpus](docs/benchmarks/escapes.md) holds 22/22 on macOS and 28/28 on Linux (24/24 on hosts that refuse
unprivileged user namespaces),
zero escapes, losing rows kept in ([threat model](docs/threat-model.md)).
The same corpus run under srt, Fence, nono and landrun is
[published beside it](docs/benchmarks/competitors.md), including where porta loses,
and so is [what each costs a command](docs/benchmarks/overhead.md): about
16 ms under porta on macOS and 2 to 6 ms on Linux.

```text
$ porta run sh -v ./work --read-policy strict --timeout 5 -- …
  wrote ./work/out.txt              legitimate work goes through
  read the SSH key       → refused
  write outside ./work   → refused
  # a hung command is killed at the deadline → exit 124
```

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/almide/porta/main/scripts/install.sh | bash
```

A single binary for macOS on Apple silicon and Linux on x86-64 and arm64,
checked against its published SHA-256 before it is installed, and against
the release's Sigstore signature where `cosign` is installed
([verifying a release](docs/cli.md#verifying-a-release)). Or, with
[Almide](https://github.com/almide/almide), the way `go install` does it
— the route for an Intel Mac or any other target:

```bash
almide install github.com/almide/porta --branch main
```

`main` is the latest release; `--tag v0.6.6` pins one. It builds from source
into `~/.local/bin` and takes a few minutes (the Rust side compiles
Wasmtime). Building by hand is in
[the CLI reference](docs/cli.md#build-from-source). `porta
run` takes either a native command or a `.wasm` module, so anything that
compiles to WASI runs under it.

## Use it for

Everything before `--` is porta's; everything after belongs to the command.

### Restrict an AI agent you already run

The agent installs deps, runs tests, edits files — on the machine that holds
your keys. Give it one directory and one host, and close the rest.

```bash
porta run claude --allow-net 'api.anthropic.com:443' -v ./project -e "HOME=$HOME" \
  -- --print "Fix the bug in main.rs"
```

`claude` runs unchanged, but it can write only inside `./project`, reach only
the host you listed, and it cannot read `~/.ssh`, `~/.aws` or the Keychain. No
Docker daemon, no image, no change to the agent.

### Give a tool one API, and log every attempt

A tool needs one endpoint and nothing else. Route egress through porta's proxy
and keep the record.

```bash
porta run ./agent --proxy-allow 'api.example.com' --proxy-audit egress.jsonl -v ./work
```

Only `api.example.com` is reachable; direct TCP, UDP and Unix-socket egress are
denied, a name resolving to a cloud-metadata or link-local address is refused,
and every decision lands in `egress.jsonl`.

### Try an unknown command without handing over the machine

Preview the policy before anything runs, then run it boxed in, with a deadline
and resource ceilings the kernel enforces on everything it starts.

```bash
porta explain ./sketchy-installer -v ./sandbox --allow-net github.com:443   # see the policy, run nothing
porta run     ./sketchy-installer -v ./sandbox --allow-net github.com:443 \
  --timeout 60 --max-cpu 30 --max-procs 500 --max-file-size 200
```

A hang is killed at the deadline, a CPU burn ends with SIGXCPU, a fork bomb
cannot fork, and a file stops growing at the ceiling. `--max-memory-mb 512`
bounds resident memory for the whole run: a cgroup v2 ceiling on Linux (a
systemd user session is needed, and the flag is refused without one), a
supervisor that ends the run at the ceiling on macOS.

### Run untrusted or generated WASM

Code from a user or a model, run with no host filesystem or network and bounded
fuel, memory and time. A core module or a WASI 0.2 or 0.3 component, checked
the same way: every import it declares needs a capability you granted.

```bash
porta run plugin.wasm --profile worker --step-limit 5000000 --max-memory 256
```

### Build an agent whose decision loop is WASM

`porta run` restricts an agent someone else wrote. `porta agent` runs one whose
loop is itself WASM: the loop and every tool run in separate instances that
inherit no environment or directory, model credentials stay in the host, and a
crashed run resumes without repeating completed writes.

```bash
porta agent agent.toml --record run.jsonl -- "Add 20 and 22 using the tool."
porta agent-resume agent.toml run.jsonl    # completed writes are not repeated
porta agent-journal run.jsonl              # read-only metadata, no code loaded
```

See [agents you build](docs/agent-runtime.md), [completion
checks](docs/completion-checks.md) the guest cannot bypass, and [artifact
pins](docs/artifact-pins.md) that bind WASM to a reviewed SHA-256.

### Keep the settings as project config

```bash
porta init native claude                 # writes porta.toml
porta up -- --print "Fix the bug in main.rs"
```

Or let a working invocation write its own: `porta explain claude … --save
porta.toml`.

## See a restriction actually stop something

A working restriction is invisible, so watch one fail.

```bash
porta run curl -- https://example.com                     # network open by default
porta run curl --allow-net '*:443' -- https://example.com # HTTPS allowed → works
porta run curl --allow-net '*:80'  -- https://example.com # → exit 7, port 443 denied
```

A refused run does not leave you guessing: after a non-zero exit, porta reads
the kernel's denial records and prints which flag each refusal would have
needed (on macOS after every failed run; on Linux with `--why`, which traces
the run with `strace`). Exit codes tell a script
apart a command that failed from one that never ran — see
[the CLI reference](docs/cli.md#when-a-run-is-refused).

## Evidence

Every published number keeps its raw report, source hashes and an audit that
regrades it; CI runs those audits, and results that do not favour Porta are
published in the same place.

- [Escape corpus](docs/benchmarks/escapes.md) — known jailbreaks run against the
  binary, scored on what escaped, losing rows included.
- [Startup and memory](docs/benchmarks/startup-and-memory.md) — fixed-response
  measurements with explicit comparison limits.
- [Containment and recovery](docs/benchmarks/containment-evaluation.md) — under
  hostile inputs; one of five scenarios separated the runtimes, and containment
  cost task completion.
- [Real-task quality](docs/benchmarks/README.md) — Porta has not demonstrated
  general quality superiority, and the shared-compute follow-up was rejected
  rather than published as a win.

## Docs

- [CLI reference](docs/cli.md) — every command, flag, `porta.toml` key and exit code.
- [Enforcement](docs/enforcement.md) — exactly what macOS and Linux each stop.
- [Threat model](docs/threat-model.md) — what it defends against, and what it does not.
- [Architecture](docs/architecture.md) — Almide decides policy, Rust enforces it.

## License

Apache-2.0
