<!-- description: uses: almide/porta — a checked install that also gives porta its namespaces on Ubuntu runners -->
<!-- done: 2026-09-24 -->
# A GitHub Action

## Why

The hosted Ubuntu runners restrict unprivileged user namespaces through
AppArmor, as Ubuntu does from 23.10. There porta ran without its own PID,
mount and network namespace, and so without the protections that need one:
other processes' `/proc` entries, a writable mount's hooks, the credential
sockets. `scripts/apparmor-userns.sh` fixed it, for whoever knew to run it.

## What

`action.yml` at the repository root, a composite action:

- installs the release named by `version`, or the tag the action was used
  at, through `scripts/install.sh` with cosign on the PATH and
  `PORTA_REQUIRE_SIGNATURE=1`, and puts it on the PATH;
- on Linux, when `porta check` shows no namespaces and AppArmor is there,
  loads the profile for that binary alone (`namespaces: false` skips it);
- with `why: true`, installs strace for `porta run --why`.

## Evidence

CI runs the action as a user would, on Ubuntu and macOS: a step confined to
the checkout writes inside it and is refused outside it, and on Ubuntu
`porta check` shows the namespaces.
