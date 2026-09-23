<!-- description: Wall time a command pays under porta and under four other sandboxes -->
# What the sandbox costs

`scripts/overhead.py` times three commands, each run 30 times after three
warm-up runs, first with no sandbox and then under each porta mode and each
other tool. The runs alternate, so a slow moment on the host lands on every
variant alike.

The commands are `true` (nothing but the sandbox's own start),
`python3 -c pass` (an interpreter starting) and `git status` in a 50-file
repository. Each cell is the median wall time in milliseconds, with the
median minus bare in brackets.

The raw results, with the 90th percentile, are in
[`2026-09-24-overhead/`](2026-09-24-overhead/).

## Measured 2026-09-24

The runs used porta 0.6.6 and the same competitor versions as in
[the comparison](competitors.md).

### macOS 26.3, Apple M-series

| | true | python3 -c pass | git status |
|---|---|---|---|
| no sandbox | 2.9 | 15.4 | 9.3 |
| porta, default | 18.9 (+16.0) | 31.3 (+15.9) | 25.6 (+16.3) |
| porta, strict reads | 18.3 (+15.4) | 31.5 (+16.1) | — |
| porta, `--no-net` | 18.4 (+15.5) | 31.0 (+15.6) | 25.1 (+15.9) |
| porta, `--allow-net '*:443'` | 18.5 (+15.6) | 31.7 (+16.3) | 25.1 (+15.8) |
| porta, `--proxy-allow` | 18.5 (+15.6) | 31.4 (+16.0) | 25.6 (+16.3) |
| srt | 144.2 (+141.3) | 147.8 (+132.4) | 139.1 (+129.8) |
| Fence | 42.7 (+39.7) | 53.0 (+37.6) | 47.5 (+38.2) |
| nono | 116.2 (+113.2) | 133.2 (+117.8) | — |

### Linux, with namespaces

The host was a Docker container on the same Mac: aarch64, kernel 6.12
(linuxkit), with unprivileged user namespaces allowed, so porta ran the
command in its own PID and mount namespace.

| | true | python3 -c pass | git status |
|---|---|---|---|
| no sandbox | 0.5 | 7.8 | 0.9 |
| porta, default | 5.1 (+4.6) | 12.8 (+5.0) | 4.7 (+3.9) |
| porta, strict reads | 5.0 (+4.5) | 13.0 (+5.2) | — |
| porta, `--no-net` | 5.6 (+5.1) | 13.9 (+6.2) | 4.9 (+4.0) |
| porta, `--allow-net '*:443'` | 4.6 (+4.1) | 13.9 (+6.2) | 4.5 (+3.6) |
| porta, `--proxy-allow` | 5.0 (+4.5) | 13.8 (+6.0) | 4.7 (+3.8) |
| srt | 245.8 (+245.4) | 249.5 (+241.7) | 185.3 (+184.5) |
| Fence | 145.5 (+145.0) | 148.9 (+141.2) | 136.2 (+135.3) |
| nono | 33.3 (+32.8) | 43.6 (+35.8) | — |
| landrun | 2.6 (+2.1) | 10.1 (+2.4) | 2.5 (+1.6) |

### Linux, without namespaces

This is the same container with `user.max_user_namespaces=0`, the situation
of a host that refuses them.

| | true | python3 -c pass | git status |
|---|---|---|---|
| no sandbox | 0.4 | 7.2 | 1.0 |
| porta, default | 2.7 (+2.4) | 9.9 (+2.7) | 3.6 (+2.6) |
| porta, strict reads | 2.7 (+2.3) | 9.9 (+2.8) | — |
| porta, `--no-net` | 2.4 (+2.0) | 10.0 (+2.8) | 3.5 (+2.5) |
| porta, `--allow-net '*:443'` | 2.3 (+1.9) | 9.6 (+2.5) | 3.6 (+2.6) |
| porta, `--proxy-allow` | 2.7 (+2.3) | 9.9 (+2.7) | 4.0 (+3.0) |
| srt | — | — | — |
| Fence | — | — | — |
| nono | 28.1 (+27.8) | 39.9 (+32.8) | — |
| landrun | 2.2 (+1.8) | 9.4 (+2.2) | 2.9 (+1.9) |

## Reading it

- **porta costs about 16 ms a command on macOS and 2 to 6 ms on Linux.**
  Neither number moves with the command, only with the platform, and the
  mode hardly matters. On macOS most of the cost is `sandbox-exec` compiling
  the profile. On Linux the PID, mount and network namespaces add about
  2.5 ms over Landlock and seccomp alone.
- **macOS numbers move with the machine's load.** A shorter run on the same
  Mac an hour earlier, while it was busy, measured porta at +27 to +36 ms.
  The table is from a quiet one. The ranking of the tools did not change.
- **landrun is the lightest on Linux.** It is a thin Landlock wrapper with no
  seccomp filter, no namespaces and no supervisor. It pays for that in the
  corpus rows it cannot hold (UDP, user namespaces; see
  [the comparison](competitors.md)).
- **srt and Fence start bubblewrap and a proxy for every command**: 130 to
  250 ms. In a host that refuses user namespaces they do not run at all
  (`bwrap: Creating new namespace failed`), where porta runs and says what
  it could not do. On stock Ubuntu, bubblewrap has an AppArmor profile of
  its own that grants them, so this is a container's situation, not a
  desktop's.
- **—** is a combination that did not run, and why:
  - git under porta's strict reads: git reads the user's configuration under
    the home, and granting the home would not measure strict reads.
  - git under nono: nono also confines reads by default, and git failed with
    "unable to access .config/git/config".
  - srt and Fence without namespaces: see above.

## Run it yourself

```bash
python3 scripts/overhead.py "$(command -v porta)" \
  --tool srt="$(command -v srt)" --tool fence="$(command -v fence)" \
  --tool nono="$(command -v nono)" --tool landrun="$(command -v landrun)"
```

CI runs it for porta alone on every push, on both runners, and prints the
table in the job log.
