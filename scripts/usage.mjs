#!/usr/bin/env node

import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import readline from "node:readline";
import { fileURLToPath } from "node:url";

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const COLS = 120;
const ROWS = 40;
const GROK_MS = 90_000;

const rootOf = (root) => root || repo;

function argsOf(argv) {
    const out = { _: [] };
    for (let i = 0; i < argv.length; i++) {
        const a = argv[i];
        if (a.startsWith("--")) {
            const key = a.slice(2);
            const next = argv[i + 1];
            if (next != null && !next.startsWith("--")) {
                out[key] = next;
                i++;
            } else out[key] = true;
        } else out._.push(a);
    }
    return out;
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function readText(file) {
    try {
        return fs.readFileSync(file, "utf8").trim();
    } catch {
        return "";
    }
}

function readJson(file) {
    try {
        return JSON.parse(fs.readFileSync(file, "utf8"));
    } catch {
        return null;
    }
}

function num(v) {
    const n = Number(v);
    return Number.isFinite(n) ? n : 0;
}

function emit(obj) {
    process.stdout.write(`${JSON.stringify(obj)}\n`);
}

function unavailable(error, extra = {}) {
    return { ok: false, error, ...extra };
}

async function settle(fn, extra = {}) {
    try {
        const v = await fn();
        if (!v || typeof v !== "object") return unavailable("empty result", extra);
        return v;
    } catch (e) {
        return unavailable(String(e && e.message ? e.message : e), extra);
    }
}

export function parseUsageScreen(raw) {
    const text = String(raw || "").replace(/\r/g, "");
    const weekAt = text.toLowerCase().lastIndexOf("weekly limit");
    const region = weekAt >= 0 ? text.slice(weekAt, weekAt + 600) : "";
    const picked = region.match(/(\d+(?:\.\d+)?)\s*%/);
    const percent = picked ? Number(picked[1]) : null;
    const lines = (region || text)
        .split("\n")
        .map((l) => l.trim())
        .filter(Boolean);
    const resetLine = lines.find((l) => /reset/i.test(l));
    let resets = "";
    if (resetLine) {
        resets = resetLine.replace(/^.*?\breset(?:s|ting)?\b[:\s]*/i, "").trim() || resetLine;
    } else {
        const timeLine = lines.find((l) => /\b(?:am|pm|\d{1,2}:\d{2}|day|hour|minute)\b/i.test(l) && !/%/.test(l));
        if (timeLine) resets = timeLine;
    }
    resets = String(resets)
        .replace(/^[\s\u2500-\u257F|]+/g, "")
        .replace(/[\s\u2500-\u257F|]+$/g, "")
        .trim();
    if (percent == null || Number.isNaN(percent)) return { ok: false, error: "percent not found", raw: text };
    return { ok: true, percent, resets, raw: text };
}

function screenOf(term) {
    const buf = term.buffer.active;
    const top = buf.viewportY || 0;
    const lines = [];
    for (let i = 0; i < term.rows; i++) {
        const line = buf.getLine(top + i);
        lines.push(line ? line.translateToString(true) : "");
    }
    return lines.join("\n");
}

function findGrok() {
    const exts = process.platform === "win32" ? [".exe", ".cmd", ".bat", ""] : [""];
    for (const dir of (process.env.PATH || "").split(path.delimiter)) {
        if (!dir) continue;
        for (const ext of exts) {
            const file = path.join(dir, `grok${ext}`);
            try {
                if (fs.statSync(file).isFile()) return file;
            } catch {
                // next
            }
        }
    }
    return "grok";
}

function composerReady(text) {
    if (/Grok 4\.7[^\n]{0,24}fast/i.test(text)) return false;
    return /Grok 4\.7 \(/.test(text) && /\u2502 >/.test(text);
}

function weeklyVisible(text) {
    const at = text.toLowerCase().lastIndexOf("weekly limit");
    if (at < 0) return false;
    return /\d+(?:\.\d+)?\s*%/.test(text.slice(at, at + 600));
}

function usageLoading(text) {
    return /loading usage/i.test(text);
}

function usageMeterDown(text) {
    return /couldn'?t load usage|meter unavailable|billing service error/i.test(text);
}

function modalOpen(text) {
    return /context usage/i.test(text) && /usage limit/i.test(text);
}

export async function grokLimit() {
    let raw = "";
    let term;
    let pty;
    let exited = false;
    let painted = Promise.resolve();
    const started = Date.now();
    const deadline = started + GROK_MS;
    const cwd = fs.mkdtempSync(path.join(os.tmpdir(), "grok-usage-"));
    const frame = async () => {
        await Promise.race([painted, sleep(400)]);
        raw = term ? screenOf(term) : raw;
        return raw;
    };
    const waitFor = async (pred, until) => {
        let hits = 0;
        while (Date.now() < until && !exited) {
            const text = await frame();
            if (pred(text)) {
                hits++;
                if (hits >= 2) return text;
            } else hits = 0;
            await sleep(150);
        }
        return null;
    };
    const typeUsage = async () => {
        for (const ch of "/usage") {
            try {
                pty.write(ch);
            } catch {
                return false;
            }
            await sleep(30);
        }
        const echoed = await waitFor(
            (t) => /\/usage/.test(t) || weeklyVisible(t) || usageLoading(t) || modalOpen(t),
            Math.min(deadline, Date.now() + 4000),
        );
        if (!echoed) return false;
        if (weeklyVisible(echoed) || usageLoading(echoed) || (modalOpen(echoed) && !/\/usage/.test(echoed))) return true;
        try {
            pty.write("\r");
        } catch {
            return false;
        }
        return true;
    };
    const readModal = async (until) => {
        let tabs = 0;
        while (Date.now() < until && !exited) {
            const text = await frame();
            if (weeklyVisible(text)) return "ok";
            if (usageLoading(text)) {
                await sleep(200);
                continue;
            }
            if (modalOpen(text) && !/weekly limit/i.test(text) && tabs < 3) {
                try {
                    pty.write("\t");
                } catch {
                    return "gone";
                }
                tabs++;
                await sleep(400);
                continue;
            }
            if (usageMeterDown(text)) {
                const recovered = await waitFor(weeklyVisible, Math.min(until, Date.now() + 3000));
                if (recovered) return "ok";
                return "meter";
            }
            await sleep(200);
        }
        return "timeout";
    };
    try {
        const require = (await import("node:module")).createRequire(path.join(repo, "tools/grok-usage/package.json"));
        const ptyMod = require("node-pty");
        const { Terminal } = require("@xterm/headless");
        term = new Terminal({ cols: COLS, rows: ROWS, allowProposedApi: true });
        const bin = findGrok();
        pty = ptyMod.spawn(bin, ["-m", "grok-4.7"], {
            name: "xterm-256color",
            cols: COLS,
            rows: ROWS,
            cwd,
            env: process.env,
        });
        pty.onData((data) => {
            if (!term) return;
            painted = painted.then(
                () =>
                    new Promise((resolve) => {
                        term.write(data, () => resolve());
                    }),
            );
        });
        pty.onExit(() => {
            exited = true;
        });
        let outcome = "timeout";
        let sawMeter = false;
        let sawComposer = false;
        let fastModel = false;
        for (let attempt = 0; attempt < 2 && Date.now() < deadline && !exited; attempt++) {
            let nudged = false;
            let hits = 0;
            const attemptStart = Date.now();
            const readyAt = Math.min(deadline, attemptStart + 25000);
            const ready = await (async () => {
                while (Date.now() < readyAt && !exited) {
                    const text = await frame();
                    if (/Grok 4\.7[^\n]{0,24}fast/i.test(text)) return "fast";
                    if (composerReady(text)) {
                        hits++;
                        if (hits >= 2) return "ready";
                    } else hits = 0;
                    if (!nudged && Date.now() > attemptStart + 8000) {
                        nudged = true;
                        try {
                            pty.write("\x1b");
                        } catch {
                            return "gone";
                        }
                    }
                    await sleep(150);
                }
                return null;
            })();
            if (ready === "fast") {
                fastModel = true;
                break;
            }
            if (ready !== "ready") break;
            sawComposer = true;
            const typed = await typeUsage();
            if (!typed) {
                try {
                    pty.write("\x1b");
                } catch {
                    break;
                }
                continue;
            }
            outcome = await readModal(Math.min(deadline, Date.now() + 20000));
            if (outcome === "ok") break;
            if (outcome === "meter") sawMeter = true;
            try {
                pty.write("\x1b");
            } catch {
                break;
            }
            await sleep(300);
        }
        raw = await frame();
        const parsed = outcome === "ok" ? parseUsageScreen(raw) : null;
        const error = fastModel
            ? "fast model"
            : outcome === "ok"
              ? parsed?.error || "parse failed"
              : sawMeter
                ? "usage meter unavailable"
                : !sawComposer
                  ? exited
                      ? "grok exited before the prompt"
                      : "input not ready"
                  : modalOpen(raw) || usageLoading(raw)
                    ? "usage modal has no weekly percent"
                    : "usage modal did not appear";
        try {
            pty.write("\x1b");
        } catch {
            // already gone
        }
        await sleep(300);
        try {
            pty.write("/exit\r");
        } catch {
            // already gone
        }
        const killAt = Date.now() + 4000;
        while (!exited && Date.now() < killAt) await sleep(200);
        if (!exited) {
            try {
                pty.kill();
            } catch {
                // already gone
            }
        }
        if (parsed?.ok) return { ok: true, percent: parsed.percent, resets: parsed.resets, raw };
        return { ok: false, error, raw };
    } catch (e) {
        try {
            pty?.kill();
        } catch {
            // already gone
        }
        return { ok: false, error: String(e && e.message ? e.message : e), raw };
    } finally {
        try {
            term?.dispose();
        } catch {
            // headless terminal has nothing to dispose
        }
        try {
            fs.rmSync(cwd, { recursive: true, force: true });
        } catch {
            // temp dir
        }
    }
}

function deepseekKey() {
    const fromEnv = process.env.DEEPSEEK_API_KEY;
    if (fromEnv && fromEnv.trim()) return fromEnv.trim();
    const file = path.join(os.homedir(), ".secrets", "deepseek.key");
    const text = readText(file);
    return text || "";
}

export async function deepseekBalance() {
    const key = deepseekKey();
    if (!key) return unavailable("no key");
    let res;
    try {
        res = await fetch("https://api.deepseek.com/user/balance", {
            headers: { Authorization: `Bearer ${key}`, Accept: "application/json" },
            signal: AbortSignal.timeout(20_000),
        });
    } catch (e) {
        return unavailable(String(e && e.message ? e.message : e));
    }
    let body = null;
    try {
        body = await res.json();
    } catch {
        return unavailable(`balance response is not JSON (${res.status})`);
    }
    if (!res.ok) return unavailable(`balance HTTP ${res.status}`);
    const infos = Array.isArray(body?.balance_infos) ? body.balance_infos : [];
    const row = infos.find((x) => x && x.total_balance != null) || null;
    if (!row) return unavailable("no balance in the response");
    const balance = Number(row.total_balance);
    if (!Number.isFinite(balance)) return unavailable("balance is not a number");
    return { ok: true, balance, currency: String(row.currency || "USD") };
}

function claudePart(opts) {
    const five = opts.claude5h;
    const week = opts.claudeWeek;
    if (five == null && week == null) return unavailable("not provided");
    const fiveHour = five == null || five === "" ? null : Number(five);
    const weekN = week == null || week === "" ? null : Number(week);
    if ((fiveHour != null && !Number.isFinite(fiveHour)) || (weekN != null && !Number.isFinite(weekN))) {
        return unavailable("claude limits are not numbers");
    }
    if (fiveHour == null || weekN == null) return unavailable("claude limits incomplete");
    return { ok: true, fiveHour, week: weekN };
}

export async function takeSnapshot(opts = {}) {
    const only = opts.only || "";
    const want = (part) => !only || only === part;
    const [grok, deepseek] = await Promise.all([
        want("grok") ? settle(() => (opts.grokLimit || grokLimit)(), { raw: "" }) : Promise.resolve(unavailable("not requested", { raw: "" })),
        want("deepseek") ? settle(() => (opts.deepseekBalance || deepseekBalance)()) : Promise.resolve(unavailable("not requested")),
    ]);
    const line = {
        time: opts.time || new Date().toISOString(),
        label: String(opts.label || ""),
        claude: claudePart(opts),
        grok,
        deepseek,
    };
    const root = rootOf(opts.root);
    const file = path.join(root, "subagents", "usage.jsonl");
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.appendFileSync(file, `${JSON.stringify(line)}\n`);
    return line;
}

function fmtPct(n) {
    if (!Number.isFinite(n)) return "?";
    return Number.isInteger(n) ? String(n) : String(Math.round(n * 10) / 10);
}

function money(n) {
    return `$${Number(n).toFixed(2)}`;
}

export function runNote(dir) {
    const prov = readText(path.join(dir, "provider"));
    const start = readJson(path.join(dir, "usage-start.json"));
    const end = readJson(path.join(dir, "usage-end.json"));
    const why = (side, part) => {
        if (!side) return "missing";
        const p = side[part];
        if (p && p.ok) return "";
        return (p && p.error) || side.error || "missing";
    };
    if (prov === "grok") {
        const a = start?.grok?.ok ? start.grok.percent : null;
        const b = end?.grok?.ok ? end.grok.percent : null;
        if (a == null || b == null) return `Grok limit: not recorded (start: ${why(start, "grok")}; end: ${why(end, "grok")})`;
        return `Grok limit: ${fmtPct(a)} % → ${fmtPct(b)} %`;
    }
    if (prov === "deepseek") {
        const a = start?.deepseek?.ok ? start.deepseek.balance : null;
        const b = end?.deepseek?.ok ? end.deepseek.balance : null;
        if (a == null || b == null) return `DeepSeek balance: not recorded (start: ${why(start, "deepseek")}; end: ${why(end, "deepseek")})`;
        return `DeepSeek balance: ${money(a)} → ${money(b)}`;
    }
    return "";
}

function listRunDirs(root) {
    const out = [];
    for (const base of [path.join(root, "target", "subagents"), path.join(root, "target", "subagents", "archive")]) {
        let names = [];
        try {
            names = fs.readdirSync(base);
        } catch {
            continue;
        }
        for (const name of names) {
            if (name === "archive" && base.endsWith(`${path.sep}subagents`)) continue;
            const dir = path.join(base, name);
            try {
                if (!fs.statSync(dir).isDirectory()) continue;
            } catch {
                continue;
            }
            if (!readText(path.join(dir, "provider")) && !fs.existsSync(path.join(dir, "stream.jsonl"))) continue;
            out.push(dir);
        }
    }
    return out;
}

function costOf(dir) {
    let sum = 0;
    let any = false;
    let names = [];
    try {
        names = fs.readdirSync(dir);
    } catch {
        return null;
    }
    for (const f of names) {
        if (!f.startsWith("cost-")) continue;
        const n = Number(readText(path.join(dir, f)));
        if (Number.isFinite(n)) {
            sum += n;
            any = true;
        }
    }
    return any ? sum : null;
}

function scriptRun(dir) {
    const result = readJson(path.join(dir, "result.json")) || {};
    const u = result.usage || {};
    const startedSec = Number(readText(path.join(dir, "started")));
    const started = Number.isFinite(startedSec) && startedSec > 0 ? new Date(startedSec * 1000) : null;
    let end = null;
    if (started && num(result.duration_ms) > 0) end = new Date(started.getTime() + num(result.duration_ms));
    else {
        try {
            end = fs.statSync(path.join(dir, "result.json")).mtime;
        } catch {
            end = null;
        }
    }
    return {
        kind: "script",
        executor: readText(path.join(dir, "provider")) || "?",
        model: readText(path.join(dir, "model")) || result.model || "?",
        name: readText(path.join(dir, "name")) || path.basename(dir),
        started,
        end,
        turns: Number.isFinite(result.num_turns) ? result.num_turns : null,
        input: num(u.input_tokens ?? u.inputTokens),
        output: num(u.output_tokens ?? u.outputTokens),
        reasoning: num(u.reasoning_tokens ?? u.reasoningTokens ?? u.output_tokens_details?.thinking_tokens),
        cost: costOf(dir),
        costKind: readText(path.join(dir, "provider")) === "grok" ? "xAI" : "tokens",
    };
}

function cell(v) {
    return String(v ?? "").replace(/\|/g, "/").replace(/\n/g, " ");
}

function registryClaude(root) {
    const text = readText(path.join(root, "subagents", "registry.md"));
    const rows = [];
    const seen = new Set();
    for (const line of text.split("\n")) {
        if (!line.startsWith("|")) continue;
        if (/^\|\s*-+/.test(line)) continue;
        const cols = line.split("|").slice(1, -1).map((c) => c.trim());
        if (cols.length < 6) continue;
        if (cols[0] === "Начало") continue;
        const folder = cols[5].replace(/^`|`$/g, "");
        if (!folder.endsWith(".jsonl")) continue;
        if (folder.includes("target/subagents") || folder.includes("target\\subagents")) continue;
        if (seen.has(folder)) continue;
        seen.add(folder);
        rows.push({
            startedText: cols[0],
            title: cols[1],
            model: cols[2],
            file: folder,
        });
    }
    return rows;
}

async function claudeUsage(file) {
    const seen = new Map();
    let model = "";
    if (!fs.existsSync(file)) return { model: "", turns: null, input: 0, output: 0, reasoning: 0, missing: true };
    const rl = readline.createInterface({ input: fs.createReadStream(file, { encoding: "utf8" }), crlfDelay: Infinity });
    for await (const line of rl) {
        if (!line) continue;
        let e;
        try {
            e = JSON.parse(line);
        } catch {
            continue;
        }
        const msg = e.message;
        if (!msg || msg.role !== "assistant") continue;
        if (msg.model) model = msg.model;
        if (!msg.usage) continue;
        seen.set(msg.id || `n${seen.size}`, msg.usage);
    }
    let input = 0;
    let output = 0;
    let reasoning = 0;
    for (const u of seen.values()) {
        input += num(u.input_tokens) + num(u.cache_creation_input_tokens) + num(u.cache_read_input_tokens);
        output += num(u.output_tokens);
        reasoning += num(u.output_tokens_details?.thinking_tokens);
    }
    return { model, turns: seen.size, input, output, reasoning, missing: false };
}

function executorOf(model) {
    const m = String(model || "").toLowerCase();
    if (m.includes("sonnet")) return "sonnet";
    if (m.includes("haiku")) return "haiku";
    if (m.includes("opus")) return "opus";
    return "claude";
}

async function loadRuns(root) {
    const runs = listRunDirs(root).map(scriptRun);
    for (const row of registryClaude(root)) {
        const usage = await claudeUsage(row.file);
        const started = new Date(row.startedText.replace(" ", "T"));
        let end = null;
        try {
            end = fs.statSync(row.file).mtime;
        } catch {
            end = null;
        }
        runs.push({
            kind: "claude",
            executor: executorOf(usage.model || row.model),
            model: usage.model || row.model || "?",
            name: row.title,
            started: Number.isNaN(started.getTime()) ? null : started,
            end,
            turns: usage.turns,
            input: usage.input,
            output: usage.output,
            reasoning: usage.reasoning,
            cost: null,
            costKind: "subscription",
            missing: usage.missing,
        });
    }
    return runs;
}

function loadSnapshots(root) {
    const file = path.join(root, "subagents", "usage.jsonl");
    if (!fs.existsSync(file)) return [];
    const out = [];
    for (const line of fs.readFileSync(file, "utf8").split("\n")) {
        if (!line.trim()) continue;
        try {
            out.push(JSON.parse(line));
        } catch {
            // a half-written line
        }
    }
    out.sort((a, b) => String(a.time).localeCompare(String(b.time)));
    return out;
}

export function pairSnapshots(snaps) {
    const open = new Map();
    const pairs = [];
    for (const s of snaps) {
        const m = /^(start|end)\s+(.*)$/.exec(String(s.label || ""));
        if (!m) continue;
        if (m[1] === "start") {
            const q = open.get(m[2]) || [];
            q.push(s);
            open.set(m[2], q);
        } else {
            const q = open.get(m[2]) || [];
            const start = q.shift();
            if (start) pairs.push({ name: m[2], start, end: s });
        }
    }
    return pairs;
}

function partDelta(start, end, read, fmt, unit) {
    const a = read(start);
    const b = read(end);
    if (a == null || b == null) return "не снят";
    const d = b - a;
    const sign = d > 0 ? "+" : "";
    return `${fmt(a)} → ${fmt(b)} (${sign}${fmt(d)}${unit})`;
}

function pairCells(pair) {
    const claude5 = partDelta(pair.start, pair.end, (s) => (s.claude?.ok ? s.claude.fiveHour : null), (n) => `${fmtPct(n)} %`, "");
    const claudeW = partDelta(pair.start, pair.end, (s) => (s.claude?.ok ? s.claude.week : null), (n) => `${fmtPct(n)} %`, "");
    const grok = partDelta(pair.start, pair.end, (s) => (s.grok?.ok ? s.grok.percent : null), (n) => `${fmtPct(n)} %`, "");
    const deep = partDelta(pair.start, pair.end, (s) => (s.deepseek?.ok ? s.deepseek.balance : null), (n) => money(n), "");
    return { claude5, claudeW, grok, deep };
}

function when(d) {
    if (!d || Number.isNaN(d.getTime())) return "—";
    const p = (n) => String(n).padStart(2, "0");
    return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

function costCell(run) {
    if (run.cost == null) return run.costKind === "subscription" ? "подписка" : "—";
    const note = run.costKind === "xAI" ? " оценка xAI" : "";
    return `$${run.cost.toFixed(4)}${note}`;
}

export function renderUsageReport(runs, snaps) {
    const pairs = pairSnapshots(snaps);
    const byName = new Map();
    for (const p of pairs) {
        const list = byName.get(p.name) || [];
        list.push(p);
        byName.set(p.name, list);
    }
    const sortedRuns = [...runs].sort((a, b) => (b.started?.getTime() || 0) - (a.started?.getTime() || 0));
    const sortedPairs = [...pairs].sort((a, b) => String(b.end.time).localeCompare(String(a.end.time)));
    const runRows = sortedRuns.map((r) => {
        const around = byName.get(r.name) || [];
        const snap = around.length
            ? around
                  .map((p) => {
                      const c = pairCells(p);
                      return `Grok ${c.grok}; DeepSeek ${c.deep}; Claude 5ч ${c.claude5}; неделя ${c.claudeW}`;
                  })
                  .join(" · ")
            : "—";
        return `| ${cell(when(r.started))} | ${cell(r.executor)} | ${cell(r.model)} | ${cell(when(r.end))} | ${r.turns ?? "—"} | ${r.input || "—"} | ${r.output || "—"} | ${r.reasoning || "—"} | ${cell(costCell(r))} | ${cell(snap)} |`;
    });
    const seriesRows = sortedPairs.map((p) => {
        const c = pairCells(p);
        return `| ${cell(p.name)} | ${cell(p.end.time)} | ${c.claude5} | ${c.claudeW} | ${c.grok} | ${c.deep} |`;
    });
    const totals = new Map();
    for (const r of runs) {
        const t = totals.get(r.executor) || { n: 0, turns: 0, input: 0, output: 0, reasoning: 0, cost: 0, costN: 0 };
        t.n++;
        t.turns += r.turns || 0;
        t.input += r.input || 0;
        t.output += r.output || 0;
        t.reasoning += r.reasoning || 0;
        if (r.cost != null) {
            t.cost += r.cost;
            t.costN++;
        }
        totals.set(r.executor, t);
    }
    const totalRows = [...totals.entries()]
        .sort((a, b) => b[1].cost - a[1].cost || b[1].n - a[1].n)
        .map(([name, t]) => `| ${cell(name)} | ${t.n} | ${t.turns} | ${t.input} | ${t.output} | ${t.reasoning} | ${t.costN ? `$${t.cost.toFixed(4)}` : "—"} |`);
    return [
        "# Расход по исполнителям",
        "",
        "Новые строки сверху. Цена DeepSeek — токены × прайс. Цена Grok — оценка xAI (оплата подпиской).",
        "Вход Claude включает cache read и cache write. Лимиты Claude общие для головы Claude и Sonnet:",
        "разность снимка вокруг запуска Sonnet включает и работу головы, разделить их нельзя.",
        "Голова Grok снимок пишет без чисел Claude (`get_usage` у неё нет): в столбцах Claude — «не снят»,",
        "неделя Grok берётся из `node scripts/usage.mjs grok-limit`.",
        "",
        "## Запуски",
        "",
        "| Начало | Исполнитель | Модель | Конец | Ходы | Вход | Выход | Рассуждения | Цена | Снимки вокруг |",
        "|---|---|---|---|---:|---:|---:|---:|---|---|",
        ...(runRows.length ? runRows : ["| — | — | — | — | — | — | — | — | — | — |"]),
        "",
        "## Серии",
        "",
        "Пары снимков `start …` / `end …`. «не снят» — метра не было в одном из снимков.",
        "",
        "| Серия | Конец | Claude 5 ч | Claude неделя | Grok неделя | DeepSeek |",
        "|---|---|---|---|---|---|",
        ...(seriesRows.length ? seriesRows : ["| — | — | — | — | — | — |"]),
        "",
        "## Итого по исполнителям",
        "",
        "| Исполнитель | Запуски | Ходы | Вход | Выход | Рассуждения | Цена |",
        "|---|---:|---:|---:|---:|---:|---:|",
        ...(totalRows.length ? totalRows : ["| — | — | — | — | — | — | — |"]),
        "",
    ].join("\n");
}

export async function writeReport(root) {
    const base = rootOf(root);
    const runs = await loadRuns(base);
    const snaps = loadSnapshots(base);
    const md = renderUsageReport(runs, snaps);
    const file = path.join(base, "subagents", "usage.md");
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, md);
    return { file, markdown: md };
}

async function main() {
    const argv = argsOf(process.argv.slice(2));
    const cmd = argv._[0] || "";
    try {
        if (cmd === "grok-limit") {
            emit(await grokLimit());
        } else if (cmd === "deepseek-balance") {
            emit(await deepseekBalance());
        } else if (cmd === "snapshot") {
            const line = await takeSnapshot({
                label: argv.label || "",
                claude5h: argv["claude-5h"],
                claudeWeek: argv["claude-week"],
                only: argv.only || "",
                root: argv.root || repo,
            });
            emit(line);
        } else if (cmd === "note") {
            const line = runNote(path.resolve(argv._[1] || "."));
            if (line) process.stdout.write(`${line}\n`);
        } else if (cmd === "report") {
            const { file } = await writeReport(argv.root || repo);
            process.stdout.write(`${file}\n`);
        } else {
            process.stderr.write("usage: node scripts/usage.mjs grok-limit | deepseek-balance | snapshot --label TEXT [--claude-5h N] [--claude-week N] [--only grok|deepseek] | note RUN_DIR | report\n");
            process.exitCode = 2;
        }
    } catch (e) {
        emit({ ok: false, error: String(e && e.message ? e.message : e), raw: "" });
    }
}

const isMain = (() => {
    try {
        return fs.realpathSync(path.resolve(process.argv[1] || "")) === fs.realpathSync(fileURLToPath(import.meta.url));
    } catch {
        return false;
    }
})();

if (isMain) await main();
