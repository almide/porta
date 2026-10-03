#!/usr/bin/env bash
# Builds the Linux porta that ships: inside Debian bullseye, so the binary asks for
# nothing newer than glibc 2.31 and runs on Debian 11 and later, Ubuntu 20.04 and later,
# and the python:*-slim images (#35). A binary built on the runner itself (Ubuntu 24.04)
# needed glibc 2.39 and did not start on bookworm.
#
#   scripts/build-linux.sh [OUT]      (default: target/porta)
#
# The pinned Almide is built from its tag in the same container, because its own Linux
# release needs glibc 2.39 too. The result is checked before it is handed back: no glibc
# symbol past 2.31, and no libssl or libcrypto (porta speaks TLS through rustls only).
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
out="${1:-target/porta}"
tag="${ALMIDE_RELEASE_TAG:-$(sed -n 's/^tag="\${ALMIDE_RELEASE_TAG:-\(.*\)}"$/\1/p' scripts/install-almide.sh)}"
[ -n "$tag" ] || { echo "no pinned Almide tag in scripts/install-almide.sh" >&2; exit 2; }
image="${PORTA_LINUX_BUILD_IMAGE:-rust:1-bullseye}"
mkdir -p "$(dirname "$out")" .tools/linux-build

# As the caller's user, so what lands in target/ can be stripped and packaged after.
docker run --rm --user "$(id -u):$(id -g)" -e HOME=/tmp -v "$PWD:/src" -w /src \
  -e ALMIDE_TAG="$tag" -e OUT="$out" -e CARGO_HOME=/src/.tools/linux-build/cargo \
  ${CARGO_BUILD_JOBS:+-e CARGO_BUILD_JOBS="$CARGO_BUILD_JOBS"} "$image" bash -euo pipefail -c '
    almide=/src/.tools/linux-build/almide-$ALMIDE_TAG
    if [ ! -x "$almide" ]; then
      git init -q /tmp/almide && cd /tmp/almide
      git remote add origin https://github.com/almide/almide
      git fetch -q --depth 1 origin "refs/tags/$ALMIDE_TAG:refs/tags/$ALMIDE_TAG"
      git checkout -q "$ALMIDE_TAG"
      cargo build --release --locked --bin almide > /tmp/almide-build.log 2>&1 || { tail -30 /tmp/almide-build.log; exit 1; }
      install -m 0755 target/release/almide "$almide"
      cd /src
    fi
    "$almide" --version
    "$almide" build src/main.almd -o "$OUT"
    newest=$(objdump -T "$OUT" | grep -o "GLIBC_[0-9.]*" | sort -uV | tail -1)
    echo "needs $newest"
    [ "$(printf "%s\nGLIBC_2.31\n" "$newest" | sort -V | tail -1)" = GLIBC_2.31 ] || { echo "needs $newest, past bullseye" >&2; exit 1; }
    if ldd "$OUT" | grep -E "libssl|libcrypto"; then echo "links OpenSSL; porta is rustls only" >&2; exit 1; fi
  '
echo "== built $out"
