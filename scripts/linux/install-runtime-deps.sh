#!/usr/bin/env bash
# Shared libraries the Linux build needs at runtime (FFmpeg itself is linked in statically).
# Debian/Ubuntu (also inside WSL). Run as root / with sudo.
set -euo pipefail

apt-get update
pkgs=(
  libgl1 libegl1 libgl1-mesa-dri libgbm1 libvulkan1 mesa-vulkan-drivers
  libxkbcommon0 libxkbcommon-x11-0 libwayland-client0 libwayland-egl1 libwayland-cursor0
  libx11-6 libx11-xcb1 libxcursor1 libxi6 libxrandr2 libxtst6
  libxcb1 libxcb-randr0 libxcb-shm0 libxcb-xfixes0 libxcb-shape0
  libpipewire-0.3-0 libdbus-1-3 xdg-utils fonts-dejavu-core
)
# ALSA was renamed for the 64-bit time_t transition (Ubuntu 24.04+, Debian 13+).
if apt-cache show libasound2t64 >/dev/null 2>&1; then pkgs+=(libasound2t64); else pkgs+=(libasound2); fi
apt-get install -y --no-install-recommends "${pkgs[@]}"
echo "runtime dependencies installed"
