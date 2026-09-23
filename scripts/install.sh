#!/usr/bin/env bash
# Install a released porta binary. No compiler, no toolchain, no build.
#
#   curl -fsSL https://raw.githubusercontent.com/almide/porta/main/scripts/install.sh | bash
#
# PORTA_RELEASE_TAG selects a version (default: the latest release).
# The first argument, or PORTA_INSTALL_DIR, selects where it lands.
set -euo pipefail

case "$(uname -s)" in
  Darwin) os=macos ;;
  Linux)  os=linux ;;
  *) echo "porta runs on macOS and Linux; native execution has no backend on $(uname -s)." >&2; exit 1 ;;
esac
case "$(uname -m)" in
  arm64|aarch64) arch=aarch64 ;;
  x86_64)        arch=x86_64 ;;
  *) echo "no published binary for $(uname -m)" >&2; exit 1 ;;
esac

asset="porta-${os}-${arch}.tar.gz"
install_dir="${1:-${PORTA_INSTALL_DIR:-"$HOME/.local/bin"}}"
repo="https://github.com/almide/porta"

if [ -n "${PORTA_RELEASE_TAG:-}" ]; then
  base="$repo/releases/download/$PORTA_RELEASE_TAG"
else
  base="$repo/releases/latest/download"
fi

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT

# A published binary that is not checked is a published binary somebody else
# can replace, so the checksum is fetched and compared rather than trusted.
if ! curl -fsSL "$base/$asset" -o "$scratch/$asset"; then
  echo "No published $asset." >&2
  if [ "$os-$arch" = "macos-x86_64" ]; then
    echo >&2
    echo "porta publishes binaries for Apple silicon and for Linux on both" >&2
    echo "architectures. An Intel Mac is not among them: GitHub's Intel macOS" >&2
    echo "runner could not be obtained to build and test one, and porta does not" >&2
    echo "publish a binary no machine has executed. Building from source works;" >&2
    echo "with Almide installed, the way go install does it:" >&2
    echo >&2
    echo "  almide install github.com/almide/porta" >&2
    echo >&2
    echo "or by hand:" >&2
    echo >&2
    echo "  git clone $repo && cd porta" >&2
    echo "  bash scripts/install-almide.sh" >&2
    echo "  .tools/almide/almide build src/main.almd -o target/porta" >&2
  else
    echo "Releases: $repo/releases — or build from source, see the README." >&2
  fi
  exit 1
fi
curl -fsSL "$base/porta-checksums.sha256" -o "$scratch/checksums"

python3 - "$scratch" "$asset" <<'PY'
import hashlib, pathlib, sys
root, asset = pathlib.Path(sys.argv[1]), sys.argv[2]
published = dict(
    (name.lstrip('*'), digest)
    for digest, name in (line.split() for line in (root / 'checksums').read_text().split('\n') if line.strip())
)
if asset not in published:
    raise SystemExit(f'{asset} is not listed in the published checksums')
if hashlib.sha256((root / asset).read_bytes()).hexdigest() != published[asset]:
    raise SystemExit(f'{asset} does not match its published checksum; not installing')
PY

tar -xzf "$scratch/$asset" -C "$scratch"
mkdir -p "$install_dir"
cp "$scratch/porta-${os}-${arch}/porta" "$install_dir/porta"
chmod +x "$install_dir/porta"

echo "Installed $("$install_dir/porta" --version 2>/dev/null || echo porta) to $install_dir/porta"
case ":$PATH:" in
  *":$install_dir:"*) ;;
  *) echo "$install_dir is not on your PATH. Add it, or move the binary somewhere that is." ;;
esac
