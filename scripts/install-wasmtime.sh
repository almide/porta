#!/usr/bin/env bash
set -euo pipefail
# Almide runs each test file through wasmtime when it is on PATH and otherwise
# falls back to a native build. The fallback rebuilds every test binary, which
# costs about an hour of CI per job on a cold cache.
# Upstream publishes no checksum file, so the digest of each pinned artifact is
# recorded here and reviewed with the rest of the repository.
tag="${WASMTIME_RELEASE_TAG:-v47.0.2}"
case "$(uname -s)" in Darwin) os=macos ;; Linux) os=linux ;; *) echo "unsupported OS" >&2; exit 1 ;; esac
case "$(uname -m)" in arm64|aarch64) arch=aarch64 ;; x86_64) arch=x86_64 ;; *) echo "unsupported architecture" >&2; exit 1 ;; esac
asset="wasmtime-${tag}-${arch}-${os}.tar.xz"
case "${tag}/${arch}-${os}" in
  v47.0.2/x86_64-linux)  digest=9ec85751649139711b6a5061c4f48a41412bf9b1ab98a08b9924ca73f22ca575 ;;
  v47.0.2/aarch64-linux) digest=5bb3fe06876a1c3f4043781590b4c0a69e9237549023ccd441c18083f11decd5 ;;
  v47.0.2/aarch64-macos) digest=06d53af42ef3cbef5c7d44c14a6693b3456ac3d9df00950fb202075e27314f3e ;;
  v47.0.2/x86_64-macos)  digest=548b37f774d55e845f1d0407d9d9bbba8799cbabe45d617d5d0127706badd08b ;;
  *) echo "no recorded digest for ${asset}; add one before pinning this release" >&2; exit 1 ;;
esac
install_dir="${1:-.tools/wasmtime}"
mkdir -p "$install_dir"
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
curl -fsSL "https://github.com/bytecodealliance/wasmtime/releases/download/$tag/$asset" -o "$scratch/$asset"
python3 - "$scratch/$asset" "$digest" <<'PY'
import hashlib, pathlib, sys
path, expected = pathlib.Path(sys.argv[1]), sys.argv[2]
if hashlib.sha256(path.read_bytes()).hexdigest() != expected:
    raise SystemExit('Wasmtime checksum mismatch')
PY
tar -xJf "$scratch/$asset" -C "$scratch"
cp "$scratch/wasmtime-${tag}-${arch}-${os}/wasmtime" "$install_dir/wasmtime"
chmod +x "$install_dir/wasmtime"
"$install_dir/wasmtime" --version
