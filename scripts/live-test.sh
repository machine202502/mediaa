#!/usr/bin/env bash
set -euo pipefail
usage() {
    cat <<'USAGE'
Runs one ignored live test on the real models, so a subagent can do it with one allowed command.

  scripts/live-test.sh LOG PACKAGE FILTER [KEY=VALUE ...]

LOG: where the output goes (also printed at the end, last 60 lines). PACKAGE: e.g. mediaa-agent.
FILTER: the test name filter. KEY=VALUE: extra environment (MEDIAA_BENCH_*, RUST_LOG, ...).
Models: MEDIAA_TEST_MODELS defaults to the app's models folder.
USAGE
}
[[ $# -ge 3 ]] || { usage; exit 2; }
log="$1"; package="$2"; filter="$3"; shift 3
export VULKAN_SDK="${VULKAN_SDK:-C:/VulkanSDK/1.4.363.0}"
export LIBCLANG_PATH="${LIBCLANG_PATH:-C:/Program Files/Microsoft Visual Studio/18/Community/VC/Tools/Llvm/x64/bin}"
export MEDIAA_TEST_MODELS="${MEDIAA_TEST_MODELS:-C:/Users/wmachine/AppData/Local/MediaA/MediaA/data/models}"
for pair in "$@"; do export "$pair"; done
mkdir -p "$(dirname "$log")"
status=0
cargo test --release -q -p "$package" "$filter" -- --ignored --nocapture > "$log" 2>&1 || status=$?
tail -60 "$log"
exit "$status"
