<!-- description: Linux closes credential sockets and, under --allow-net, UDP -->
<!-- done: 2026-09-24 -->
# Credential sockets and UDP on Linux

## Why

Two gaps were left on Linux after the preset moved into data.

- The preset's `[unix] deny` (the SSH agent, gpg-agent, container runtimes)
  was enforced on macOS only. On Linux a command could connect to
  `/tmp/ssh-XXXX/agent.<pid>` and sign with the caller's keys, or to
  `docker.sock` and be root on the host. `--allow-unix` meant nothing there.
- Under `--allow-net`, Landlock's port rule reaches TCP only, so UDP stayed
  open to every host. The threat model listed it as a known gap.

## What

- `unix_sockets.rs` reads the pathname sockets bound on the host now from
  `/proc/net/unix`, keeps those the preset's patterns match and `--allow-unix`
  does not name, and the command's mount namespace covers each with
  `/dev/null`: a `connect` there is refused. `porta explain` lists them.
  Without a namespace the run names the sockets that stay reachable.
- Under `--allow-net`, seccomp lets an `AF_INET`/`AF_INET6` socket be a TCP
  stream only, which closes UDP, SCTP and ICMP echo. Names then resolve over
  TCP: porta sets `RES_OPTIONS=use-vc` and Landlock lets TCP reach port 53.
- A `--deny-unix` pattern must be a regular expression; one that is not
  refuses the run on both platforms.

## Evidence

- Escape corpus: "reach the SSH agent's socket" and "get a UDP answer from
  outside under a TCP port rule", on both platforms.
- Integration: an internet socket is TCP only under `--allow-net` and
  `RES_OPTIONS` is set; a socket at an agent's path is refused unless
  `--allow-unix` names it, or the run says it stays reachable.
- Unit: the TCP-ports seccomp program, the socket matcher.
