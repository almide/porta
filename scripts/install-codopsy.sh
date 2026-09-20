#!/usr/bin/env bash
set -euo pipefail
# The static analyzer CI grades this repository with. The score depends on the
# analyzer's own thresholds and its Almide grammar coverage, so the version is
# pinned: an unpinned upgrade would move the number without anything in this
# repository changing.
# Upstream publishes no checksum file, so the digest of each pinned artifact is
# recorded here and reviewed with the rest of the repository.
tag="${CODOPSY_RELEASE_TAG:-v2.2.0}"
case "$(uname -s)" in Darwin) os=apple-darwin ;; Linux) os=unknown-linux-gnu ;; *) echo "unsupported OS" >&2; exit 1 ;; esac
case "$(uname -m)" in arm64|aarch64) arch=aarch64 ;; x86_64) arch=x86_64 ;; *) echo "unsupported architecture" >&2; exit 1 ;; esac
asset="codopsy-${arch}-${os}.tar.gz"
case "${tag}/${arch}-${os}" in
  v2.2.0/aarch64-apple-darwin)     digest=61ebc751b8def53b6be0f905efad65dcf140452d923fd90b6d932969195a555e ;;
  v2.2.0/x86_64-apple-darwin)      digest=d5067ea508857ce7449dcb6fa8ebf2d95618d4a57275be5941b5c417ad423ee5 ;;
  v2.2.0/x86_64-unknown-linux-gnu) digest=4495b5aa3073a2d1b85bf25755d322c619508e510c75d007d3bf8d324e4792ef ;;
  *) echo "no recorded digest for ${asset}; add one before pinning this release" >&2; exit 1 ;;
esac
install_dir="${1:-.tools/codopsy}"
mkdir -p "$install_dir"
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
curl -fsSL "https://github.com/O6lvl4/codopsy/releases/download/$tag/$asset" -o "$scratch/$asset"
python3 - "$scratch/$asset" "$digest" <<'PY'
import hashlib, pathlib, sys
path, expected = pathlib.Path(sys.argv[1]), sys.argv[2]
if hashlib.sha256(path.read_bytes()).hexdigest() != expected:
    raise SystemExit('codopsy checksum mismatch')
PY
tar -xzf "$scratch/$asset" -C "$scratch"
cp "$scratch/codopsy" "$install_dir/codopsy"
chmod +x "$install_dir/codopsy"
"$install_dir/codopsy" --version
