#!/usr/bin/env bash
# Builds MediaA for Linux with statically linked FFmpeg.
# Works natively on Linux and is also what the Docker build runs inside the container.
#
#   scripts/linux/build.sh            # release build -> dist/linux/mediaa
#   scripts/linux/build.sh --test     # run the test suite instead
#   scripts/linux/build.sh --run      # debug run (needs a desktop session)
#
# Env: VCPKG_ROOT (required), MEDIAA_WORK (where vcpkg_installed/target live; default: repo).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORK="${MEDIAA_WORK:-$ROOT}"
TRIPLET="${MEDIAA_TRIPLET:-x64-linux-release}"
MODE="${1:-build}"

if [[ -z "${VCPKG_ROOT:-}" ]]; then
  echo "error: set VCPKG_ROOT to a vcpkg checkout (commit: $(cat "$ROOT/scripts/vcpkg-commit.txt"))" >&2
  exit 1
fi

echo "== vcpkg: static FFmpeg ($TRIPLET) =="
"$VCPKG_ROOT/vcpkg" install \
  --triplet "$TRIPLET" \
  --x-manifest-root="$ROOT" \
  --x-install-root="$WORK/vcpkg_installed"

export FFMPEG_DIR="$WORK/vcpkg_installed/$TRIPLET"
export CARGO_TARGET_DIR="$WORK/target"
cd "$ROOT"

case "$MODE" in
  --test)
    echo "== cargo test =="
    cargo test --workspace
    ;;
  --run)
    cargo run -p mediaa
    ;;
  *)
    echo "== cargo build --release =="
    cargo build --release -p mediaa
    mkdir -p "$ROOT/dist/linux"
    cp "$CARGO_TARGET_DIR/release/mediaa" "$ROOT/dist/linux/mediaa"
    strip "$ROOT/dist/linux/mediaa" || true
    echo "done: dist/linux/mediaa ($(du -h "$ROOT/dist/linux/mediaa" | cut -f1))"
    ;;
esac
