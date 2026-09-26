#!/usr/bin/env bash
set -euo pipefail
# Packages one porta binary as a .deb for Debian and Ubuntu.
#
#   bash scripts/build-deb.sh target/porta 0.6.15 amd64 dist
#
# The package puts porta at /usr/bin/porta, and on a host that restricts
# unprivileged user namespaces through AppArmor (Ubuntu 23.10 and later) its
# postinst writes and loads the profile Ubuntu documents for a program that
# needs them — for /usr/bin/porta alone, granting `userns` and nothing else —
# so the command gets its own PID, mount and network namespace without a
# manual step. On a host without that restriction, or with an AppArmor too old
# for the rule (Ubuntu 22.04), it writes nothing: a profile the parser cannot
# read would fail at the next boot. Removing the package unloads and deletes
# the profile.

binary=${1:?usage: build-deb.sh <porta binary> <version> <amd64|arm64> <out dir>}
version=${2:?version}
arch=${3:?architecture}
out=${4:?out dir}

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
chmod 0755 "$stage"
install -D -m 0755 "$binary" "$stage/usr/bin/porta"
install -D -m 0644 LICENSE "$stage/usr/share/doc/porta/copyright"
mkdir -p "$stage/DEBIAN"

cat > "$stage/DEBIAN/control" <<EOF
Package: porta
Version: $version
Architecture: $arch
Maintainer: almide <https://github.com/almide/porta>
Section: utils
Priority: optional
Recommends: strace
Homepage: https://github.com/almide/porta
Description: run a command or an AI agent with only what you grant it
 porta confines a native command or a WASM module to the directories, hosts
 and ports it is granted, with Landlock and seccomp on Linux. On Ubuntu this
 package also loads the AppArmor profile that lets porta give each command its
 own PID, mount and network namespace.
EOF

# The profile, and the rule that decides whether this host needs it.
cat > "$stage/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e
profile=/etc/apparmor.d/usr.bin.porta
restricted=$(cat /proc/sys/kernel/apparmor_restrict_unprivileged_userns 2>/dev/null || echo 0)
if [ "$1" = configure ] && [ "$restricted" = 1 ] && command -v apparmor_parser >/dev/null \
   && [ -e /etc/apparmor.d/abi/4.0 ]; then
  cat > "$profile" <<'PROFILE'
# Written by the porta package: lets /usr/bin/porta create user namespaces,
# and nothing else; the program is otherwise unconfined, as it was before.
abi <abi/4.0>,
include <tunables/global>

profile usr.bin.porta /usr/bin/porta flags=(unconfined) {
  userns,

  include if exists <local/usr.bin.porta>
}
PROFILE
  apparmor_parser -r "$profile"
  echo "porta: loaded $profile; /usr/bin/porta may create user namespaces"
fi
exit 0
EOF

cat > "$stage/DEBIAN/postrm" <<'EOF'
#!/bin/sh
set -e
profile=/etc/apparmor.d/usr.bin.porta
if [ "$1" = remove ] || [ "$1" = purge ]; then
  if [ -e "$profile" ]; then
    command -v apparmor_parser >/dev/null && apparmor_parser -R "$profile" 2>/dev/null || true
    rm -f "$profile"
  fi
fi
exit 0
EOF
chmod 0755 "$stage/DEBIAN/postinst" "$stage/DEBIAN/postrm"

mkdir -p "$out"
deb="$out/porta_${version}_${arch}.deb"
dpkg-deb --root-owner-group --build "$stage" "$deb" >/dev/null
echo "$deb"
