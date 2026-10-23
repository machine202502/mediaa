#!/usr/bin/env node
// Mirrors the dialogs of subagents into readable Markdown files in `subagents/` (one per run),
// so the user can watch what each subagent does while it works:
// * Claude Code's own subagents (Sonnet, Haiku…): ~/.claude/projects/<project>/<session>/subagents/agent-*.jsonl
//   with agent-*.meta.json (task description, model);
// * runs of scripts/subagent.sh (DeepSeek, Grok…): target/subagents/<run>/stream.jsonl with task.md, model,
//   name (optional), result.json. Grok's stream is streaming-json (flat events or ACP session updates).
// * Grok's built-in subagents: ~/.grok/sessions/<cwd>/<parent>/subagents/**/meta.json points at the
//   child session; the dialog is that child's updates.jsonl. GROK_HOME overrides ~/.grok.
//
//   node scripts/subagent-log.mjs once            render every transcript once
//   node scripts/subagent-log.mjs watch           keep rendering what changes (every 3 s); one at a time
//   node scripts/subagent-log.mjs watch --detach  start that watcher in the background and return
//   node scripts/subagent-log.mjs follow RUN_DIR  render one scripts/subagent.sh run until its process exits
//
// A DeepSeek run is finished by its result line. A Grok stream writes `end` after every turn and a
// resume appends after that, so `end` alone does not finish the run: `follow` keeps rendering while
// the agent pid is alive. A Claude Code subagent is finished only by its SubagentHandback tool call
// (a quiet transcript alone may just be a long build).
// Finished runs (a result line: готово, ошибка, лимит ходов) whose transcript has not changed for
// `archiveAfterHours` hours move to subagents/archive/ — the log and its artifacts keep their layout,
// so the links inside the log stay valid. The limit comes from subagents/config.json (created with
// {"archiveAfterHours": 6} when missing; the env SUBAGENT_ARCHIVE_HOURS overrides it) and is read on
// every pass. A resumed run (its transcript changes) comes back to subagents/. registry.md never moves.
// `watch` keeps one watcher: its pid is in subagents/.watch.pid; `watch --detach` starts it in the
// background and returns at once (SessionStart hook).

import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

const repo = path.resolve(path.dirname(new URL(import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, "$1")), "..");
const outDir = path.join(repo, "subagents");
const archiveDir = path.join(outDir, "archive");
const indexFile = path.join(outDir, ".index.json");
const pidFile = path.join(outDir, ".watch.pid");
const configFile = path.join(outDir, "config.json");
/// Longest text kept of one thought, tool input or tool result.
const CUT = 2000;
/// A transcript untouched this long (and not finished explicitly) counts as stopped.
const IDLE_MS = 10 * 60 * 1000;

// ~/.claude/projects/<the project path with every non-alphanumeric character as '-'>
const projectKey = repo.replace(/[^A-Za-z0-9]/g, "-");
const claudeProject = path.join(os.homedir(), ".claude", "projects", projectKey);

function readJsonl(file) {
    const out = [];
    let text = "";
    try {
        text = fs.readFileSync(file, "utf8");
    } catch {
        return out;
    }
    for (const line of text.split("\n")) {
        if (!line.trim()) continue;
        try {
            out.push(JSON.parse(line));
        } catch {
            // A line still being written.
        }
    }
    return out;
}

const readText = (file) => {
    try {
        return fs.readFileSync(file, "utf8").trim();
    } catch {
        return "";
    }
};

/// A path with forward slashes, relative to `base` (for links inside a log).
const relTo = (base, abs) => path.relative(base, abs).replace(/\\/g, "/");
const cut = (s) => (s.length > CUT ? `${s.slice(0, CUT)}\n… (ещё ${s.length - CUT} символов)` : s);
const time = (iso) => (iso ? new Date(iso).toLocaleTimeString("ru-RU", { hour12: false }) : "");
const stamp = (d) => {
    const p = (n) => String(n).padStart(2, "0");
    return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}_${p(d.getHours())}-${p(d.getMinutes())}`;
};
const slug = (s) =>
    s
        .toLowerCase()
        .replace(/[^\p{L}\p{N}]+/gu, "-")
        .replace(/^-+|-+$/g, "")
        .slice(0, 50)
        .replace(/-+$/, "") || "task";
const shortModel = (m) => (m || "model").split("/").pop().replace(/[^A-Za-z0-9.-]+/g, "-");
const fence = (s) => {
    const n = Math.max(3, ...[...String(s).matchAll(/`+/g)].map((m) => m[0].length + 1));
    return "`".repeat(n);
};
const block = (s, lang = "") => {
    const f = fence(s);
    return `${f}${lang}\n${s}\n${f}`;
};

function resultText(content) {
    if (typeof content === "string") return content;
    if (Array.isArray(content)) return content.map((c) => (c.type === "text" ? c.text : `[${c.type}]`)).join("\n");
    return JSON.stringify(content);
}

function acpUpdate(e) {
    const u = e?.params?.update || (e?.sessionUpdate ? e : null);
    return u?.sessionUpdate ? u : null;
}

function isGrokEvents(events) {
    return events.some((e) => {
        const su = acpUpdate(e)?.sessionUpdate;
        return (
            su === "agent_message_chunk" ||
            su === "agent_thought_chunk" ||
            su === "tool_call" ||
            su === "tool_call_update" ||
            su === "turn_completed" ||
            e.type === "thought" ||
            e.type === "tool_call" ||
            e.type === "tool_call_update" ||
            (e.type === "end" && (e.sessionId || e.modelUsage))
        );
    });
}

function isGrok(src, events) {
    if (src?.provider === "grok") return true;
    if (src?.provider === "deepseek" || src?.provider === "openrouter") return false;
    return isGrokEvents(events);
}

/// Model names that actually ran: modelUsage keys, else the recorded model id.
function collectModels(events, usage) {
    const names = [];
    const add = (mu) => {
        if (!mu || typeof mu !== "object" || Array.isArray(mu)) return;
        for (const k of Object.keys(mu)) if (!names.includes(k)) names.push(k);
    };
    for (const e of events || []) {
        add(e.modelUsage);
        const u = acpUpdate(e);
        add(u?.usage?.modelUsage);
        add(u?.modelUsage);
    }
    add(usage?.session?.modelUsage);
    for (const t of usage?.turns || []) add(t.modelUsage);
    if (names.length) return names;
    const push = (id) => {
        if (typeof id === "string" && id && !names.includes(id)) names.push(id);
    };
    push(usage?.session?.primaryModelId);
    for (const t of usage?.turns || []) push(t.primaryModelId);
    for (const e of events || []) push(acpUpdate(e)?._meta?.modelId);
    return names;
}

function chunkText(e) {
    const u = acpUpdate(e);
    if (typeof e?.data === "string") return e.data;
    if (typeof e?.text === "string" && (e.type === "text" || e.type === "thought")) return e.text;
    const c = u?.content;
    if (typeof c === "string") return c;
    if (c && typeof c.text === "string") return c.text;
    if (Array.isArray(c)) return c.map((x) => x.text || "").join("");
    return "";
}

function isTerminal(e) {
    const su = acpUpdate(e)?.sessionUpdate;
    return e.type === "end" || e.type === "result" || e.type === "error" || su === "turn_completed";
}

function isToolCall(e) {
    return e.type === "tool_call" || acpUpdate(e)?.sessionUpdate === "tool_call";
}

function isAgentText(e) {
    return e.type === "text" || acpUpdate(e)?.sessionUpdate === "agent_message_chunk";
}

/// The last turn's answer: text after its last tool call.
function finalGrokText(events) {
    let prev = -1;
    let last = -1;
    events.forEach((e, i) => {
        if (isTerminal(e)) {
            prev = last;
            last = i;
        }
    });
    const slice = events.slice(prev + 1, last === -1 ? events.length : last);
    let lastTool = -1;
    slice.forEach((e, i) => {
        if (isToolCall(e)) lastTool = i;
    });
    return slice
        .filter((e, i) => i > lastTool && isAgentText(e))
        .map(chunkText)
        .join("");
}

function stopOf(e) {
    const u = acpUpdate(e);
    if (e?.type === "error") return "error";
    return String(e?.stopReason || e?.stop_reason || e?.subtype || u?.stop_reason || u?.stopReason || "");
}

function subtypeOf(stop, type) {
    const norm = String(stop || "").toLowerCase().replace(/-/g, "_");
    if (type === "error") return "error";
    if (/max_turn/.test(norm) || norm === "error_max_turns") return "error_max_turns";
    if (!norm || norm === "end_turn" || norm === "endturn" || norm === "success") return "success";
    if (norm === "refusal") return "error";
    return norm;
}

function turnsOf(e) {
    const u = acpUpdate(e);
    const n = e?.num_turns ?? e?.numTurns ?? u?.usage?.numTurns ?? e?.usage?.num_turns ?? e?.usage?.numTurns;
    return Number.isFinite(n) ? n : null;
}

function durationOf(e) {
    const u = acpUpdate(e);
    const n = e?.duration_ms ?? u?.elapsed_ms ?? u?.usage?.apiDurationMs;
    return Number.isFinite(n) ? n : 0;
}

function ticksOf(e) {
    if (!e) return null;
    const u = acpUpdate(e);
    for (const c of [u?.usage?.costUsdTicks, e.usage?.costUsdTicks, e.total_cost_usd_ticks, e.costUsdTicks]) {
        if (Number.isFinite(c)) return c;
    }
    return null;
}

function tokenFields(src) {
    if (!src || typeof src !== "object") return null;
    const input = src.inputTokens ?? src.input_tokens;
    const output = src.outputTokens ?? src.output_tokens;
    const reasoning = src.reasoningTokens ?? src.reasoning_tokens;
    const total = src.totalTokens ?? src.total_tokens;
    const calls = src.modelCalls ?? src.model_calls;
    if (![input, output, reasoning, total, calls].some((n) => Number.isFinite(n))) return null;
    return {
        input_tokens: input || 0,
        output_tokens: output || 0,
        reasoning_tokens: reasoning || 0,
        total_tokens: total || 0,
        model_calls: calls || 0,
    };
}

const FAST_WARNING = (names) =>
    [
        "!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!",
        `WARNING: FAST MODEL USED: ${names.join(", ")}`,
        "This burns the separate subscription limits.",
        "The run is failed on purpose. Tell the user. Do not continue with this model.",
        "!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!",
    ].join("\n");

/// Tokens, cost and the fast-model check for one Grok run. `usage` is `grok usage` JSON or null.
export function grokRunFacts(events, usage) {
    const models = collectModels(events, usage);
    const fast = models.filter((m) => /fast/i.test(m));
    const terminals = (events || []).filter(isTerminal);
    const last = terminals[terminals.length - 1] || null;
    const sessionTicks = usage?.session?.costUsdTicks;
    const sessionUsd = Number.isFinite(sessionTicks) ? sessionTicks / 1e10 : null;
    const turnTicks = ticksOf(last);
    const turnUsd = Number.isFinite(turnTicks) ? turnTicks / 1e10 : null;
    const sessionId =
        usage?.sessionId ||
        (events || []).map((e) => e.sessionId || e.session_id || e.params?.sessionId).filter(Boolean).pop() ||
        "";
    const tokens = tokenFields(usage?.session) || tokenFields(acpUpdate(last)?.usage) || tokenFields(last?.usage) || {};
    return {
        models,
        fast,
        sessionUsd,
        turnUsd,
        text: finalGrokText(events || []),
        subtype: last ? subtypeOf(stopOf(last), last.type) : "error",
        sessionId,
        tokens,
        numTurns: (last && turnsOf(last)) ?? usage?.session?.turnCount ?? terminals.length,
        durationMs: last ? durationOf(last) : 0,
        warning: fast.length ? FAST_WARNING(fast) : "",
    };
}

/// Grok ACP `timestamp` is unix seconds. Claude lines are ISO strings or milliseconds.
/// A bare `new Date(seconds)` lands in 1970 and names the log that way.
export function startedAt(value, fallbackMs) {
    if (typeof value === "number" && Number.isFinite(value)) return new Date(value < 1e12 ? value * 1000 : value);
    if (typeof value === "string" && value.trim()) {
        const d = new Date(value);
        if (!Number.isNaN(d.getTime())) return d;
    }
    return new Date(fallbackMs);
}

function whenOf(e) {
    const u = acpUpdate(e);
    const ms = e?._meta?.agentTimestampMs || e?.params?._meta?.agentTimestampMs || u?._meta?.agentTimestampMs;
    let iso = "";
    if (Number.isFinite(ms)) iso = new Date(ms).toISOString();
    else if (typeof e?.timestamp === "number") iso = startedAt(e.timestamp, NaN).toISOString();
    else if (typeof e?.timestamp === "string") iso = e.timestamp;
    const t = time(iso);
    return t && t !== "Invalid Date" ? t : "";
}

function grokToolName(e, u) {
    const tool = u?._meta?.["x.ai/tool"];
    return e.toolName || u?.toolName || tool?.name || u?.title || e.title || "tool";
}

function grokToolOutput(upd) {
    if (!upd) return "";
    if (upd.rawOutput != null) return typeof upd.rawOutput === "string" ? upd.rawOutput : JSON.stringify(upd.rawOutput, null, 2);
    const c = upd.content;
    if (typeof c === "string") return c;
    if (Array.isArray(c)) return c.map((x) => (typeof x === "string" ? x : x.text || x.content || JSON.stringify(x))).join("\n");
    return "";
}

/// Grok streaming-json (flat or ACP) as the same dialog Markdown DeepSeek runs use.
function renderGrokEvents(events) {
    const outputs = new Map();
    for (const e of events) {
        const u = acpUpdate(e);
        const upd = u?.sessionUpdate === "tool_call_update" ? u : e.type === "tool_call_update" ? e : null;
        if (!upd?.toolCallId) continue;
        const prev = outputs.get(upd.toolCallId) || {};
        outputs.set(upd.toolCallId, {
            ...prev,
            ...upd,
            rawOutput: upd.rawOutput != null ? upd.rawOutput : prev.rawOutput,
            status: upd.status || prev.status,
        });
    }
    const lines = [];
    let buf = [];
    let kind = "";
    let stampAt = "";
    let firstUser = true;
    const flush = () => {
        const s = buf.join("");
        const k = kind;
        const st = stampAt;
        buf = [];
        kind = "";
        stampAt = "";
        if (!s.trim()) return;
        const label = k === "thought" ? "думает" : "пишет";
        const head = st ? `### ${st} · ${label}` : `### ${label}`;
        lines.push(head, "", k === "thought" ? cut(s).replace(/^/gm, "> ") : s, "");
    };
    for (const e of events) {
        const u = acpUpdate(e);
        const su = u?.sessionUpdate;
        if (su === "agent_thought_chunk" || e.type === "thought") {
            if (kind !== "thought") flush();
            if (!buf.length) stampAt = whenOf(e);
            kind = "thought";
            buf.push(chunkText(e));
        } else if (su === "agent_message_chunk" || e.type === "text") {
            if (kind !== "text") flush();
            if (!buf.length) stampAt = whenOf(e);
            kind = "text";
            buf.push(chunkText(e));
        } else if (su === "user_message_chunk") {
            flush();
            const t = chunkText(e);
            if (firstUser) {
                firstUser = false;
                continue;
            }
            if (t.trim()) lines.push(whenOf(e) ? `### ${whenOf(e)} · сообщение подагенту` : "### сообщение подагенту", "", t, "");
        } else if (su === "tool_call" || e.type === "tool_call") {
            flush();
            const id = (u || e).toolCallId;
            const name = grokToolName(e, u);
            const input = (u || e).rawInput ?? {};
            lines.push(whenOf(e) ? `### ${whenOf(e)} · вызывает \`${name}\`` : `### вызывает \`${name}\``, "", block(cut(JSON.stringify(input, null, 2)), "json"), "");
            const upd = outputs.get(id);
            const out = grokToolOutput(upd);
            if (out) {
                const err = upd.status === "failed" || upd.status === "error";
                lines.push(`<details><summary>результат${err ? " (ошибка)" : ""}</summary>`, "", block(cut(out)), "", "</details>", "");
            }
        } else if (e.type === "error") {
            flush();
            lines.push(whenOf(e) ? `### ${whenOf(e)} · ошибка` : "### ошибка", "", e.message || "error", "");
        }
    }
    flush();
    return lines;
}

/// Dialog after the last turn ending: a later turn, or a resume appended after `end`.
function grokContinued(events) {
    let last = -1;
    for (let i = 0; i < events.length; i++) if (isTerminal(events[i])) last = i;
    if (last < 0) return false;
    for (let i = last + 1; i < events.length; i++) {
        const su = acpUpdate(events[i])?.sessionUpdate;
        const t = events[i].type;
        if (
            t === "thought" ||
            t === "text" ||
            t === "tool_call" ||
            t === "tool_call_update" ||
            su === "agent_thought_chunk" ||
            su === "agent_message_chunk" ||
            su === "tool_call" ||
            su === "tool_call_update"
        ) {
            return true;
        }
    }
    return false;
}

/// The agent process recorded in RUN_DIR/pid. Missing or dead is false; EPERM means it is alive.
function agentPidAlive(src) {
    if (!src?.dir) return false;
    const pid = Number(readText(path.join(src.dir, "pid")));
    if (!Number.isInteger(pid) || pid <= 0) return false;
    try {
        process.kill(pid, 0);
        return true;
    } catch (e) {
        return e.code === "EPERM";
    }
}

function grokState(src, events, mtime) {
    // `end` / turn_completed is one turn. The process keeps writing, and a resume appends after it.
    if (agentPidAlive(src) || !events.some(isTerminal) || grokContinued(events)) {
        const alive = agentPidAlive(src) || Date.now() - mtime < IDLE_MS;
        return { status: alive ? "работает" : "остановлен (журнал давно не менялся)", finished: false, summary: [] };
    }
    const facts = grokRunFacts(events, null);
    // A built-in subagent's summary names the model even when the stream has no modelUsage.
    if (src.model && !facts.models.includes(src.model)) facts.models.push(src.model);
    if (src.model && /fast/i.test(src.model) && !facts.fast.includes(src.model)) {
        facts.fast.push(src.model);
        facts.warning = FAST_WARNING(facts.fast);
    }
    const fast = facts.fast.length > 0;
    const maxTurns = facts.subtype === "error_max_turns";
    const ok = facts.subtype === "success" && !fast;
    const sec = Math.round((facts.durationMs || 0) / 1000);
    const status = fast ? "ошибка (fast model)" : ok ? "готово" : maxTurns ? "остановлен на лимите ходов" : `ошибка (${facts.subtype})`;
    // streaming-json `end` often has no wall-clock duration; don't print a fake 0s.
    const turns = facts.durationMs
        ? `Ходов: ${facts.numTurns ?? "?"}, время: ${Math.floor(sec / 60)} мин ${sec % 60} с.`
        : `Ходов: ${facts.numTurns ?? "?"}.`;
    return {
        status,
        finished: true,
        summary: [
            ...(facts.models.length ? [`Модель: ${facts.models.join(", ")}`] : []),
            turns,
            "",
            ...(fast ? [facts.warning, ""] : []),
            ...(maxTurns ? [...continueLines(src), ""] : []),
            facts.text || "",
        ],
    };
}

/// Markdown of a Grok transcript: model, dialog, result. Used by the tests.
export function renderGrokLog(events) {
    const src = { kind: "script", provider: "grok", model: "", dir: "", file: "stream.jsonl" };
    const st = state(src, events, Date.now());
    const model = collectModels(events, null).join(", ") || "?";
    return [
        `* **Модель:** ${model}`,
        `* **Статус:** ${st.status}`,
        "",
        "## Ход работы",
        "",
        ...renderEvents(events, src),
        ...(st.summary.length ? ["## Итог", "", ...st.summary, ""] : []),
    ].join("\n");
}

/// The dialog as Markdown lines: thoughts, texts, tool calls with their results.
function renderEvents(events, src) {
    if (isGrok(src, events)) return renderGrokEvents(events);
    return renderClaudeEvents(events);
}

function renderClaudeEvents(events) {
    const lines = [];
    let first = true;
    for (const e of events) {
        const content = e.message?.content;
        if (e.type === "user") {
            if (typeof content === "string") {
                // The task (first) or a later message to the subagent (a resume, a nudge).
                if (!first) lines.push(`### ${time(e.timestamp)} · сообщение подагенту`, "", content, "");
                first = false;
                continue;
            }
            for (const c of content || []) {
                if (c.type === "tool_result") {
                    lines.push(`<details><summary>результат${c.is_error ? " (ошибка)" : ""}</summary>`, "", block(cut(resultText(c.content))), "", "</details>", "");
                } else if (c.type === "text" && first) {
                    first = false;
                } else if (c.type === "text") {
                    lines.push(`### ${time(e.timestamp)} · сообщение подагенту`, "", c.text, "");
                }
            }
            first = false;
        } else if (e.type === "assistant") {
            for (const c of content || []) {
                if (c.type === "thinking" && c.thinking) lines.push(`### ${time(e.timestamp)} · думает`, "", cut(c.thinking).replace(/^/gm, "> "), "");
                if (c.type === "text" && c.text.trim()) lines.push(`### ${time(e.timestamp)} · пишет`, "", c.text, "");
                if (c.type === "tool_use") lines.push(`### ${time(e.timestamp)} · вызывает \`${c.name}\``, "", block(cut(JSON.stringify(c.input, null, 2)), "json"), "");
            }
        }
    }
    return lines;
}

/// The task: the first user message of the transcript.
function firstTask(events) {
    for (const e of events) {
        if (e.type !== "user") continue;
        const c = e.message?.content;
        if (typeof c === "string") return c;
        const t = (c || []).find((x) => x.type === "text");
        if (t) return t.text;
    }
    return "";
}

const grokHome = () => process.env.GROK_HOME || path.join(os.homedir(), ".grok");
const normPath = (p) => path.resolve(String(p || "")).replace(/[\\/]+$/, "").toLowerCase();

function readJson(file) {
    try {
        return JSON.parse(fs.readFileSync(file, "utf8"));
    } catch {
        return null;
    }
}

/// First user_message_chunk text (Grok sessions store the task there, not as a Claude user line).
function userChunkText(events) {
    for (const e of events) {
        if (acpUpdate(e)?.sessionUpdate !== "user_message_chunk") continue;
        const t = chunkText(e);
        if (t.trim()) return t;
    }
    return "";
}

/// Child sessions of Grok's built-in subagents whose parent or child cwd is this repo.
/// `meta.json` (SubagentMeta) lives under `<parent>/subagents/` and names `child_session_id`.
export function grokBuiltinSources(sessionsDir, repoPath) {
    const repoN = normPath(repoPath);
    const byId = new Map();
    let groups = [];
    try {
        groups = fs.readdirSync(sessionsDir, { withFileTypes: true });
    } catch {
        return [];
    }
    for (const group of groups) {
        if (!group.isDirectory()) continue;
        const groupDir = path.join(sessionsDir, group.name);
        let sessions = [];
        try {
            sessions = fs.readdirSync(groupDir, { withFileTypes: true });
        } catch {
            continue;
        }
        for (const session of sessions) {
            if (!session.isDirectory()) continue;
            const dir = path.join(groupDir, session.name);
            const updates = path.join(dir, "updates.jsonl");
            if (!fs.existsSync(updates)) continue;
            byId.set(session.name, { dir, updates });
        }
    }
    const out = [];
    const seen = new Set();
    for (const { dir } of byId.values()) {
        const sub = path.join(dir, "subagents");
        if (!fs.existsSync(sub)) continue;
        const parent = readJson(path.join(dir, "summary.json"));
        const parentCwd = parent?.info?.cwd || "";
        const metas = [];
        const walk = (folder, depth) => {
            if (depth > 3) return;
            let entries = [];
            try {
                entries = fs.readdirSync(folder, { withFileTypes: true });
            } catch {
                return;
            }
            for (const entry of entries) {
                const abs = path.join(folder, entry.name);
                if (entry.isDirectory()) walk(abs, depth + 1);
                else if (entry.name === "meta.json") metas.push(abs);
            }
        };
        walk(sub, 0);
        for (const metaPath of metas) {
            const meta = readJson(metaPath);
            const childId = meta?.child_session_id;
            if (!childId || seen.has(childId)) continue;
            const childCwd = meta.child_cwd || parentCwd;
            if (normPath(childCwd) !== repoN && normPath(parentCwd) !== repoN) continue;
            const child = byId.get(childId);
            if (!child) continue;
            seen.add(childId);
            const childSummary = readJson(path.join(child.dir, "summary.json"));
            out.push({
                key: `grok-builtin:${childId}`,
                file: child.updates,
                kind: "grok-builtin",
                title: meta.description || childSummary?.generated_title || "подагент Grok",
                model: meta.effective_model_id || childSummary?.current_model_id || "",
                task: typeof meta.prompt === "string" ? meta.prompt : "",
                sessionId: childId,
                parentSession: meta.parent_session_id || parent?.info?.id || "",
                subagentId: meta.subagent_id || "",
                agentType: meta.subagent_type || "",
                effort: childSummary?.reasoning_effort || "",
            });
        }
    }
    return out;
}

/// Every transcript: { key, file (the JSONL), title, model, task, started, status, summary }.
function sources() {
    const out = [];
    // Claude Code's subagents, every session of this project.
    if (fs.existsSync(claudeProject)) {
        for (const session of fs.readdirSync(claudeProject)) {
            const dir = path.join(claudeProject, session, "subagents");
            if (!fs.existsSync(dir)) continue;
            for (const f of fs.readdirSync(dir)) {
                if (!f.endsWith(".jsonl")) continue;
                const file = path.join(dir, f);
                let meta = {};
                try {
                    meta = JSON.parse(fs.readFileSync(file.replace(/\.jsonl$/, ".meta.json"), "utf8"));
                } catch {}
                out.push({ key: file, file, kind: "claude", title: meta.description, model: meta.model || "inherit" });
            }
        }
    }
    // Runs of scripts/subagent.sh.
    const runs = path.join(repo, "target", "subagents");
    if (fs.existsSync(runs)) {
        for (const r of fs.readdirSync(runs)) out.push(runSource(path.join(runs, r)));
    }
    // Grok's built-in subagents (child sessions), not the lead's own session.
    out.push(...grokBuiltinSources(path.join(grokHome(), "sessions"), repo));
    return out.filter((s) => s && fs.existsSync(s.file));
}

function runSource(dir) {
    const task = readText(path.join(dir, "task.md"));
    return {
        key: dir,
        dir,
        file: path.join(dir, "stream.jsonl"),
        kind: "script",
        provider: readText(path.join(dir, "provider")),
        title: readText(path.join(dir, "name")) || task.split("\n").find((l) => l.trim()) || path.basename(dir),
        model: readText(path.join(dir, "model")),
    };
}

/// A path a task wrote (relative, absolute Windows, backslashes) as repo-relative; "" when it is
/// not a path inside the repo or holds a placeholder (`<name>` and the like).
function repoRel(raw) {
    const v = String(raw || "").trim().replace(/\\/g, "/").replace(/[.,;:]+$/, "");
    if (!v || /[<>]/.test(v)) return "";
    const rel = relTo(repo, path.resolve(repo, v));
    if (!rel || rel.startsWith("..") || path.isAbsolute(rel) || /^[A-Za-z]:/.test(rel)) return "";
    return rel;
}

/// The run's artifacts folder, repo-relative, from the authoritative place for its kind:
/// scripts/subagent.sh writes it to RUN_DIR/artifacts (one line, repo-relative); a Claude Code
/// subagent names it in the task («Artifacts folder…: <path>»), else the first
/// `subagents/artifacts/<name>` mention counts.
function artifactFolder(src, task) {
    const folder = (p) => {
        const rel = repoRel(p);
        return /^subagents\/artifacts\/./.test(rel) ? rel : "";
    };
    if (src.kind === "script") return folder(readText(path.join(src.dir, "artifacts")));
    const line = String(task).match(/artifacts\s+folder[^\n:]*:[ \t]*([^\s`'")\]]+)/i);
    const named = line ? folder(line[1]) : "";
    if (named) return named;
    const mention = String(task).match(/subagents[\\/]artifacts[\\/][^\s`'")\\/]+/);
    return mention ? folder(mention[0]) : "";
}

/// How to continue a run that stopped at its turn limit.
function continueLines(src) {
    if (src.kind === "script") {
        const dir = relTo(repo, src.dir);
        return ["Продолжить в той же сессии:", "", `\`scripts/subagent.sh resume ${dir} MESSAGE_FILE --max-turns N\``];
    }
    if (src.kind === "grok-builtin") {
        return [`Встроенный подагент Grok остановился на лимите ходов. Продолжить ту же сессию: \`resume_from\` \`${src.subagentId || src.sessionId || "?"}\`.`];
    }
    return ["Подагент Claude Code остановился на лимите ходов: запустите его снова или продолжите работу над задачей сами."];
}

/// Running / done / error and the closing summary of a transcript.
function state(src, events, mtime) {
    if (isGrok(src, events)) return grokState(src, events, mtime);
    const result = [...events].reverse().find((e) => e.type === "result");
    if (result) {
        const ok = result.subtype === "success" && !result.is_error;
        const maxTurns = result.subtype === "error_max_turns";
        const sec = Math.round((result.duration_ms || 0) / 1000);
        const status = ok ? "готово" : maxTurns ? "остановлен на лимите ходов" : `ошибка (${result.subtype})`;
        const turns = `Ходов: ${result.num_turns ?? "?"}, время: ${Math.floor(sec / 60)} мин ${sec % 60} с.`;
        return { status, finished: true, summary: [turns, "", ...(maxTurns ? [...continueLines(src), ""] : []), result.result || ""] };
    }
    if (src.kind === "script") {
        const alive = Date.now() - mtime < IDLE_MS;
        return { status: alive ? "работает" : "остановлен (журнал давно не менялся)", finished: false, summary: [] };
    }
    // Claude Code's subagent: finished only by its SubagentHandback tool call (the final report).
    // A last text without tool calls means nothing — a long build looks exactly like that.
    for (const e of [...events].reverse()) {
        if (e.type !== "assistant") continue;
        const handback = (e.message?.content || []).find((c) => c.type === "tool_use" && c.name === "SubagentHandback");
        if (handback) return { status: "готово", finished: true, summary: [JSON.stringify(handback.input, null, 2)] };
    }
    return { status: Date.now() - mtime < IDLE_MS ? "работает" : "остановлен (журнал давно не менялся)", finished: false, summary: [] };
}

/// How long a finished run waits before it moves to subagents/archive/ (config.json, env wins).
function archiveHours() {
    const def = { archiveAfterHours: 6 };
    let config = null;
    try {
        config = JSON.parse(fs.readFileSync(configFile, "utf8"));
    } catch {}
    if (typeof config?.archiveAfterHours !== "number") {
        try {
            fs.mkdirSync(outDir, { recursive: true });
            fs.writeFileSync(configFile, `${JSON.stringify(def, null, 2)}\n`);
        } catch {}
        config = def;
    }
    const env = Number(process.env.SUBAGENT_ARCHIVE_HOURS);
    return process.env.SUBAGENT_ARCHIVE_HOURS && Number.isFinite(env) && env >= 0 ? env : config.archiveAfterHours;
}

/// Rename a file or folder, creating the parent; true when nothing is left at `from`.
function move(from, to) {
    if (from === to || !fs.existsSync(from)) return true;
    try {
        fs.mkdirSync(path.dirname(to), { recursive: true });
        fs.rmSync(to, { recursive: true, force: true });
        fs.renameSync(from, to);
        return true;
    } catch {
        return false; // in use: the next pass tries again
    }
}

/// Move the files of `from` into `to` (skipping names already there) and remove the source folder
/// once it is empty; true when everything that could move did.
function mergeDir(from, to) {
    if (!fs.existsSync(from)) return true;
    try {
        fs.mkdirSync(to, { recursive: true });
        for (const e of fs.readdirSync(from, { withFileTypes: true })) {
            const dst = path.join(to, e.name);
            if (fs.existsSync(dst)) continue;
            fs.renameSync(path.join(from, e.name), dst);
        }
        if (!fs.readdirSync(from).length) fs.rmdirSync(from);
        return true;
    } catch {
        return false; // in use: the next pass tries again
    }
}

const underArchive = (rel) => rel.replace(/^subagents\//, "subagents/archive/");
const fromArchive = (rel) => rel.replace(/^subagents\/archive\//, "subagents/");

/// The run's artifacts folder (repo-relative, "" when there is none), keeping the index in step:
/// the authoritative folder wins when it exists, else the stored mapping if that one still exists;
/// mappings to a folder that is gone or a placeholder are dropped. Files are never moved here,
/// except that a finished Claude Code subagent's folder takes the log's name and a leftover
/// task-named folder is merged into it (the subagent may keep writing after the first rename).
function resolveArtifacts(src, index, task, archived, finished) {
    const key = `${src.key}#artifacts`;
    const named = artifactFolder(src, task);
    const wanted = `subagents/artifacts/${index[src.key].replace(/\.md$/, "")}`;
    const abs = (rel) => (rel ? path.join(repo, rel) : "");
    const exists = (rel) => Boolean(rel) && !/[<>]/.test(rel) && fs.existsSync(abs(rel));
    let merged = true;
    if (!archived && src.kind === "claude" && finished && named && named !== wanted && exists(named)) {
        merged = exists(wanted) ? mergeDir(abs(named), abs(wanted)) : move(abs(named), abs(wanted));
    }
    const order = archived
        ? [underArchive(named), index[key]]
        : src.kind === "claude" && finished
          ? merged
              ? [wanted, named, index[key]]
              : [named, wanted, index[key]]
          : [named, wanted, index[key]];
    const artifacts = order.find(exists) || "";
    if (artifacts) index[key] = artifacts;
    else delete index[key];
    return artifacts;
}

/// The files whose links follow subagents/: every workflow/*.md and CLAUDE.md at the root.
export function docFiles(root) {
    const files = [];
    try {
        for (const f of fs.readdirSync(path.join(root, "workflow"))) if (f.endsWith(".md")) files.push(path.join(root, "workflow", f));
    } catch {}
    const claude = path.join(root, "CLAUDE.md");
    if (fs.existsSync(claude)) files.push(claude);
    return files;
}

const escapeRe = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
/// A path segment ends here: no letter, digit or name character follows (so run `a` never matches `a-b`).
const SEGMENT_END = "(?![\\p{L}\\p{N}_.-])";

/// Rewrite occurrences of the repo paths `from` → `to` in the docs. Only the `subagents/…` part is
/// replaced, so a relative prefix stays as it was (`../subagents/x` → `../subagents/archive/x`).
/// A file is written only when its text changed (line endings are left alone). Returns its path.
function rewriteDocPaths(root, pairs) {
    const rules = pairs.map(([from, to]) => [
        new RegExp(`subagents/${escapeRe(from.replace(/^subagents\//, ""))}${SEGMENT_END}`, "gu"),
        to,
    ]);
    const changed = [];
    for (const file of docFiles(root)) {
        let text;
        try {
            text = fs.readFileSync(file, "utf8");
        } catch {
            continue;
        }
        let out = text;
        for (const [re, to] of rules) out = out.replace(re, () => to);
        if (out === text) continue;
        fs.writeFileSync(file, out, "utf8");
        changed.push(file);
    }
    return changed;
}

/// Follow a run's log and artifacts links in the docs: archived → under subagents/archive/, back
/// when it returns from the archive. `kinds` says which of the two actually moved (a failed move
/// must not point the docs at a path that is not there).
export function rewriteRunLinks(root, run, archived, kinds = { log: true, artifacts: true }) {
    const name = run.replace(/\.md$/, "");
    const pairs = [];
    if (kinds.log) pairs.push(archived ? [`subagents/${name}.md`, `subagents/archive/${name}.md`] : [`subagents/archive/${name}.md`, `subagents/${name}.md`]);
    if (kinds.artifacts)
        pairs.push(
            archived
                ? [`subagents/artifacts/${name}`, `subagents/archive/artifacts/${name}`]
                : [`subagents/archive/artifacts/${name}`, `subagents/artifacts/${name}`],
        );
    return rewriteDocPaths(root, pairs);
}

/// A link in a doc: the relative prefix, subagents/, an optional archive/ and artifacts/, the name.
const DOC_LINK = /((?:\.\.?\/)*)subagents\/(archive\/)?(artifacts\/)?([^\s\/)\]}`'"<>,;:]+)(?=\/|[\s)\]}`'"<>,;:]|$)/gu;

/// One-time repair: a link to a log (`<run>.md`) or an artifacts folder that is missing while its
/// archived (or live) counterpart exists moves to the existing one. Other paths and text are left.
export function repairDocLinks(root) {
    const changed = [];
    for (const file of docFiles(root)) {
        let text;
        try {
            text = fs.readFileSync(file, "utf8");
        } catch {
            continue;
        }
        const base = path.dirname(file);
        const out = text.replace(DOC_LINK, (m, prefix, archived = "", kind = "", name) => {
            if (!kind && !name.endsWith(".md")) return m; // not a log
            if (name === "registry.md") return m; // its paths are its own
            const rel = `subagents/${archived}${kind}${name}`;
            const other = archived ? rel.replace("subagents/archive/", "subagents/") : rel.replace(/^subagents\//, "subagents/archive/");
            if (fs.existsSync(path.resolve(base, prefix + rel))) return m;
            if (!fs.existsSync(path.resolve(base, prefix + other))) return m;
            return prefix + other;
        });
        if (out === text) continue;
        fs.writeFileSync(file, out, "utf8");
        changed.push(file);
    }
    return changed;
}

/// A finished run untouched for long: its log and artifacts move under subagents/archive/.
function archiveRun(src, index, mtime) {
    const name = index[src.key];
    if (!move(path.join(outDir, name), path.join(archiveDir, name))) return;
    const artifacts = index[`${src.key}#artifacts`];
    let movedArtifacts = false;
    if (artifacts) {
        const to = underArchive(artifacts);
        if (move(path.join(repo, artifacts), path.join(repo, to))) {
            index[`${src.key}#artifacts`] = to;
            movedArtifacts = true;
        }
    }
    index[`${src.key}#archived`] = true;
    index[`${src.key}#archivedAt`] = mtime;
    rewriteRunLinks(repo, name, true, { log: true, artifacts: movedArtifacts });
}

/// The transcript changed after archiving (a resume): the log and artifacts come back.
function unarchiveRun(src, index) {
    const name = index[src.key];
    if (!move(path.join(archiveDir, name), path.join(outDir, name))) return;
    const artifacts = index[`${src.key}#artifacts`];
    let movedArtifacts = false;
    if (artifacts?.startsWith("subagents/archive/")) {
        const to = fromArchive(artifacts);
        if (move(path.join(repo, artifacts), path.join(repo, to))) {
            index[`${src.key}#artifacts`] = to;
            movedArtifacts = true;
        }
    }
    delete index[`${src.key}#archived`];
    delete index[`${src.key}#archivedAt`];
    rewriteRunLinks(repo, name, false, { log: true, artifacts: movedArtifacts });
}

const checked = new Map(); // key → mtime the run was judged for archiving

/// Archive what is finished and old.
function archivePass(all, index, hours) {
    const cutoff = Date.now() - hours * 3600e3;
    for (const src of all) {
        let mtime = 0;
        try {
            mtime = fs.statSync(src.file).mtimeMs;
        } catch {}
        if (!index[src.key] || index[`${src.key}#archived`] || !mtime || mtime >= cutoff) continue;
        if (checked.get(src.key) === mtime) continue; // judged this version of the transcript already
        checked.set(src.key, mtime);
        if (state(src, readJsonl(src.file), mtime).finished) archiveRun(src, index, mtime);
    }
}

/// Bring back the archived runs that became active again (transcript changed: a resume).
function unarchivePass(all, index) {
    for (const src of all) {
        if (!index[`${src.key}#archived`]) continue;
        let mtime = 0;
        try {
            mtime = fs.statSync(src.file).mtimeMs;
        } catch {}
        if (mtime && mtime !== index[`${src.key}#archivedAt`]) unarchiveRun(src, index);
    }
}

/// The pid of a living watcher from subagents/.watch.pid, or 0.
function runningWatcher() {
    const pid = Number(readText(pidFile));
    if (!Number.isInteger(pid) || pid <= 0) return 0;
    try {
        process.kill(pid, 0);
        return pid;
    } catch {
        return 0;
    }
}

/// Where the run lives, for watching and recovery: pids, the session to resume.
function processLines(src, events) {
    const session = events.find((e) => e.session_id || e.sessionId);
    const sid = session?.session_id || session?.sessionId || "";
    if (src.kind === "grok-builtin") {
        const effort = src.effort ? `, effort ${src.effort}` : "";
        return [`* **Процессы:** встроенный подагент Grok \`${src.agentType || "?"}\`${effort}, родитель \`${src.parentSession || "?"}\`, сессия \`${src.sessionId || "?"}\``];
    }
    if (src.kind === "script") {
        const r = (f) => readText(path.join(src.dir, f));
        const bin = r("provider") === "grok" ? "grok" : "claude";
        const pids = [`${bin} ${r("pid") || "?"}${r("winpid") ? ` (Windows ${r("winpid")})` : ""}`, `скрипт ${r("script_pid") || "?"}`, `зеркало ${r("log_pid") || "?"}`];
        // Cost per call (cost-run, cost-resume-…), written by scripts/subagent.sh at the end.
        let costs = [];
        try {
            costs = fs.readdirSync(src.dir).filter((f) => f.startsWith("cost-")).map((f) => Number(r(f))).filter(Number.isFinite);
        } catch {}
        const costNote = r("provider") === "grok" ? " (оценка xAI; оплата по подписке)" : "";
        return [
            ...(costs.length ? [`* **Стоимость:** $${costs.reduce((a, b) => a + b, 0).toFixed(4)}${costNote}`] : []),
            `* **Процессы:** ${pids.join(", ")}`,
            `* **Сессия:** \`${sid || r("session_id") || "?"}\` · продолжить: \`scripts/subagent.sh resume ${path.relative(repo, src.dir).replace(/\\/g, "/")} MESSAGE_FILE\`, ход: \`scripts/subagent.sh status …\``,
        ];
    }
    // A subagent of Claude Code runs inside the lead's Claude Code process.
    const agent = path.basename(src.file, ".jsonl").replace(/^agent-/, "");
    const sessionDir = path.basename(path.dirname(path.dirname(src.file)));
    const host = sessionDir === process.env.CLAUDE_CODE_SESSION_ID && process.env.CLAUDE_PID ? `, процесс Claude Code ${process.env.CLAUDE_PID}` : "";
    return [`* **Процессы:** внутри сессии головы \`${sessionDir}\`${host}; агент \`${agent}\``];
}

/// A row in subagents/registry.md (the same table scripts/subagent.sh writes to).
function register(src, events, title, model, started) {
    const reg = path.join(outDir, "registry.md");
    if (!fs.existsSync(reg)) {
        fs.mkdirSync(outDir, { recursive: true });
        fs.writeFileSync(
            reg,
            "# Запуски подагентов\n\nПишется при старте каждого запуска. Проверить, кто жив: `scripts/subagent.sh list`.\n\n" +
                "| Начало | Задача | Модель | Процессы | Сессия | Папка запуска |\n|---|---|---|---|---|---|\n",
        );
    }
    const p = (n) => String(n).padStart(2, "0");
    const when = `${started.getFullYear()}-${p(started.getMonth() + 1)}-${p(started.getDate())} ${p(started.getHours())}:${p(started.getMinutes())}:${p(started.getSeconds())}`;
    const procs = processLines(src, events)[0].replace(/^\* \*\*Процессы:\*\* /, "").replace(/\|/g, "/");
    const session = src.sessionId || path.basename(path.dirname(path.dirname(src.file)));
    fs.appendFileSync(reg, `| ${when} | ${title.replace(/\|/g, "/")} | ${model} | ${procs} | \`${session}\` | \`${src.file}\` |\n`);
}

/// The artifacts the subagent left, linked from the log (paths relative to the log's folder).
function artifactLines(logDir, artifacts) {
    if (!artifacts) return [];
    const abs = path.join(repo, artifacts);
    let files = [];
    try {
        files = fs.readdirSync(abs, { recursive: true }).filter((f) => fs.statSync(path.join(abs, f)).isFile());
    } catch {
        return ["## Артефакты", "", "_пока нет_", ""];
    }
    const rel = relTo(logDir, abs);
    const lines = files.map((f) => {
        const link = `${rel}/${String(f).replace(/\\/g, "/")}`;
        return /\.(png|jpe?g|gif|webp)$/i.test(f) ? `* ![${f}](${link})` : `* [${f}](${link})`;
    });
    return ["## Артефакты", "", ...(lines.length ? lines : ["_пока нет_"]), ""];
}

function loadIndex() {
    try {
        return JSON.parse(fs.readFileSync(indexFile, "utf8"));
    } catch {
        return {};
    }
}

const rendered = new Map(); // key → mtime rendered

function render(src, index, env = {}) {
    let mtime;
    try {
        mtime = fs.statSync(src.file).mtimeMs;
    } catch {
        return false;
    }
    const events = readJsonl(src.file);
    if (!events.length) return false;
    const task =
        src.kind === "script"
            ? readText(path.join(src.dir, "task.md"))
            : src.kind === "grok-builtin"
              ? src.task || userChunkText(events) || firstTask(events)
              : firstTask(events);
    const title = src.title || task.split("\n").find((l) => l.trim()) || "задача";
    const startRaw = events.find((e) => e.timestamp != null && e.timestamp !== "")?.timestamp;
    const started = startedAt(startRaw, mtime);
    const streamModels = isGrok(src, events) ? collectModels(events, null) : [];
    const model = streamModels.length ? streamModels.join(", ") : src.model || events.find((e) => e.type === "system" && e.model)?.model || "";
    if (!index[src.key]) {
        // A run of scripts/subagent.sh is already named that way: its log takes the same name.
        const named = src.kind === "script" && /^\d{4}-\d\d-\d\d_\d\d-\d\d_/.test(path.basename(src.dir));
        index[src.key] = named ? `${path.basename(src.dir)}.md` : `${stamp(started)}_${shortModel(model)}_${slug(title)}.md`;
        // scripts/subagent.sh registers its runs itself. Claude Code's and Grok's built-in subagents are registered here.
        if (src.kind === "claude" || src.kind === "grok-builtin") register(src, events, title, model, started);
    }
    const { status, summary, finished } = state(src, events, mtime);
    const archived = Boolean(index[`${src.key}#archived`]);
    const logDir = env.outDir || (archived ? archiveDir : outDir);
    const artifacts = resolveArtifacts(src, index, task, archived, finished);
    const md = [
        `# ${title}`,
        "",
        `* **Модель:** ${model || "?"}${src.effort ? ` (effort ${src.effort})` : ""}`,
        `* **Статус:** ${status}`,
        `* **Начало:** ${started.toLocaleString("ru-RU")} · **обновлено:** ${new Date(mtime).toLocaleString("ru-RU")}`,
        ...processLines(src, events),
        `* **Журнал:** \`${src.file}\``,
        ...(artifacts ? [`* **Артефакты:** [${artifacts}](${relTo(logDir, path.join(repo, artifacts))})`] : []),
        "",
        "## Задание",
        "",
        block(task),
        "",
        "## Ход работы",
        "",
        ...renderEvents(events, src),
        ...(summary.length ? ["## Итог", "", ...summary, ""] : []),
        ...artifactLines(logDir, artifacts),
    ].join("\n");
    fs.mkdirSync(logDir, { recursive: true });
    fs.writeFileSync(path.join(logDir, index[src.key]), md);
    rendered.set(src.key, mtime);
    return finished;
}

let repaired = false; // the one-time repair of links in the docs has run

function pass(force) {
    // Once per process, before anything moves: point every stale link at the path that exists.
    if (!repaired) {
        repaired = true;
        repairDocLinks(repo);
    }
    const index = loadIndex();
    const all = sources();
    unarchivePass(all, index);
    for (const src of all) {
        let mtime = 0;
        try {
            mtime = fs.statSync(src.file).mtimeMs;
        } catch {}
        // Claude Code's subagents settle a little after their last line: re-render while recent.
        const recent = Date.now() - mtime < 60000;
        if (force || rendered.get(src.key) !== mtime || recent) render(src, index);
    }
    fs.mkdirSync(outDir, { recursive: true });
    archivePass(all, index, archiveHours());
    fs.writeFileSync(indexFile, JSON.stringify(index, null, 2));
}

/// One watcher only: take subagents/.watch.pid and keep rendering until stopped.
function startWatch() {
    fs.mkdirSync(outDir, { recursive: true });
    fs.writeFileSync(pidFile, String(process.pid));
    const mine = () => Number(readText(pidFile)) === process.pid;
    const drop = () => {
        try {
            if (mine()) fs.rmSync(pidFile, { force: true });
        } catch {}
    };
    process.on("exit", drop);
    process.on("SIGINT", () => process.exit(0));
    process.on("SIGTERM", () => process.exit(0));
    pass(true);
    setInterval(() => pass(false), 3000);
}

function readUsageFile(file) {
    let text = "";
    try {
        text = fs.readFileSync(file, "utf8");
    } catch {
        return null;
    }
    try {
        return JSON.parse(text);
    } catch {
        // A warning line may sit in front of the JSON object.
        const i = text.indexOf("{");
        const j = text.lastIndexOf("}");
        if (i >= 0 && j > i) {
            try {
                return JSON.parse(text.slice(i, j + 1));
            } catch {
                return null;
            }
        }
        return null;
    }
}

/// result.json, the cost inputs and the fast-model warning for scripts/subagent.sh.
function writeGrokFacts(dir) {
    const events = readJsonl(path.join(dir, "stream.jsonl"));
    const usage = readUsageFile(path.join(dir, "usage.json"));
    const facts = grokRunFacts(events, usage);
    const costUsd = facts.sessionUsd != null ? facts.sessionUsd.toFixed(4) : facts.turnUsd != null ? facts.turnUsd.toFixed(4) : "";
    const result = {
        type: "result",
        subtype: facts.fast.length ? "error_fast_model" : facts.subtype,
        is_error: Boolean(facts.fast.length) || facts.subtype !== "success",
        num_turns: facts.numTurns,
        duration_ms: facts.durationMs,
        session_id: facts.sessionId,
        result: facts.text,
        model: facts.models.join(", "),
        usage: facts.tokens,
        cost_usd: costUsd,
        cost_note: "xAI's estimate; the user pays by subscription",
        fast_warning: facts.warning,
    };
    fs.writeFileSync(path.join(dir, "result.json"), JSON.stringify(result, null, 2) + "\n");
    if (facts.sessionId) fs.writeFileSync(path.join(dir, "session_id"), facts.sessionId + "\n");
    if (facts.models.length) fs.writeFileSync(path.join(dir, "model"), facts.models.join(", ") + "\n");
    fs.writeFileSync(path.join(dir, "grok-cost-mode"), (facts.sessionUsd != null ? "session" : facts.turnUsd != null ? "turn" : "unknown") + "\n");
    fs.writeFileSync(path.join(dir, "grok-usd-session"), facts.sessionUsd != null ? facts.sessionUsd.toFixed(4) + "\n" : "");
    fs.writeFileSync(path.join(dir, "grok-usd-turn"), facts.turnUsd != null ? facts.turnUsd.toFixed(4) + "\n" : "");
    const fastFile = path.join(dir, "fast-model");
    if (facts.warning) fs.writeFileSync(fastFile, facts.warning + "\n");
    else {
        try {
            fs.rmSync(fastFile, { force: true });
        } catch {}
    }
}

/// One pass of `follow`: re-read the run (provider, model, name, stream) and render the log.
/// `end` in a Grok stream is one turn. While that agent pid is alive, the run is not finished,
/// so a later turn and a resume keep showing up. A partial last line is skipped, not fatal.
/// `env.outDir` / `env.index` / `env.isolated` keep tests off the real subagents/ folder.
export function followTick(dir, env = {}) {
    const src = runSource(path.resolve(dir));
    const index = env.index || loadIndex();
    const logRoot = env.outDir || outDir;
    try {
        if (!env.isolated && index[`${src.key}#archived`]) unarchiveRun(src, index);
        let finished = false;
        if (fs.existsSync(src.file)) finished = Boolean(render(src, index, { outDir: env.outDir }));
        if (!env.isolated) {
            fs.mkdirSync(path.dirname(indexFile), { recursive: true });
            fs.writeFileSync(indexFile, JSON.stringify(index, null, 2));
        }
        // The pid wins over a turn's `end`: the process is about to write the next turn.
        if (agentPidAlive(src)) finished = false;
        const name = index[src.key] || "";
        return { finished, log: name ? path.join(logRoot, name) : "" };
    } catch (e) {
        if (!env.quiet) console.error(`follow ${dir}: ${e && e.stack ? e.stack : e}`);
        return { finished: false, log: "", error: String(e && e.message ? e.message : e) };
    }
}

// Run the CLI only when this file is executed, so tests can import the helpers above.
const isMain = (() => {
    try {
        return fs.realpathSync(path.resolve(process.argv[1])) === fs.realpathSync(fileURLToPath(import.meta.url));
    } catch {
        return false;
    }
})();

if (isMain) {
    const [cmd, arg] = process.argv.slice(2);
    if (cmd === "id") {
        // The shared name of a run (log, artifacts, run folder): `id MODEL TITLE`, now.
        const [model, title] = process.argv.slice(3);
        console.log(`${stamp(new Date())}_${shortModel(model)}_${slug(title || "")}`);
    } else if (cmd === "once") {
        pass(true);
    } else if (cmd === "watch" && arg === "--detach") {
        // Started from a hook: hand the work to the background and return without a sound.
        try {
            const script = path.resolve(process.argv[1]);
            const child = spawn(process.execPath, [script, "watch"], { detached: true, stdio: "ignore", windowsHide: true });
            child.on("error", () => {});
            child.unref();
        } catch {}
    } else if (cmd === "watch") {
        if (!runningWatcher()) startWatch();
    } else if (cmd === "follow" && arg) {
        const timer = setInterval(() => {
            if (followTick(arg).finished) clearInterval(timer);
        }, 3000);
        if (followTick(arg).finished) clearInterval(timer);
    } else if (cmd === "grok-facts" && arg) {
        writeGrokFacts(path.resolve(arg));
    } else {
        console.error("usage: node scripts/subagent-log.mjs once | watch [--detach] | follow RUN_DIR | id MODEL TITLE | grok-facts RUN_DIR");
        process.exit(2);
    }
}
