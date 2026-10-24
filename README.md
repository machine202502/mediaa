# MediaA

Cross-platform screen capture tool written in Rust: Lightshot-style screenshots, OBS-style
recordings, audio recording and conversion between popular formats. The release is **one program**,
`mediaa` (one executable, FFmpeg linked in statically).

The window has three sections:

| Section | What it does |
|---|---|
| **Library** | Every screenshot, recording and conversion (images, videos, animations, audio) |
| **Capture** | Screenshots, video recording (screen / area / windows), audio recording |
| **Convert** | Video, animation, image and audio conversions |

## Features

- **Screenshots** — Lightshot-style area picker in its own fullscreen window over a frozen screen
  (move/resize handles, keyboard nudging, annotations: brush, line, arrow, rectangle,
  highlighter, text, colours, thickness, undo),
  multi-monitor; or full screen / a single window. Copy to clipboard and/or save as PNG / WebP.
- **Recording** — full screen, an area, or several windows at once (each window → its own file).
  MP4 / MKV (H.264 + AAC) with microphone and/or system sound, or GIF / animated WebP
  (recorded losslessly, then encoded with a per-clip palette). Pause / resume / stop / cancel from
  the window or the **tray** (the window hides to the tray while recording). The tray and hotkeys
  have separate quick-video and region presets: MKV at 60 fps by default, or WebP at high quality.
  After a recording or a tray screenshot finishes, MediaA opens the Library by default or can save
  the result and copy its files to the clipboard. Audio and video stopped from the tray or by the
  stop hotkey use the built-in confirmation chime; stopping from Capture, screenshots and GIF/WebP
  stay silent.
- **Audio recording** — microphone and / or system sound into MP3, M4A, WAV, FLAC or OGG (Opus).
  One recording at a time (video or audio); separate start hotkeys, one stop hotkey.
- **Library** — a gallery of every screenshot, recording and conversion (images, videos,
  animations, audio): hover previews, a viewer with zoom / playback, search, sorting, rename,
  open, show in folder, copy file / image / path / `file://` link, convert, delete.
- **Convert** — between popular formats: video (MP4, MOV, MKV, WebM, AVI) ⇄ video / animation
  (GIF, WebP) / audio; images (JPEG, PNG, WebP, BMP, TIFF); audio (MP3, M4A, WAV, FLAC, OGG).
  Quality / resolution / bitrate settings, sound removal, output name and folder (by default
  the library folder, set in Settings); progress and cancel.
- **Global hotkeys** (configurable), **dark / light / system theme**, **languages**: English and
  Russian (system language by default). Closing the window hides the app to the **tray** (on by
  default, switchable in Settings); a left click starts an area screenshot by default, can open the
  window instead, and stops and saves a running recording by default; quitting is done from the
  tray menu.

## Project layout

```
crates/
  core/      settings, selection geometry, annotations (tiny-skia), pacing, naming, library, WAV  (no FFmpeg)
  media/     FFmpeg pipeline via ez-ffmpeg: recording sessions, muxing, conversions, thumbnails
  platform/  OS integration: xcap (screenshots; macOS/Wayland recording), windows-capture (Windows recording), xcb + MIT-SHM (X11 recording), cpal (audio), clipboard, open/trash
  app/       egui UI (eframe + wgpu), tray, hotkeys, area picker, i18n (locales/*.toml) -> `mediaa`
scripts/     build / run / test helpers for every platform (see below)
vcpkg.json   static FFmpeg 8.1 (avcodec, avformat, avfilter, swscale, swresample, x264, libvpx, libwebp, opus, mp3lame, dav1d)
```

Main third-party building blocks: [ez-ffmpeg](https://crates.io/crates/ez-ffmpeg) (FFmpeg CLI
pipeline as a library), [xcap](https://crates.io/crates/xcap), [windows-capture](https://crates.io/crates/windows-capture), [cpal](https://crates.io/crates/cpal),
[eframe/egui](https://crates.io/crates/eframe), [tray-icon](https://crates.io/crates/tray-icon),
[global-hotkey](https://crates.io/crates/global-hotkey), [rust-i18n](https://crates.io/crates/rust-i18n).

## Prerequisites

| | Windows | Linux | macOS |
|---|---|---|---|
| Rust | stable, `x86_64-pc-windows-msvc` | stable | stable |
| C/C++ toolchain | Visual Studio 2022+ "Desktop development with C++" **and** "C++ Clang Compiler for Windows" (libclang for bindgen) | `build-essential`, `clang`, `libclang-dev` | Xcode command line tools |
| FFmpeg build | vcpkg (cloned automatically if missing) | vcpkg + `nasm pkg-config cmake ninja-build autoconf automake libtool` | vcpkg + `brew install nasm pkg-config cmake ninja autoconf automake libtool` |
| System libs | — | see `scripts/linux/Dockerfile` (pipewire, xcb, alsa, dbus, xkbcommon, wayland, GL dev packages) | — |
| Optional | Docker Desktop (Linux builds), WSL2 (running Linux builds) | Docker | Docker |

FFmpeg is built **once** by vcpkg into `vcpkg_installed/` (10–30 min) and then reused. The vcpkg
commit is pinned in `scripts/vcpkg-commit.txt`.

## Run for testing

**Windows** (PowerShell):

```powershell
.\scripts\windows\run.ps1            # debug build, logs in the console
.\scripts\windows\run.ps1 -Release   # optimized
```

`env.ps1` (used by all Windows scripts) finds or clones vcpkg, finds `libclang.dll` via
`vswhere`, builds FFmpeg on the first run and sets `VCPKG_ROOT`, `LIBCLANG_PATH`, `FFMPEG_DIR`.

## Git hooks

`node scripts/install-git-hooks.mjs` (the Windows, Linux and macOS build scripts do it too) sets
`core.hooksPath` to `.githooks`. Every `git commit`, `push`, `rebase` and `merge` then prints one
question — «did the human give explicit consent to this push (branch …, commit …)? yes or no» — and
stops until it is answered. One «yes» allows one call; it is bound to that branch and commit, and
every answer is logged in `.git/consent.log`. Without a terminal (an agent, CI) the hook refuses and
explains how to confirm explicitly. `--no-verify` and switching `core.hooksPath` away are forbidden
by [CLAUDE.md](CLAUDE.md).

**Linux:**

```bash
export VCPKG_ROOT=~/vcpkg      # a vcpkg checkout (bootstrap-vcpkg.sh)
scripts/linux/build.sh --run
```

**macOS:**

```bash
scripts/macos/build.sh --run   # clones vcpkg into .tools/ if VCPKG_ROOT is not set
```

On macOS grant *Screen Recording* (and *Microphone*) permissions in System Settings → Privacy
& Security on first use.

## Build

| Platform | Command | Result |
|---|---|---|
| Windows | `.\scripts\windows\build.ps1` | `dist\windows\mediaa.exe` — a single file, FFmpeg + C runtime static (only system DLLs) |
| Linux (native) | `scripts/linux/build.sh` | `dist/linux/mediaa` — FFmpeg static; system GUI/audio libs dynamic |
| Linux (Docker) | `scripts/linux/build-docker.sh` | the same program, built in an Ubuntu 24.04 container |
| macOS | `scripts/macos/build.sh` | `dist/macos/MediaA.app` |

Manual equivalent (Windows): `vcpkg install --triplet x64-windows-static-release
--x-manifest-root=. --x-install-root=vcpkg_installed`, then `cargo build --release -p mediaa`
(`.cargo/config.toml` points `FFMPEG_DIR` at `vcpkg_installed/x64-windows-static-release`
and enables `+crt-static`).

## Building the Linux version on Windows

Requires **Docker Desktop** (Linux containers).

```powershell
.\scripts\linux\build-docker.ps1          # -> dist\linux\mediaa
.\scripts\linux\build-docker.ps1 -Test    # run the whole test suite on Linux
```

What happens:

1. `scripts/linux/Dockerfile` builds an image `mediaa-linux-builder` (Ubuntu 24.04 — the PipeWire
   bindings need PipeWire ≥ 1.0, so the binary needs glibc ≥ 2.39: Ubuntu 24.04+, Debian 13+, Fedora 40+; in WSL use `Ubuntu-24.04`) with Rust, the dev
   packages and vcpkg at the pinned commit.
2. The repo is mounted at `/src`; `scripts/linux/build.sh` runs inside the container.
3. vcpkg output, the cargo `target` dir and caches live in Docker volumes
   (`mediaa-work`, `mediaa-cargo-registry`, `mediaa-vcpkg-cache`), so the first run is slow
   (FFmpeg) and later runs are incremental. Remove them with
   `docker volume rm mediaa-work mediaa-cargo-registry mediaa-vcpkg-cache`.

## Running the Linux build on Windows (WSL)

Requires **WSL2 with WSLg** (Windows 11, or Windows 10 with the Store version of WSL): Linux GUI
apps appear as normal windows, with sound.

```powershell
wsl --install Ubuntu-24.04                          # once, if you do not have it
.\scripts\wsl\run-linux.ps1 -InstallDeps            # first run: installs runtime libs (sudo)
.\scripts\wsl\run-linux.ps1                         # run dist/linux/mediaa
.\scripts\wsl\run-linux.ps1 -Build                  # rebuild in Docker first
.\scripts\wsl\run-linux.ps1 -Distro Ubuntu-26.04     # another distro (glibc >= 2.39)
```

The script copies the binary into the distro (`~/.local/bin/mediaa`, so exec permissions and
speed are native) and starts it. `scripts/linux/install-runtime-deps.sh` lists the shared
libraries the binary needs on any Debian/Ubuntu system.

Inside WSL the app captures the **WSLg desktop** (Linux windows), not the Windows desktop —
it is meant for testing the Linux build.

Without WSL, the Linux build can be smoke-tested headless in Docker — a clean `ubuntu:24.04`
container gets only the runtime libraries, `ldd` must resolve everything, the app is started on a
virtual X server and a screenshot is saved to `dist/linux/smoke.png`:

```powershell
.\scripts\linux\smoke-test-docker.ps1               # -Lang ru_RU.UTF-8 to check the Russian UI
```

## Tests

```powershell
.\scripts\windows\test.ps1          # or: cargo test --workspace (with the env from env.ps1)
```

```bash
scripts/linux/build.sh --test       # Linux
scripts/linux/build-docker.sh --test
```

The suite includes unit tests (geometry, pacing, settings, naming, library, filter graphs,
i18n key coverage…) and end-to-end tests that really encode with the statically linked FFmpeg:
recording from synthetic sources with synthetic audio (MP4 + mixed audio tracks, MKV per window,
GIF, WebP), pause accounting, cancel, every conversion and thumbnails.

Tests against the real desktop (screen/window capture, recording the actual screen to MP4) need a
display session and are skipped by default:

```bash
cargo test -p mediaa-platform --test live -- --ignored
```

## Languages

Translations live in `crates/app/locales/<code>.toml` (compiled into the binary). To add a
language: copy `en.toml` to e.g. `de.toml`, translate it, and add `("de", "Deutsch")` to
`LANGUAGES` in `crates/app/src/i18n.rs`. Tests fail if any language misses a key or the code uses
a key that does not exist. The default is the system language (falls back to English); it can be
changed in Settings.

## Where files go

| What | Location |
|---|---|
| Screenshots, recordings, conversions | `Documents/MediaA` (changeable in Settings) |
| Library cache | `.cache/` in that folder: `library.db` (SQLite: file index with titles, sizes, dates, dimensions, durations, and thumbnail slots; every change is written by row) and thumbnail atlases `thumbs-N.jpg`; everything is rebuilt when missing |
| Settings | Windows `%APPDATA%\MediaA\MediaA\config\settings.toml`, Linux `~/.config/mediaa`, macOS `~/Library/Application Support/dev.MediaA.MediaA` |
| Logs, scratch files | the platform's local data dir (`…\MediaA\data\logs`) |

## Platform notes

- **Hotkeys** are not set by default; assign them in Settings. On **Windows 11** `PrintScreen`
  opens the Snipping Tool; to give it to MediaA turn off *Settings → Accessibility → Keyboard →
  Use the Print screen key to open screen capture*.
- **Recording backends** (frames are taken from a stream, so recordings keep their frame rate
  on large displays; each falls back to per-frame screenshots if unavailable):
  - Windows: Windows Graphics Capture (`windows-capture`), cursor drawn by Windows.
  - Linux X11: persistent XCB connection + MIT-SHM (like FFmpeg `x11grab`), real cursor via XFixes.
  - Linux Wayland: PipeWire through the ScreenCast portal (`xcap`); asks for permission for each
    recording; the cursor is not added.
  - macOS: AVFoundation screen stream (`xcap`), cursor always shown; window recording uses
    screenshots.
- **System sound**: Windows (WASAPI loopback) and macOS 14.6+. On Linux pick a
  "Monitor of …" PulseAudio/PipeWire device as the microphone.
- **Linux Wayland**: screenshots and recordings go through the desktop portal (a permission prompt
  appears); global hotkeys need X11/XWayland.

## License

Source code: MIT. The binaries link FFmpeg built with **libx264 (GPL)**, so distributed builds
are covered by the GPL. To ship a non-GPL build, drop `gpl`/`x264` from `vcpkg.json` (e.g. use
`openh264`) and adjust the encoder in `crates/media`.
