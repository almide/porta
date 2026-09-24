<!-- description: porta init claude / codex: a porta.toml measured to what each agent needs -->
<!-- done: 2026-09-24 -->
# Recipes for the coding agents people run

## Why

The README's example for confining Claude Code did not work: Claude Code
writes its sessions, history and plugin cache under `~/.claude`, and on macOS
keeps its login in the Keychain, which porta closes. Working out the grants
an agent needs was left to each user, one refusal at a time.

## What

- `porta init claude` and `porta init codex` write a commented `porta.toml`
  (`native/recipes/*.toml`, built into the binary): the project and the
  agent's own directory writable, its settings, commands and global
  instructions there protected, its API key or token passed by name.
- Each was measured under `--why` on macOS: Claude Code starts and stops at
  the login it needs a token for; Codex reaches its API.
- `--protect` takes one path as well as a name (`~/.codex/config.toml`), so a
  file is protected where it is without every project's `config.toml` with
  it.
- `-v ~/x` and `mounts = ["~/x"]` resolve against the home; the `porta init`
  template had always suggested `~/.config`, which failed.
- On macOS the child gets `SSL_CERT_FILE=/etc/ssl/cert.pem`: with the
  Keychain closed, a tool that lists its roots through Security.framework
  (Codex, via rustls-native-certs) found none and failed every TLS handshake.
