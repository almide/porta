<!-- description: A .deb per release whose postinst gives /usr/bin/porta its namespaces on Ubuntu -->
<!-- done: 2026-09-26 -->
# A .deb that brings the AppArmor profile

## Why

On Ubuntu 23.10 and later porta runs without its own namespaces until an
AppArmor profile grants it `userns`, and with the tarball that was a manual
`sudo bash scripts/apparmor-userns.sh`. The way Ubuntu ships programs that
need them — bubblewrap among them — is a package that brings the profile.

## What

- `scripts/build-deb.sh` packages a binary as `porta_<version>_<arch>.deb`:
  `/usr/bin/porta`, a postinst and a postrm.
- The postinst writes and loads `/etc/apparmor.d/usr.bin.porta` (`userns`
  for `/usr/bin/porta`, nothing else) only where the host restricts
  unprivileged user namespaces and its AppArmor understands the rule; on
  Ubuntu 22.04, or a host without the restriction, it writes nothing, since a
  profile the parser cannot read would fail at the next boot.
- The postrm unloads and deletes the profile.
- The release builds one for amd64 and arm64, lists them in the signed
  checksums and attests their provenance.

## Evidence

- CI builds the package from each commit on the Ubuntu runner, installs it,
  sees `porta check` report the namespaces, removes it and sees the profile
  gone.
- The release's verify job does the same with the published `.deb`, after
  checking it against the signed checksum list.
