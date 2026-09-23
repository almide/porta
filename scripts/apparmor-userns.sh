#!/usr/bin/env bash
set -euo pipefail
# Lets one porta binary create user namespaces on a host that restricts them.
#
# Ubuntu from 23.10 sets kernel.apparmor_restrict_unprivileged_userns=1: an
# unprivileged process may still create a user namespace, but gets no rights
# in it, so porta cannot give a command its own PID, mount and network
# namespace there (`porta check` says so). The remedy Ubuntu documents is an
# AppArmor profile for the one program that needs them, the way bubblewrap
# gets its own. This writes that profile for the binary named and loads it.
# It grants `userns` and nothing else; the profile is otherwise unconfined, as
# the program was before.
#
#   sudo bash scripts/apparmor-userns.sh "$(command -v porta)"

binary=$(readlink -f "${1:?usage: apparmor-userns.sh /path/to/porta}")
[ -x "$binary" ] || { echo "not an executable: $binary" >&2; exit 2; }
case "$binary" in *'"'*|*$'\n'*) echo "refusing a path with a quote or newline in it: $binary" >&2; exit 2 ;; esac
command -v apparmor_parser >/dev/null || { echo "apparmor_parser is not installed; this host does not restrict user namespaces through AppArmor" >&2; exit 2; }

profile=${APPARMOR_DIR:-/etc/apparmor.d}/porta
{
  printf '# Written by porta'"'"'s scripts/apparmor-userns.sh for %s.\n' "$binary"
  printf 'abi <abi/4.0>,\ninclude <tunables/global>\n\n'
  printf 'profile porta "%s" flags=(unconfined) {\n  userns,\n\n  include if exists <local/porta>\n}\n' "$binary"
} > "$profile"
apparmor_parser -r "$profile"
echo "loaded $profile: $binary may create user namespaces"
