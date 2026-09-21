#!/usr/bin/env bash
# Check that a published release is one somebody can actually install.
#
#   bash scripts/verify_release.sh v0.5.0
#
# Publishing is not the same as being installable: the archive has to exist for
# this host, its checksum has to be listed and match, the binary inside has to
# run, and it has to report the version the tag claims. This checks all four
# against the real release, from outside the repository.
set -euo pipefail

tag="${1:?usage: verify_release.sh <tag>}"
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT

# Listed over the same HTTP a user's install would use, not through gh: this
# has to work for somebody who has the script and nothing else — no checkout,
# no gh, no token. It failed exactly that way the first time it ran outside a
# repository.
echo "== assets published under $tag"
curl -fsSL "https://api.github.com/repos/almide/porta/releases/tags/$tag" \
  | python3 -c 'import json,sys
assets = json.load(sys.stdin).get("assets", [])
if not assets:
    raise SystemExit("no assets published under this tag")
for a in assets:
    print("   " + a["name"] + "  " + str(a["size"]) + " bytes")' 

echo "== installing from the published release, as a user would"
PORTA_RELEASE_TAG="$tag" bash "$(dirname "$0")/install.sh" "$scratch/bin"

echo "== the installed binary runs and agrees with the tag"
reported=$("$scratch/bin/porta" --version | awk '{print $NF}')
echo "   porta --version -> $reported"
[ "v$reported" = "$tag" ] || {
  echo "   the binary says $reported but the tag says $tag" >&2
  exit 1
}

echo "== it enforces, and still does the job"
work="$scratch/work"
mkdir -p "$work"

# The target for the write that must fail cannot be under /tmp or /dev: porta
# makes those writable on every run by design, so a write landing there proves
# nothing about the binary. `mktemp -d` returns /tmp/... on Linux, which is how
# this check passed on macOS — where it returns /var/folders/... — and failed a
# working Linux build. A home directory is granted to nothing unless mounted.
ungranted=$(mktemp -d "${HOME:-/nonexistent}/porta-verify-XXXXXX") || {
  echo "   cannot create a directory outside the always-writable roots" >&2
  exit 1
}
trap 'rm -rf "$scratch" "$ungranted"' EXIT

# Both halves, because either alone passes a binary that is useless. A refusal
# on its own is what a porta that cannot run anything looks like; a successful
# write on its own is what a porta that enforces nothing looks like.
"$scratch/bin/porta" run /bin/sh -v "$work" -- -c 'echo x > "$1"' sh "$ungranted/escape" 2>/dev/null || true
if [ -e "$ungranted/escape" ]; then
  echo "   a write outside every mount landed; this binary enforces nothing" >&2
  exit 1
fi
echo "   a write outside every mount was refused"

inside=$("$scratch/bin/porta" run /bin/sh -v "$work" -- -c 'echo x > "$1"' sh "$work/inside" 2>&1 || true)
if [ ! -e "$work/inside" ]; then
  # Never discard the reason. A refusal here is porta working as designed on a
  # host whose kernel cannot restrict anything — an emulated container, an old
  # kernel — and that reads exactly like a broken binary if the message is
  # thrown away.
  echo "   a write inside the granted mount did not land: ${inside:-no output}" >&2
  case "$inside" in
    *"no usable Landlock support"*)
      echo "   that is porta refusing a host it cannot restrict, not a bad build." >&2
      echo "   run this on a host with Landlock to check the binary itself." >&2
      # Exit 2 tells a human "wrong host, try elsewhere". In CI there is no
      # elsewhere: the runners are the platforms this release claims, so a
      # release that cannot be shown to enforce on one of them is not a
      # release. ${CI:-} is set by every runner.
      [ -n "${CI:-}" ] && exit 1
      exit 2 ;;
  esac
  exit 1
fi
echo "   a write inside the granted mount worked"

echo
echo "$tag is installable and enforcing on $(uname -s)/$(uname -m)."
