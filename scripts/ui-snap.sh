#!/usr/bin/env bash
# A screenshot of MediaA's own window, for checking UI changes (subagents save these as artifacts).
# Starts the release build on a test library, captures ONLY the window of the process it started
# (by its pid, never the rest of the screen or another copy of the app), then closes that process.
#
#   scripts/ui-snap.sh OUT.png [--page NAME] [--open NAME_PART] [--library DIR] [--wait SECONDS]
#
# --page    the page to open (library, record, catalogs, queue, budget, providers, models, …)
# --open    open the first catalog file (or a folder's chat) whose name contains NAME_PART
# --library the media library (default: $MEDIAA_TEST_LIBRARY, else target/ui-library — empty)
# --wait    seconds before the shot (default 8)
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="" page="" open="" library="${MEDIAA_TEST_LIBRARY:-$repo_root/target/ui-library}" wait=8
while [[ $# -gt 0 ]]; do
    case "$1" in
        --page) page="$2"; shift 2 ;;
        --open) open="$2"; shift 2 ;;
        --library) library="$2"; shift 2 ;;
        --wait) wait="$2"; shift 2 ;;
        -h|--help) sed -n '2,13p' "$0"; exit 0 ;;
        *) [[ -z "$out" ]] || { echo "one OUT.png only" >&2; exit 2; }; out="$1"; shift ;;
    esac
done
[[ -n "$out" ]] || { sed -n '2,13p' "$0" >&2; exit 2; }
mkdir -p "$(dirname "$out")" "$library"

export VULKAN_SDK="${VULKAN_SDK:-C:/VulkanSDK/1.4.363.0}"
export LIBCLANG_PATH="${LIBCLANG_PATH:-C:/Program Files/Microsoft Visual Studio/18/Community/VC/Tools/Llvm/x64/bin}"
cd "$repo_root"
cargo build --release -q -p mediaa
cargo build --release -q -p mediaa-platform --example snap

MEDIAA_DEV_LIBRARY="$library" MEDIAA_DEV_OPEN="$open" ./target/release/mediaa.exe ${page:+--page="$page"} &
pid=$!
# Only our own process is ever closed.
trap 'kill "$pid" 2>/dev/null || true' EXIT
sleep "$wait"
# Git Bash gives its own pid; the window belongs to the Windows one (read once the app runs).
winpid="$pid"
[[ ! -r "/proc/$pid/winpid" ]] || winpid="$(cat "/proc/$pid/winpid")"
./target/release/examples/snap.exe "$out" "pid:$winpid"
