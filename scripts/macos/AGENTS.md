# AGENTS: macOS scripts

Instructions for agents. The human-readable list is in [README.md](README.md).

- Build: `scripts/macos/build.sh` produces `dist/macos/MediaA.app`; `--run` builds and starts it.
  It clones vcpkg into `.tools/` when `VCPKG_ROOT` is not set.
- On first run macOS asks for Screen Recording and Microphone permissions: grant them in
  System Settings → Privacy & Security.
- The project-level process for choosing the OS document is in
  [specs/rules.md](../../specs/rules.md), section "Как тестировать".
