#!/usr/bin/env bash
#
# Local release builder. Mirrors the GitHub Actions release workflow so
# that the same tarball + checksums can be produced and signed locally.
#
# Usage:
#   ./scripts/release.sh <version>
#
# Example:
#   ./scripts/release.sh 0.1.0
#
# Produces in ./dist/:
#   sleepyminer-<version>-macos-arm64.tar.gz
#   SHA256SUMS
#   SHA256SUMS.asc      (only if GPG_KEY_ID is set)
#
# Requires: rust toolchain, cmake, gpg (if signing).

set -euo pipefail

if [ "$#" -ne 1 ]; then
  echo "usage: $0 <version>" >&2
  exit 2
fi

VERSION="$1"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DIST="$ROOT/dist"
STAGE="sleepyminer-${VERSION}-macos-arm64"

cd "$ROOT"

echo "==> Building release binary"
cargo build --release --locked

echo "==> Running RandomX correctness tests"
(
  cd vendor/randomx
  mkdir -p build
  cd build
  cmake -DCMAKE_BUILD_TYPE=Release .. >/dev/null
  make -j"$(sysctl -n hw.ncpu)" >/dev/null
  ./randomx-tests | tee /tmp/sleepyminer-randomx-tests.log
  grep -q 'All tests PASSED' /tmp/sleepyminer-randomx-tests.log
)

echo "==> Staging release tarball"
rm -rf "$DIST"
mkdir -p "$DIST/$STAGE"
cp target/release/sleepyminer "$DIST/$STAGE/"
cp README.md LICENSE CREDITS.md "$DIST/$STAGE/"
cp config.example.json "$DIST/$STAGE/"

(
  cd "$DIST"
  tar czf "${STAGE}.tar.gz" "$STAGE"
  shasum -a 256 "${STAGE}.tar.gz" > SHA256SUMS
  echo "    ${STAGE}.tar.gz"
  echo "    SHA256SUMS"
)

if [ -n "${GPG_KEY_ID:-}" ]; then
  echo "==> Signing SHA256SUMS with $GPG_KEY_ID"
  (
    cd "$DIST"
    gpg --batch --yes --local-user "$GPG_KEY_ID" \
        --detach-sign --armor SHA256SUMS
    echo "    SHA256SUMS.asc"
  )
else
  echo "==> Skipping signing (set GPG_KEY_ID to enable)"
fi

echo
echo "==> Done. Artifacts in: $DIST/"
ls -la "$DIST/" | grep -v '^d'
