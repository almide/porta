<!-- description: sudo porta setup gives any porta its namespaces on Ubuntu, safely -->
<!-- done: 2026-09-27 -->
# porta setup

## Why

The `.deb` gives `/usr/bin/porta` its user namespaces on Ubuntu 23.10 and
later. A porta installed by `install.sh` or `almide install` sits under the
home, and `scripts/apparmor-userns.sh` wrote the profile for whatever path it
was given — including one the user can write. A profile for such a path
grants `userns` to anything the user, or malware running as them, puts
there: the restriction undone for that user.

## What

- `sudo porta setup` copies the running porta to `/usr/local/bin/porta` where
  that directory and every one above it are root's alone, and otherwise to
  `/usr/libexec/porta/porta` with a link from `/usr/local/bin` (the GitHub runner
  image leaves `/usr/local/bin` writable by its user, and setup refused it
  there). It writes the profile for that copy alone and loads it,
  then runs `porta check` as the user who ran sudo to confirm the
  namespaces. It says what it will do first; `--dry-run` stops there,
  `--undo` takes both away. On a host without the restriction it does
  nothing.
- `scripts/apparmor-userns.sh` refuses a binary that it, or any directory
  above it, is not root's alone, and points at `porta setup`.
- The runtime notice on such a host names `sudo porta setup`.
- The action installs a root-owned copy the same way before loading the
  profile.

## Evidence

CI refuses the helper script on the checkout's build, runs `porta setup` on
the Ubuntu runner, sees the namespaces as the runner's user, runs the
integration suite and the escape corpus against the copy, and undoes it.
