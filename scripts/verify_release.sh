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

echo "== assets published under $tag"
gh release view "$tag" --json assets -q '.assets[].name' | sed 's/^/   /'

echo "== installing from the published release, as a user would"
PORTA_RELEASE_TAG="$tag" bash "$(dirname "$0")/install.sh" "$scratch/bin"

echo "== the installed binary runs and agrees with the tag"
reported=$("$scratch/bin/porta" --version | awk '{print $NF}')
echo "   porta --version -> $reported"
[ "v$reported" = "$tag" ] || {
  echo "   the binary says $reported but the tag says $tag" >&2
  exit 1
}

echo "== it enforces something, rather than merely starting"
work="$scratch/work"
mkdir -p "$work"
if "$scratch/bin/porta" run /bin/sh -v "$work" -- -c 'echo x > "$1"' sh "$scratch/escape" 2>/dev/null; then
  echo "   a write outside every mount succeeded; this binary enforces nothing" >&2
  exit 1
fi
echo "   a write outside every mount was refused"

echo
echo "$tag is installable and enforcing on $(uname -s)/$(uname -m)."
