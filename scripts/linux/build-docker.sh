#!/usr/bin/env bash
# Builds the Linux version inside Docker (from Linux or macOS hosts).
#   scripts/linux/build-docker.sh [--test]        JOBS=4 scripts/linux/build-docker.sh
# Output: dist/linux/mediaa. vcpkg/cargo caches live in Docker volumes (mediaa-*).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
IMAGE=mediaa-linux-builder
NAME=mediaa-linux-build
JOBS="${JOBS:-8}"

docker build -t "$IMAGE" --build-arg VCPKG_COMMIT="$(cat "$ROOT/scripts/vcpkg-commit.txt")" -f "$HERE/Dockerfile" "$HERE"
docker rm -f "$NAME" >/dev/null 2>&1 || true
# Detached + `logs -f`: a dropped client connection does not abort the build.
docker run -d --name "$NAME" \
  -v "$ROOT:/src" \
  -v mediaa-work:/work \
  -v mediaa-cargo-registry:/root/.cargo/registry \
  -v mediaa-vcpkg-cache:/root/.cache/vcpkg \
  -e MEDIAA_WORK=/work \
  -e VCPKG_MAX_CONCURRENCY="$JOBS" \
  -e CARGO_BUILD_JOBS="$JOBS" \
  "$IMAGE" bash /src/scripts/linux/build.sh "${1:-build}" >/dev/null
docker logs -f "$NAME" || true
CODE="$(docker wait "$NAME")"
docker rm "$NAME" >/dev/null
exit "$CODE"
