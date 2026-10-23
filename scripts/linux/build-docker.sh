#!/usr/bin/env bash
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
IMAGE=mediaa-linux-builder
NAME=mediaa-linux-build
JOBS="${JOBS:-8}"

docker build -t "$IMAGE" --build-arg VCPKG_COMMIT="$(cat "$ROOT/scripts/vcpkg-commit.txt")" -f "$HERE/Dockerfile" "$HERE"
docker rm -f "$NAME" >/dev/null 2>&1 || true
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
