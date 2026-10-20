#!/usr/bin/env bash
# Builds MediaA for macOS with statically linked FFmpeg and wraps it into MediaA.app
# (an app bundle is needed for the microphone / screen recording permission prompts).
#
#   scripts/macos/build.sh          # -> dist/macos/MediaA.app
#   scripts/macos/build.sh --test   # run the test suite
#   scripts/macos/build.sh --run    # debug run
#
# Prerequisites: Xcode command line tools, Homebrew: brew install nasm pkg-config cmake ninja autoconf automake libtool
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
case "$(uname -m)" in
  arm64) TRIPLET=arm64-osx-release ;;
  *) TRIPLET=x64-osx-release ;;
esac

if [[ -z "${VCPKG_ROOT:-}" ]]; then
  VCPKG_ROOT="$ROOT/.tools/vcpkg"
  if [[ ! -x "$VCPKG_ROOT/vcpkg" ]]; then
    git clone https://github.com/microsoft/vcpkg "$VCPKG_ROOT"
    git -C "$VCPKG_ROOT" checkout "$(cat "$ROOT/scripts/vcpkg-commit.txt")"
    "$VCPKG_ROOT/bootstrap-vcpkg.sh" -disableMetrics
  fi
fi

"$VCPKG_ROOT/vcpkg" install --triplet "$TRIPLET" --x-manifest-root="$ROOT" --x-install-root="$ROOT/vcpkg_installed"
export FFMPEG_DIR="$ROOT/vcpkg_installed/$TRIPLET"
cd "$ROOT"

case "${1:-build}" in
  --test) cargo test --workspace ;;
  --run) cargo run -p mediaa ;;
  *)
    cargo build --release -p mediaa
    APP="$ROOT/dist/macos/MediaA.app"
    rm -rf "$APP"
    mkdir -p "$APP/Contents/MacOS"
    cp target/release/mediaa "$APP/Contents/MacOS/mediaa"
    cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>MediaA</string>
  <key>CFBundleIdentifier</key><string>dev.mediaa.app</string>
  <key>CFBundleExecutable</key><string>mediaa</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>LSMinimumSystemVersion</key><string>12.3</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSMicrophoneUsageDescription</key><string>MediaA records audio for your screen recordings.</string>
  <key>NSAudioCaptureUsageDescription</key><string>MediaA records system sound for your screen recordings.</string>
</dict>
</plist>
PLIST
    echo "done: dist/macos/MediaA.app ($(du -sh "$APP" | cut -f1))"
    ;;
esac
