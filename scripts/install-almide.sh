#!/usr/bin/env bash
set -euo pipefail
# Exact published tag; override to v0.63.0 once the final release is available.
tag="${ALMIDE_RELEASE_TAG:-v0.63.0-rc1}"
case "$(uname -s)" in Darwin) os=macos ;; Linux) os=linux ;; *) exit 1 ;; esac
case "$(uname -m)" in arm64|aarch64) arch=aarch64 ;; x86_64) arch=x86_64 ;; *) exit 1 ;; esac
asset="almide-${os}-${arch}.tar.gz"
install_dir="${1:-.tools/almide}"
mkdir -p "$install_dir"
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
base="https://github.com/almide/almide/releases/download/$tag"
curl -fsSL "$base/$asset" -o "$scratch/$asset"
curl -fsSL "$base/almide-checksums.sha256" -o "$scratch/checksums"
python3 - "$scratch" "$asset" <<'PY'
import hashlib, pathlib, sys
root, asset = pathlib.Path(sys.argv[1]), sys.argv[2]
checksums = dict((name.lstrip('*'), checksum) for checksum, name in
                 (line.split() for line in (root / 'checksums').read_text().splitlines()))
actual = hashlib.sha256((root / asset).read_bytes()).hexdigest()
if actual != checksums[asset]:
    raise SystemExit('Almide checksum mismatch')
PY
tar -xzf "$scratch/$asset" -C "$scratch"
cp "$scratch/almide-${os}-${arch}/almide" "$install_dir/almide"
"$install_dir/almide" --version
