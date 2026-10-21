#!/usr/bin/env bash
# Runs one task with a headless agent. DeepSeek and OpenRouter go through Claude Code;
# Grok goes through the Grok CLI (`grok`). The lead calls this and reviews the result.
# A run lives in target/subagents/<name>/ so that long runs can be watched and continued:
#
#   scripts/subagent.sh run    [--provider deepseek|openrouter|grok] [--model ID] [--effort LEVEL] [--max-turns N] [--name TITLE] TASK_FILE
#   scripts/subagent.sh resume RUN_DIR MESSAGE_FILE [--max-turns N]
#   scripts/subagent.sh status RUN_DIR
#   scripts/subagent.sh list             # every run: running / finished / gone (crashed)
#   scripts/subagent.sh ... -            # the task from stdin
#
# `run` is the default command: it prints the run directory first, streams the agent into
# RUN_DIR/stream.jsonl and prints a summary at the end. `resume` sends another message to the
# same session; `status` reports progress without starting anything.
# Keys (never in the repository): OPENROUTER_API_KEY / DEEPSEEK_API_KEY, else
# ~/.secrets/openrouter.key / ~/.secrets/deepseek.key. Grok uses the logged-in `grok` CLI.
# Claude Code's `total_cost_usd` is computed with Anthropic prices and is wrong for DeepSeek:
# see cost-* (OpenRouter prices). Grok's cost-* is xAI's estimate from `grok usage`
# (costUsdTicks); the user pays by subscription, not that amount.
# Grok always runs `-m grok-4.7` (resolves to grok-4.7-build). Never the fast model.
set -euo pipefail

# The repository root is the directory above this script; runs go under target/.
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

model=""
base=""
key=""
effort=""
max_turns=200
run_dir=""
agent_pid=""

usage() { sed -n '2,/^set -euo pipefail$/p' "$0" | sed '$d'; }
usage_error() { usage >&2; exit 2; }

key_of() { # env var, key file
    local value="${!1:-}"
    if [[ -z "$value" && -f "$2" ]]; then value="$(cat "$2")"; fi
    [[ -n "$value" ]] || { echo "no key: set $1 or put it in $2" >&2; exit 2; }
    printf '%s' "$value"
}

setup_provider() { # provider, model override; sets base, key, model
    case "$1" in
        openrouter)
            base="https://openrouter.ai/api"
            key="$(key_of OPENROUTER_API_KEY "$HOME/.secrets/openrouter.key")"
            model="${2:-deepseek/deepseek-v4.1-flash-1017}"
            ;;
        deepseek)
            base="https://api.deepseek.com/anthropic"
            key="$(key_of DEEPSEEK_API_KEY "$HOME/.secrets/deepseek.key")"
            # DeepSeek's own name of V4.1 Flash (GET https://api.deepseek.com/models lists them).
            model="${2:-deepseek-flash}"
            ;;
        grok)
            # The flag is always grok-4.7. It resolves to grok-4.7-build. The fast model is refused.
            if [[ -n "$2" && "$2" != "grok-4.7" ]]; then
                echo "grok provider always uses -m grok-4.7 (got $2). Never the fast model." >&2
                exit 2
            fi
            model="grok-4.7"
            if [[ -z "$effort" ]]; then effort="medium"; fi
            case "$effort" in
                none|minimal|low|medium|high|xhigh|max) ;;
                *) echo "unknown effort: $effort (none|minimal|low|medium|high|xhigh|max)" >&2; exit 2 ;;
            esac
            ;;
        *) echo "unknown provider: $1 (openrouter, deepseek or grok)" >&2; exit 2 ;;
    esac
}

# A Windows grok.exe does not understand a Git Bash /c/... path.
to_win() {
    if command -v cygpath >/dev/null 2>&1; then cygpath -w "$1"; else printf '%s\n' "$1"; fi
}

prepare_env() {
    # The project's build needs these (llama.cpp with Vulkan, bindgen).
    export VULKAN_SDK="${VULKAN_SDK:-C:/VulkanSDK/1.4.363.0}"
    export LIBCLANG_PATH="${LIBCLANG_PATH:-C:/Program Files/Microsoft Visual Studio/18/Community/VC/Tools/Llvm/x64/bin}"

    # A clean Claude environment: when started from Claude Code (the desktop app, a session), the
    # inherited variables make the child expect the host's sign-in and send no key at all.
    local var
    while read -r var; do unset "$var"; done < <(compgen -e | grep -E '^(CLAUDE|ANTHROPIC)')
}

# One name for everything of a run — the run folder, the log subagents/<id>.md and the artifacts
# subagents/artifacts/<id>/: <date_time>_<model>_<task>, made by subagent-log.mjs (same rules).
make_run_dir() { # title
    local id n=2
    id="$(node "$repo_root/scripts/subagent-log.mjs" id "$model" "$1")"
    run_dir="$repo_root/target/subagents/$id"
    while [[ -e "$run_dir" ]]; do run_dir="$repo_root/target/subagents/$id-$n"; n=$((n + 1)); done
    mkdir -p "$run_dir"
    printf '%s\n' "$run_dir"
}

start_claude() { # prompt, session id (empty for a new session), run dir
    local prompt="$1" session="$2" dir="$3"
    local -a args=(-p "$prompt")
    [[ -z "${SUBAGENT_DEBUG:-}" ]] || args+=(--debug)
    [[ -z "$session" ]] || args+=(--resume "$session")
    args+=(--output-format stream-json --verbose
        --max-turns "$max_turns"
        --permission-mode acceptEdits
        --allowedTools "Read" "Edit" "Write" "Glob" "Grep"
            "Bash(cargo:*)" "Bash(ls:*)" "Bash(cat:*)" "Bash(head:*)" "Bash(tail:*)" "Bash(wc:*)"
            "Bash(grep:*)" "Bash(find:*)" "Bash(mkdir:*)" "Bash(git diff:*)" "Bash(git status:*)"
            "WebFetch" "mcp__integrated-browser-mcp" "mcp__debugmcp" "mcp__desktop-mcp"
            "Bash(scripts/ui-snap.sh:*)" "Bash(bash scripts/ui-snap.sh:*)" "Bash(node:*)"
        --disallowedTools "Bash(git commit:*)" "Bash(git add:*)" "Bash(git reset:*)" "Bash(git push:*)"
            "Bash(git stash:*)" "Bash(git checkout:*)" "Bash(git restore:*)" "Bash(git rm:*)"
            "Bash(cargo run:*)" "mcp__claude-in-chrome")

    # Every model alias points at the chosen model, so nothing falls back to Anthropic's.
    ANTHROPIC_BASE_URL="$base" \
    ANTHROPIC_AUTH_TOKEN="$key" \
    ANTHROPIC_API_KEY="" \
    ANTHROPIC_MODEL="$model" \
    ANTHROPIC_SMALL_FAST_MODEL="$model" \
    ANTHROPIC_DEFAULT_OPUS_MODEL="$model" \
    ANTHROPIC_DEFAULT_SONNET_MODEL="$model" \
    ANTHROPIC_DEFAULT_HAIKU_MODEL="$model" \
    CLAUDE_CODE_SUBAGENT_MODEL="$model" \
    ANTHROPIC_CUSTOM_MODEL_OPTION="$model" \
    CLAUDE_CODE_ALLOW_OTHER_MODELS=1 \
    claude "${args[@]}" \
        >> "$dir/stream.jsonl" 2>> "$dir/stderr.log" &
    agent_pid=$!
    record_pid "$dir" "$agent_pid"
}

# Headless grok cancels the whole run on any approval prompt, so this is always-approve.
# The deny rules still apply and refuse the command; the run continues.
start_grok() { # prompt file, session id (empty for a new session), run dir
    local prompt_file="$1" session="$2" dir="$3"
    local saved
    saved="$(cat "$dir/effort" 2>/dev/null || echo "${effort:-medium}")"
    local -a args=(
        --cwd "$(to_win "$repo_root")"
        -m grok-4.7
        --effort "$saved"
        --max-turns "$max_turns"
        --output-format streaming-json
        # Background commands grok started must not keep the run (or fail it) after its last turn.
        --no-wait-for-background
        # A headless grok cancels the whole run on any call that would need approval (dontAsk does
        # not help), so it runs with always-approve and the deny rules (they still apply) fence it.
        --permission-mode bypassPermissions
        --disallowed-tools Agent
        --deny "Bash(git commit*)" --deny "Bash(git add*)" --deny "Bash(git reset*)"
        --deny "Bash(git push*)" --deny "Bash(git stash*)" --deny "Bash(git checkout*)"
        --deny "Bash(git restore*)" --deny "Bash(git rm*)" --deny "Bash(git clean*)"
        --deny "Bash(git rebase*)" --deny "Bash(git merge*)" --deny "Bash(git switch*)"
        --deny "Bash(cargo run*)" --deny "Bash(cargo install*)"
        --deny "Bash(rm *)" --deny "Bash(rmdir*)" --deny "Bash(del *)" --deny "Bash(Remove-Item*)"
        --deny "Bash(kill*)" --deny "Bash(taskkill*)" --deny "Bash(Stop-Process*)"
        --deny "Bash(npm *)" --deny "Bash(npx *)" --deny "Bash(pip *)" --deny "Bash(winget*)"
        --deny "Bash(choco*)" --deny "Bash(scoop*)" --deny "Bash(Set-ExecutionPolicy*)"
        --deny "Bash(reg *)" --deny "Bash(shutdown*)" --deny "Bash(Restart-Computer*)"
        --deny "Bash(grok -p*)" --deny "Bash(grok --prompt*)" --deny "Bash(claude*)"
        --prompt-file "$(to_win "$prompt_file")"
    )
    [[ -z "$session" ]] || args+=(--resume "$session")
    grok "${args[@]}" \
        >> "$dir/stream.jsonl" 2>> "$dir/stderr.log" &
    agent_pid=$!
    record_pid "$dir" "$agent_pid"
}

record_pid() { # run dir, child pid
    printf '%s\n' "$2" > "$1/pid"
    printf '%s\n' "$$" > "$1/script_pid"
    sleep 1
    [[ ! -r "/proc/$2/winpid" ]] || cat "/proc/$2/winpid" > "$1/winpid"
}

# The readable dialog in subagents/ (scripts/subagent-log.mjs), kept up to date while it runs.
follow_log() { # run dir
    command -v node >/dev/null 2>&1 || return 0
    node "$repo_root/scripts/subagent-log.mjs" follow "$1" >/dev/null 2>&1 &
    printf '%s\n' "$!" > "$1/log_pid"
}

# The session id as soon as claude reports it (for `resume` even if everything else dies).
wait_session() { # run dir
    local i sid
    for i in $(seq 1 60); do
        sid="$(grep -m 1 -oE '"(session_id|sessionId)":"[^"]+"' "$1/stream.jsonl" 2>/dev/null | head -n 1 | sed -E 's/^"[^"]+":"([^"]+)"$/\1/' || true)"
        if [[ -n "$sid" ]]; then printf '%s\n' "$sid" > "$1/session_id"; return 0; fi
        sleep 1
    done
}

# One row per run in subagents/registry.md, written at start: what runs, as which processes.
register_run() { # run dir
    local dir="$1" reg="$repo_root/subagents/registry.md" r
    r() { cat "$dir/$1" 2>/dev/null || true; }
    mkdir -p "$repo_root/subagents"
    if [[ ! -f "$reg" ]]; then
        printf '%s\n' "# Запуски подагентов" "" \
            "Пишется при старте каждого запуска. Проверить, кто жив: \`scripts/subagent.sh list\`." "" \
            "| Начало | Задача | Модель | Процессы | Сессия | Папка запуска |" "|---|---|---|---|---|---|" > "$reg"
    fi
    local bin="claude"
    [[ "$(r provider)" == grok ]] && bin="grok"
    printf '| %s | %s | %s | %s %s (Windows %s), скрипт %s | `%s` | `%s` |\n' \
        "$(date '+%Y-%m-%d %H:%M:%S')" "$(r name | tr '|' '/')" "$(r model)" "$bin" "$(r pid)" "$(r winpid)" "$(r script_pid)" \
        "$(r session_id)" "${dir#"$repo_root"/}" >> "$reg"
}

# What every subagent is told on top of its task: where to put the evidence of its checks.
artifacts_note() { # artifacts dir (relative to the repository)
    cat <<EOF


## Artifacts (always)
Put everything that shows you checked your work into \`$1/\` (create it):
* \`report.md\` — what you did, how you checked it (linking the files next to it), decisions you
  made and why (especially where you departed from the task), open questions;
* evidence: screenshots, the relevant part of test/clippy/command output, measurements.
Ways to take screenshots:
* the app's own window — the ONLY way to run the app: \`scripts/ui-snap.sh $1/<name>.png
  [--page NAME] [--open NAME_PART]\` (builds, starts a copy on a test library, shoots only that
  copy's window, closes it). Never start the app yourself;
* a web page: the VS Code browser — open your own tab (\`browser_tab_open\`) and pass its tabId;
  \`browser_screenshot\` returns the image but does not save it;
* to save a screenshot of something on screen: \`mcp__desktop-mcp__capture_screenshot\` with a
  \`region\` around what you check and \`savePath\` in the artifacts folder — never the whole
  screen, other windows are private.

## When something is refused
If a command or tool is refused (needs approval, not allowed), do NOT look for a way around it
(other commands, other ways to start the app, disabling the sandbox). Stop that step and say in
the report what was refused.
EOF
}

extract_result() { # run dir: session id and the last result line
    local dir="$1"
    local f="$dir/stream.jsonl" line sid
    [[ -f "$f" ]] || return 0

    sid="$(grep -m 1 -oE '"(session_id|sessionId)":"[^"]+"' "$f" 2>/dev/null | head -n 1 | sed -E 's/^"[^"]+":"([^"]+)"$/\1/' || true)"
    [[ -z "$sid" ]] || printf '%s\n' "$sid" > "$dir/session_id"

    line="$(grep -E '"type"[[:space:]]*:[[:space:]]*"result"' "$f" 2>/dev/null | tail -n 1 || true)"
    [[ -z "$line" ]] || printf '%s\n' "$line" > "$dir/result.json"
}

# JSON readers for when jq is missing: grep/sed, good enough for these fields.
json_str() { # file, key
    sed -n "s/.*\"$2\"[[:space:]]*:[[:space:]]*\"\(.*\)\".*/\1/p" "$1" 2>/dev/null | head -n 1 |
        sed 's/\\"/\x01/g; s/","[a-zA-Z_]*":.*//; s/\x01/"/g; s/\\n/\n/g; s/\\t/\t/g; s/\\\\/\\/g' || true
}

json_num() { # file, key
    sed -n "s/.*\"$2\"[[:space:]]*:[[:space:]]*\([0-9][0-9]*\).*/\1/p" "$1" 2>/dev/null | head -n 1 || true
}

print_usage_notes() { # result.json
    local notes="${1%/result.json}/usage-notes"
    [[ -f "$notes" ]] && cat "$notes"
}

print_summary() { # result.json
    local f="$1"
    if [[ ! -f "$f" ]]; then echo "(no result)"; print_usage_notes "$f"; return 0; fi
    if command -v jq >/dev/null 2>&1; then
        # One line: a multiline program breaks when the script has CR characters.
        if jq -r '"subtype: \(.subtype // "?")\nnum_turns: \(.num_turns // "?")\nduration_ms: \(.duration_ms // "?")\nsession_id: \(.session_id // "?")" + (if .cost_note != null and .cost_note != "" then "\nmodel: \(.model // "?")\ntokens: in \(.usage.input_tokens // 0) out \(.usage.output_tokens // 0) reasoning \(.usage.reasoning_tokens // 0)\ncost: $\(.cost_usd) (\(.cost_note))" else "" end) + (if .fast_warning != null and .fast_warning != "" then "\n" + .fast_warning else "" end) + "\nresult:\n" + (.result // "")' "$f"; then
            print_usage_notes "$f"
            return 0
        fi
    fi
    echo "subtype: $(json_str "$f" subtype)"
    echo "num_turns: $(json_num "$f" num_turns)"
    echo "duration_ms: $(json_num "$f" duration_ms)"
    echo "session_id: $(json_str "$f" session_id)"
    if [[ -n "$(json_str "$f" cost_note)" ]]; then
        echo "model: $(json_str "$f" model)"
        echo "tokens: in $(json_num "$f" input_tokens) out $(json_num "$f" output_tokens) reasoning $(json_num "$f" reasoning_tokens)"
        echo "cost: \$$(json_str "$f" cost_usd) ($(json_str "$f" cost_note))"
    fi
    if [[ -s "${f%/result.json}/fast-model" ]]; then cat "${f%/result.json}/fast-model"; fi
    print_usage_notes "$f"
    echo "result:"
    json_str "$f" result
}

# One meter reading. A failure is a note later, never a failed run.
# Grok runs record the subscription %; DeepSeek runs record the balance.
# The lead's own snapshot (no --only) records both, plus Claude limits.
usage_snapshot() { # run dir, start|end
    local dir="$1" kind="$2" name out only
    name="$(head -n 1 "$dir/name" 2>/dev/null | tr -d '\r' || true)"
    [[ -n "$name" ]] || name="$(basename "$dir")"
    case "$(tr -d '\r' < "$dir/provider" 2>/dev/null || true)" in
        grok) only=grok ;;
        deepseek) only=deepseek ;;
        *) return 0 ;;
    esac
    out="$(node "$repo_root/scripts/usage.mjs" snapshot --label "$kind $name" --only "$only" 2>>"$dir/stderr.log" || true)"
    if [[ -n "$out" ]]; then
        printf '%s\n' "$out" | tail -n 1 > "$dir/usage-$kind.json" || true
    else
        printf '%s\n' '{"ok":false,"error":"snapshot produced no output"}' > "$dir/usage-$kind.json" || true
    fi
    return 0
}

append_usage_note() { # run dir
    local dir="$1" line
    line="$(node "$repo_root/scripts/usage.mjs" note "$dir" 2>>"$dir/stderr.log" || true)"
    if [[ -n "$line" ]]; then
        printf '%s\n' "$line" >> "$dir/usage-notes" || true
    else
        printf '%s\n' "usage snapshot: not recorded" >> "$dir/usage-notes" || true
    fi
    return 0
}

pid_alive() { # pid from $!
    [[ -n "$1" ]] || return 1
    kill -0 "$1" 2>/dev/null && return 0
    local winpid="$1" out
    if [[ -r "/proc/$1/winpid" ]]; then winpid="$(cat "/proc/$1/winpid")"; fi
    [[ -n "$winpid" ]] || return 1
    command -v tasklist >/dev/null 2>&1 || return 1
    out="$(tasklist //FI "PID eq $winpid" 2>/dev/null || true)"
    [[ "$out" == *"$winpid"* ]]
}

last_tool() { # stream.jsonl
    local line tail name input
    line="$(grep -E '"type"[[:space:]]*:[[:space:]]*"tool_use"|"type"[[:space:]]*:[[:space:]]*"tool_call"|"sessionUpdate"[[:space:]]*:[[:space:]]*"tool_call"' "$1" 2>/dev/null | tail -n 1 || true)"
    if [[ -z "$line" ]]; then echo "last tool: (none)"; return 0; fi
    if command -v jq >/dev/null 2>&1; then
        if printf '%s\n' "$line" | jq -r '
            if .params.update.sessionUpdate == "tool_call" then
                "last tool: \(.params.update.title // .params.update._meta["x.ai/tool"].name // "tool"): \(.params.update.rawInput | tostring | .[0:200])"
            elif .type == "tool_call" then
                "last tool: \(.toolName // .title // "tool"): \(.rawInput | tostring | .[0:200])"
            else
                ([.message.content[]? | select(.type == "tool_use")] | last | "last tool: \(.name): \(.input | tostring | .[0:200])")
            end
        '; then
            return 0
        fi
    fi
    tail="$(printf '%s\n' "$line" | sed 's/.*"type"[[:space:]]*:[[:space:]]*"tool_use"//')"
    name="$(printf '%s\n' "$tail" | sed -n 's/.*"name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')"
    input="$(printf '%s\n' "$tail" | sed -n 's/.*"input"[[:space:]]*:[[:space:]]*//p')"
    printf 'last tool: %s: %s\n' "$name" "${input:0:200}"
}

# --- What a run changed and what it cost ------------------------------------------------------

# Every file git would see (tracked and untracked, not ignored) with its hash.
snapshot() {
    (cd "$repo_root" && git ls-files -c -o --exclude-standard -z | xargs -0 -r sha1sum) 2>/dev/null || true
}

# Money spent so far by this key (USD): OpenRouter's usage counter, DeepSeek's balance (negated).
spent() { # run dir (its provider file)
    local json
    case "$(cat "$1/provider" 2>/dev/null)" in
        openrouter)
            json="$(curl -s -m 20 -H "Authorization: Bearer $key" https://openrouter.ai/api/v1/key || true)"
            printf '%s' "$json" | grep -o '"usage":[0-9.eE+-]*' | head -n 1 | cut -d: -f2 ;;
        deepseek)
            json="$(curl -s -m 20 -H "Authorization: Bearer $key" https://api.deepseek.com/user/balance || true)"
            printf '%s' "$json" | grep -o '"total_balance":"[0-9.]*"' | head -n 1 | grep -o '[0-9.]*' | awk '{print -$1}' ;;
    esac
}

# USD of the last claude call of a run: its usage × the model's OpenRouter prices (empty if unknown).
token_cost() { # run dir
    command -v node >/dev/null 2>&1 || return 0
    curl -s -m 20 https://openrouter.ai/api/v1/models 2>/dev/null | node -e '
        const fs = require("fs");
        const [dir] = process.argv.slice(1);
        let models, result;
        try { models = JSON.parse(fs.readFileSync(0, "utf8")).data; result = JSON.parse(fs.readFileSync(dir + "/result.json", "utf8")); } catch { process.exit(0); }
        // Model names of the DeepSeek API, priced as the same model in the OpenRouter list.
        const priced = { "deepseek-flash": "deepseek/deepseek-v4.1-flash", "deepseek-v4-pro": "deepseek/deepseek-v4-pro" };
        const own = fs.readFileSync(dir + "/model", "utf8").trim();
        const name = priced[own] || own;
        const m = models.find((x) => x.id === name || x.canonical_slug === name);
        const u = result.usage || {};
        if (!m) process.exit(0);
        const p = (k) => Number(m.pricing?.[k] || 0);
        const usd = (u.input_tokens || 0) * p("prompt") + (u.cache_read_input_tokens || 0) * (p("input_cache_read") || p("prompt"))
            + (u.cache_creation_input_tokens || 0) * (p("input_cache_write") || p("prompt")) + (u.output_tokens || 0) * p("completion");
        console.log(usd.toFixed(4));
    ' "$1" 2>/dev/null || true
}

begin_accounting() { # run dir, label
    local rev
    # The state before: a commit of the working tree (nothing in git changes), else HEAD.
    rev="$(cd "$repo_root" && git stash create 2>/dev/null || true)"
    [[ -n "$rev" ]] || rev="$(cd "$repo_root" && git rev-parse HEAD)"
    printf '%s\n' "$rev" > "$1/before-$2.rev"
    snapshot > "$1/files-before-$2"
    spent "$1" > "$1/spent-before-$2" || true
}

# changes-<label>.patch (also into the artifacts) and cost-<label>; prints a line about both.
end_accounting() { # run dir, label
    local dir="$1" label="$2" rev patch art before after n cost
    rev="$(cat "$dir/before-$label.rev")"
    snapshot > "$dir/files-after-$label"
    patch="$dir/changes-$label.patch"
    {
        echo "# Files changed while this run worked (a run in parallel would show here too)."
        echo "# Base: $rev"
        awk '
            { path = substr($0, 43); sub(/^\\/, "", path) }
            NR == FNR { before[path] = $1; next }
            { after[path] = $1; if (!(path in before)) print "A\t" path; else if (before[path] != $1) print "M\t" path }
            END { for (p in before) if (!(p in after)) print "D\t" p }
        ' "$dir/files-before-$label" "$dir/files-after-$label" | sort -k2 | while IFS=$'\t' read -r kind path; do
            echo "# $kind $path"
            if [[ "$kind" == A ]]; then
                (cd "$repo_root" && git diff --no-color --no-index /dev/null "$path" || true)
            elif (cd "$repo_root" && git cat-file -e "$rev:$path" 2>/dev/null); then
                (cd "$repo_root" && git diff --no-color "$rev" -- "$path" || true)
            else
                echo "# (was untracked before the run: no base to diff against)"
            fi
        done
    } > "$patch"
    n="$(grep -c '^# [AMD] ' "$patch" || true)"
    art="$repo_root/$(cat "$dir/artifacts" 2>/dev/null || echo "subagents/artifacts/$(basename "$dir")")"
    mkdir -p "$art" && cp "$patch" "$art/"
    local note=""
    if [[ "$(cat "$dir/provider" 2>/dev/null)" == grok ]]; then
        # grok usage is the whole session. cost-* stays this invocation, so list's sum matches the session.
        grok_settle "$dir" || true
        cost="$(grok_invocation_cost "$dir")"
        note=" (xAI's estimate; the user pays by subscription)"
    else
        # The run's tokens (its result line) × the model's prices from OpenRouter's public list: exact
        # at once; the key's spending counter lags by minutes, so it is only the fallback.
        cost="$(token_cost "$dir")"
        if [[ -z "$cost" ]]; then
            before="$(cat "$dir/spent-before-$label" 2>/dev/null || true)"
            after="$(spent "$dir" || true)"
            cost="?"
            if [[ -n "$before" && -n "$after" ]]; then cost="$(awk -v a="$after" -v b="$before" 'BEGIN { printf "%.4f", a - b }')"; fi
        fi
    fi
    printf '%s\n' "$cost" > "$dir/cost-$label"
    printf 'changed files: %s (changes-%s.patch) · cost: $%s%s\n' "$n" "$label" "$cost" "$note"
}

# Asks `grok usage` and writes result.json. The model check lives in subagent-log.mjs (grok-facts).
grok_settle() { # run dir
    local dir="$1" sid
    sid="$(cat "$dir/session_id" 2>/dev/null || true)"
    if [[ -n "$sid" ]]; then
        grok usage "$sid" > "$dir/usage.json" 2>> "$dir/stderr.log" || true
    fi
    node "$repo_root/scripts/subagent-log.mjs" grok-facts "$dir"
}

# USD of this invocation: session total minus cost-* already on disk, else this turn's ticks.
grok_invocation_cost() { # run dir
    local dir="$1" mode session turn prev
    mode="$(cat "$dir/grok-cost-mode" 2>/dev/null || echo unknown)"
    session="$(cat "$dir/grok-usd-session" 2>/dev/null || true)"
    turn="$(cat "$dir/grok-usd-turn" 2>/dev/null || true)"
    if [[ "$mode" == session && -n "$session" ]]; then
        prev="$(cat "$dir"/cost-* 2>/dev/null | awk '{s+=$1} END {printf "%.4f", s+0}' || true)"
        awk -v a="$session" -v b="${prev:-0}" 'BEGIN { d=a-b; if (d<0) d=0; printf "%.4f", d }'
    elif [[ -n "$turn" ]]; then
        awk -v a="$turn" 'BEGIN { printf "%.4f", a+0 }'
    else
        printf '?'
    fi
}

cmd_run() {
    local -a rest=()
    local prov=deepseek override="" name="" task="" rc=0
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --provider) [[ $# -ge 2 ]] || usage_error; prov="$2"; shift 2 ;;
            --model) [[ $# -ge 2 ]] || usage_error; override="$2"; shift 2 ;;
            --name) [[ $# -ge 2 ]] || usage_error; name="$2"; shift 2 ;;
            --effort) [[ $# -ge 2 ]] || usage_error; effort="$2"; shift 2 ;;
            --max-turns) [[ $# -ge 2 ]] || usage_error; max_turns="$2"; shift 2 ;;
            -h|--help) usage; exit 0 ;;
            *) rest+=("$1"); shift ;;
        esac
    done
    [[ ${#rest[@]} -eq 1 ]] || usage_error
    local task_file="${rest[0]}"
    [[ "$task_file" == "-" || -f "$task_file" ]] || { echo "no task file: $task_file" >&2; exit 2; }

    setup_provider "$prov" "$override"

    task="$(cat "$task_file")"
    # A title for the logs and the registry: --name, else the task's first line.
    [[ -n "$name" ]] || name="$(printf '%s\n' "$task" | grep -m 1 . | cut -c1-80)"
    make_run_dir "$name" # prints the run directory as the first line of stdout
    local artifacts="subagents/artifacts/$(basename "$run_dir")"
    task="$task$(artifacts_note "$artifacts")"
    printf '%s\n' "$name" > "$run_dir/name"
    printf '%s\n' "$artifacts" > "$run_dir/artifacts"
    printf '%s\n' "$task" > "$run_dir/task.md"
    printf '%s\n' "$prov" > "$run_dir/provider"
    printf '%s\n' "$model" > "$run_dir/model"
    [[ "$prov" != grok ]] || printf '%s\n' "$effort" > "$run_dir/effort"
    date +%s > "$run_dir/started"

    begin_accounting "$run_dir" run
    usage_snapshot "$run_dir" start || true
    prepare_env
    # follow after record_pid (inside start_*): the pid file must already be this process,
    # otherwise a previous turn's `end` looks like the run is over and the follower exits.
    if [[ "$prov" == grok ]]; then start_grok "$run_dir/task.md" "" "$run_dir"; else start_claude "$task" "" "$run_dir"; fi
    follow_log "$run_dir"
    wait_session "$run_dir"
    register_run "$run_dir"
    wait "$agent_pid" || rc=$?
    finish_agent "$run_dir" run "$rc"
}

cmd_resume() {
    local -a rest=()
    local dir msg_file msg sid rc=0
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --max-turns) [[ $# -ge 2 ]] || usage_error; max_turns="$2"; shift 2 ;;
            -h|--help) usage; exit 0 ;;
            *) rest+=("$1"); shift ;;
        esac
    done
    [[ ${#rest[@]} -eq 2 ]] || usage_error
    dir="${rest[0]}"
    msg_file="${rest[1]}"
    [[ -f "$dir/provider" && -f "$dir/model" ]] || { echo "not a run directory: $dir" >&2; exit 2; }
    [[ -f "$dir/session_id" ]] || { echo "no session_id in $dir: the run never started" >&2; exit 2; }
    [[ -f "$msg_file" ]] || { echo "no message file: $msg_file" >&2; exit 2; }
    sid="$(cat "$dir/session_id")"
    msg="$(cat "$msg_file")"

    local label prov
    prov="$(cat "$dir/provider")"
    label="resume-$(date +%Y%m%d-%H%M%S)"
    begin_accounting "$dir" "$label"
    usage_snapshot "$dir" start || true
    prepare_env
    if [[ "$prov" == grok ]]; then
        printf '%s\n' "$msg" > "$dir/pending-prompt.md"
        start_grok "$dir/pending-prompt.md" "$sid" "$dir"
    else
        setup_provider "$prov" "$(cat "$dir/model")"
        start_claude "$msg" "$sid" "$dir"
    fi
    follow_log "$dir"
    register_run "$dir"
    wait "$agent_pid" || rc=$?
    finish_agent "$dir" "$label" "$rc"
}

# Result, cost, the fast-model check, then the summary. A fast model fails the run.
finish_agent() { # run dir, accounting label, agent exit code
    local dir="$1" label="$2" rc="$3"
    extract_result "$dir"
    end_accounting "$dir" "$label"
    usage_snapshot "$dir" end || true
    append_usage_note "$dir" || true
    if [[ -s "$dir/fast-model" ]]; then
        cat "$dir/fast-model" >&2
        rc=1
    fi
    printf 'exit: %s\n' "$rc"
    print_summary "$dir/result.json"
    exit "$rc"
}

cmd_status() {
    [[ $# -eq 1 ]] || usage_error
    local dir="$1"
    local stream="$dir/stream.jsonl" pid started now elapsed mtime idle count
    [[ -d "$dir" ]] || { echo "no run directory: $dir" >&2; exit 2; }
    echo "run: $dir"

    if [[ -f "$dir/started" ]]; then
        started="$(cat "$dir/started")"
        now="$(date +%s)"
        elapsed=$(( now - started ))
        if (( elapsed < 0 )); then elapsed=0; fi
        printf 'elapsed: %dm %02ds\n' $((elapsed / 60)) $((elapsed % 60))
    else
        echo "elapsed: unknown (no started file)"
    fi

    if [[ -f "$dir/pid" ]]; then
        pid="$(cat "$dir/pid")"
        if pid_alive "$pid"; then echo "process: running (pid $pid)"; else echo "process: not running (pid $pid)"; fi
    else
        echo "process: no pid file"
    fi

    if [[ -f "$stream" ]]; then
        mtime="$(stat -c %Y "$stream" 2>/dev/null || date -r "$stream" +%s 2>/dev/null || echo 0)"
        if [[ "$mtime" != "0" ]]; then
            idle=$(( $(date +%s) - mtime ))
            if (( idle < 0 )); then idle=0; fi
            echo "stream idle: ${idle}s"
        else
            echo "stream idle: unknown"
        fi
        count="$(grep -cE '"type"[[:space:]]*:[[:space:]]*"assistant"|"type"[[:space:]]*:[[:space:]]*"text"|"sessionUpdate"[[:space:]]*:[[:space:]]*"agent_message_chunk"' "$stream" 2>/dev/null || true)"
        echo "assistant messages: ${count:-0}"
        last_tool "$stream"
    else
        echo "stream: not created yet"
    fi

    if [[ -f "$dir/result.json" ]]; then
        echo ""
        print_summary "$dir/result.json"
    fi
    return 0
}

# Every run: alive, finished or gone without a result (a crash: continue it with `resume`).
cmd_list() {
    local dir r state
    for dir in "$repo_root"/target/subagents/*/; do
        [[ -d "$dir" ]] || continue
        dir="${dir%/}"
        r() { cat "$dir/$1" 2>/dev/null || true; }
        if [[ -f "$dir/result.json" ]]; then
            state="finished: $(json_str "$dir/result.json" subtype)"
        elif [[ -f "$dir/pid" ]] && pid_alive "$(r pid)"; then
            state="running"
        else
            state="GONE without a result (resume it)"
        fi
        printf '%s | %s | %s | pid %s (Windows %s) | session %s | $%s | %s\n' "$(basename "$dir")" "$(r name)" "$(r model)" \
            "$(r pid)" "$(r winpid)" "$(r session_id)" "$(cat "$dir"/cost-* 2>/dev/null | awk '{s+=$1} END {printf "%.4f", s}')" "$state"
    done
}

if [[ $# -gt 0 ]]; then
    case "$1" in
        run|resume|status|list) cmd="$1"; shift ;;
        *) cmd=run ;;
    esac
else
    cmd=run
fi
case "$cmd" in
    run) cmd_run "$@" ;;
    resume) cmd_resume "$@" ;;
    status) cmd_status "$@" ;;
    list) cmd_list ;;
esac
