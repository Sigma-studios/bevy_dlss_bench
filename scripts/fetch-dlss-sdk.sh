#!/usr/bin/env bash
# Fetch the pinned NVIDIA DLSS SDK into vendor/DLSS (what .cargo/config.toml points DLSS_SDK at).
#
# dlss_wgpu 4.0.0 requires DLSS SDK v310.5.3. Only the parts the build and runtime need are
# checked out (headers, the Linux static lib, the release runtime .so, license + programming guide).
# By fetching it you agree to the DLSS SDK license:
#   https://github.com/NVIDIA/DLSS/blob/v310.5.3/LICENSE.txt
set -euo pipefail

VERSION="v310.5.3"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEST="${DLSS_SDK_DEST:-$ROOT/vendor/DLSS}"
STAMP="$DEST/.bevy_dlss_bench-$VERSION"

if [[ -f "$STAMP" ]]; then
  exit 0
fi

echo "Fetching NVIDIA DLSS SDK $VERSION into $DEST"
echo "(DLSS SDK license: https://github.com/NVIDIA/DLSS/blob/$VERSION/LICENSE.txt)"
rm -rf "$DEST"
mkdir -p "$(dirname "$DEST")"
git clone --quiet --depth 1 --branch "$VERSION" --filter=blob:none --no-checkout \
  https://github.com/NVIDIA/DLSS.git "$DEST"
git -C "$DEST" sparse-checkout set --no-cone \
  '/include/' \
  '/lib/Linux_x86_64/*.a' \
  "/lib/Linux_x86_64/rel/libnvidia-ngx-dlss.so.${VERSION#v}" \
  '/LICENSE.txt' \
  '/doc/DLSS_Programming_Guide_Release.pdf'
git -C "$DEST" checkout --quiet "$VERSION"
touch "$STAMP"
echo "DLSS SDK ready: $DEST"
