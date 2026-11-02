# AGENTS: Windows scripts

Instructions for agents. The human-readable list is in [README.md](README.md).

Start from the examples below. Open a script only when the example does not cover your case.

## build

`env.ps1` — sets MSVC, vcpkg, FFmpeg and libclang; dot-source it, never run it alone.

```powershell
. .\scripts\windows\build\env.ps1
```

`env-fast.ps1` — the fast variant for test runs.

```powershell
. .\scripts\windows\build\env-fast.ps1
```

`build.ps1` — builds `dist\windows\mediaa.exe`.

```powershell
.\scripts\windows\build\build.ps1
```

`build-agent.ps1` — builds `target\debug\mediaa-agent.exe`: the agent copy with the red icon
embedded in the executable, so Task Manager and Explorer show it too.

```powershell
.\scripts\windows\build\build-agent.ps1
```

## run

`run.ps1` — builds and runs MediaA in debug; `-Release` for the release profile.

```powershell
.\scripts\windows\run\run.ps1
.\scripts\windows\run\run.ps1 -Release
```

`run.cmd` — the same from a double click or `cmd`.

```powershell
.\scripts\windows\run\run.cmd -Release
```

## test

`test.ps1` — the full test suite with the build environment.

```powershell
.\scripts\windows\test\test.ps1
```

`test-fast.ps1` — the fast test set before a commit.

```powershell
.\scripts\windows\test\test-fast.ps1
```

`test-release.ps1` — the suite in the release profile.

```powershell
.\scripts\windows\test\test-release.ps1
```

`region-overlay-smoke.ps1` — smoke check of the region picker overlay.

```powershell
.\scripts\windows\test\region-overlay-smoke.ps1
```

## checks

`run-background.ps1` — starts any MediaA binary fully in the background: off-screen windows, no
activation, no picker mask.

```powershell
.\scripts\windows\checks\run-background.ps1 -Exe target\debug\mediaa-agent.exe -Arguments @('--daemon')
.\scripts\windows\checks\run-background.ps1 -Exe target\debug\mediaa-agent.exe -Arguments @('--area')
.\scripts\windows\checks\run-background.ps1 -Exe target\debug\mediaa-agent.exe -Arguments @('--page=library')
```

`measure-memory.ps1` — memory/CPU audit of the states, writes CSV/MD/JSON to `target/perf`.

```powershell
.\scripts\windows\checks\measure-memory.ps1 -States about,library,tray
.\scripts\windows\checks\measure-memory.ps1 -States about,tray -Env @{ WGPU_BACKEND = 'gl' }
.\scripts\windows\checks\measure-memory.ps1 -States tray -WarmupSec 5 -SampleSec 20
```

`measure-picker.ps1` — cold-start latency of the region picker; background mode by default.

```powershell
.\scripts\windows\checks\measure-picker.ps1 -Runs 6 -WarmDaemon
.\scripts\windows\checks\measure-picker.ps1 -Runs 6 -WarmDaemon -Env @{ WGPU_BACKEND = 'gl' }
```

`record-smoke.ps1` — checks daemon recording without UI: starts `--daemon` with
`MEDIAA_DEV_RECORD=<seconds>`, waits for the output file and writes memory/CPU stats to `target/perf`.
The recording goes to the isolated dev library; the script removes it unless `-Keep`.

```powershell
.\scripts\windows\checks\record-smoke.ps1 -Seconds 3
.\scripts\windows\checks\record-smoke.ps1 -Seconds 5 -Keep
```

`tray-action.ps1` — starts the debug agent copy with a virtual tray command
(`show`, `region`, `fullscreen`, `start`, `audio`, `pause`, `resume`, `stop`, `quit`, `primary`)
through debug-only `MEDIAA_DEV_TRAY`. `show` intentionally makes the red agent window visible;
use it only when a tray-open check is requested.

```powershell
.\scripts\windows\checks\tray-action.ps1 -Action quit -WaitSec 6
.\scripts\windows\checks\tray-action.ps1 -Action show -WaitSec 8
```

`list-windows.ps1` — lists windows with handle, PID, visibility and rect.

```powershell
.\scripts\windows\checks\list-windows.ps1 -TitleLike MediaA
.\scripts\windows\checks\list-windows.ps1 -TitleLike MediaA -All
.\scripts\windows\checks\list-windows.ps1 -ProcessId 1234 -All
.\scripts\windows\checks\list-windows.ps1 -TitleLike MediaA -Json
```

`capture-screen.ps1` — saves the whole screen (or one monitor) to a PNG; DPI-aware.

```powershell
.\scripts\windows\checks\capture-screen.ps1 -Out target\screenshots\screen.png
.\scripts\windows\checks\capture-screen.ps1 -Monitor 0
```

`capture-window.ps1` — saves one window to a PNG by title, handle or process id.

```powershell
.\scripts\windows\checks\capture-window.ps1 -Title "MediaA area picker" -Out target\screenshots\picker.png
.\scripts\windows\checks\list-windows.ps1 -TitleLike MediaA
.\scripts\windows\checks\capture-window.ps1 -Handle 123456 -Out target\screenshots\window.png
```

`send-input.ps1` — feeds synthetic input into the app through `MEDIAA_DEV_INPUT`, so the agent
does not touch the real keyboard or mouse. Keyboard, pointer drag and click are verified: a drag
plus a click on the picker's copy button puts the region into the clipboard. Coordinates are
viewport points (`overlay` is 1600×1000 on this machine).

```powershell
$env:MEDIAA_DEV_INPUT = "$PWD\target\demo\input.jsonl"
.\scripts\windows\checks\run-background.ps1 -Exe target\debug\mediaa-agent.exe -Arguments @('--area')
.\scripts\windows\checks\send-input.ps1 -Drag "200,200,700,600"
.\scripts\windows\checks\send-input.ps1 -Click "1222,968"
.\scripts\windows\checks\send-input.ps1 -Key Escape
```

After a synthetic action, verify the result through app state or the clipboard, not through a
window screenshot: an off-screen window may capture a stale frame.

## Rules for agents

Read this file before any build, run or check on Windows: it is the platform contract.

- **Run the app only through the scripts.** For tests use `checks/run-background.ps1` or a
  measurement script; do not start `mediaa.exe` directly for UI checks unless the user explicitly
  asks for another way.
- **Never let test windows appear on the user's screen.** The background scripts keep windows
  off-screen, non-activating and hide the picker mask.
- **Screenshots only through the scripts:** `checks/capture-screen.ps1` for what the user sees and
  `checks/capture-window.ps1` for a window. Do not write your own capture code. A hidden window
  captures as a black frame: show it off-screen first (`run-background.ps1`); the region picker
  must run with `MEDIAA_DEV_PICKER=1` to be capturable. Default output is `target/screenshots`.
- **UI input only through `checks/send-input.ps1`**, and only when the check really needs a key,
  click or drag. If the check can be a normal `cargo test` or an `egui_kittest` UI test, write that
  instead of driving the app.
- **The agent copy is separate and marked red.** The scripts use `mediaa-agent.exe`: its
  embedded executable icon, window icon and tray icon are red, so Task Manager, Explorer, the
  window and the tray show the agent copy. The scripts also set `MEDIAA_INSTANCE_ID` and
  `MEDIAA_DEV_LIBRARY`; never touch the user's instance, its tray or its library.
- `MEDIAA_DEV_BACKGROUND=1`, `MEDIAA_DEV_AGENT=1` and `MEDIAA_DEV_RECORD=<seconds>` are
  debug-only and are applied by the check scripts; regular application runs are not affected.
  `MEDIAA_DEV_RECORD` starts a short region recording in the daemon and is used only by
  `checks/record-smoke.ps1`.
