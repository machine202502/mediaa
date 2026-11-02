# AGENTS: Linux scripts

Instructions for agents. The human-readable list is in [README.md](README.md).

- Build: `scripts/linux/build.sh` produces `dist/linux/mediaa`. `--run` builds and starts it.
- Docker build: `scripts/linux/build-docker.sh` (from Linux/macOS) or `scripts/linux/build-docker.ps1`
  (from Windows) build inside the Ubuntu 24.04 container from `Dockerfile`.
- Smoke tests: `scripts/linux/smoke-test.sh` runs inside the container,
  `scripts/linux/smoke-test-docker.ps1` runs it from Windows (`-Lang ru_RU.UTF-8` for Russian UI).
- Runtime dependencies: `scripts/linux/install-runtime-deps.sh` installs the system GUI/audio libs.
- From Windows, `scripts/wsl/run-linux.ps1` runs the Linux build through WSL.
- The project-level process for choosing the OS document is in
  [specs/rules.md](../../specs/rules.md), section "Как тестировать".
