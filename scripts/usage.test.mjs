import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { parseUsageScreen, takeSnapshot, runNote, writeReport } from "./usage.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));

function tempRoot() {
    return fs.mkdtempSync(path.join(os.tmpdir(), "usage-test-"));
}

test("parses the live /usage screen", () => {
    const raw = fs.readFileSync(path.join(here, "fixtures", "grok-usage-screen.txt"), "utf8");
    const parsed = parseUsageScreen(raw);
    assert.equal(parsed.ok, true);
    assert.equal(parsed.percent, 5);
    assert.equal(parsed.resets, "October 28, 05:22");
    assert.match(parsed.raw, /no model calls yet/);
});

test("a screen without a percent is unavailable", () => {
    const parsed = parseUsageScreen("Usage limit\nResets: later\nno numbers\n");
    assert.equal(parsed.ok, false);
    assert.match(parsed.error, /percent/);
});

test("parses the screen the lead saved from a grok-limit run", () => {
    const raw = fs.readFileSync(path.join(here, "fixtures", "grok-usage-lead-fail.txt"), "utf8");
    const parsed = parseUsageScreen(raw);
    assert.equal(parsed.ok, true);
    assert.equal(parsed.percent, 5);
    assert.equal(parsed.resets, "October 28, 05:22");
    assert.match(parsed.raw, /Weekly limit/);
});

test("a usage modal that failed to load is not a weekly percent", () => {
    const raw = fs.readFileSync(path.join(here, "fixtures", "grok-usage-meter-unavailable.txt"), "utf8");
    const parsed = parseUsageScreen(raw);
    assert.equal(parsed.ok, false);
    assert.match(raw, /Couldn't load usage/);
    assert.match(raw, /meter unavailable/);
});

test("the context-usage percent is not the weekly limit", () => {
    const raw = fs.readFileSync(path.join(here, "fixtures", "grok-usage-context-tab.txt"), "utf8");
    const parsed = parseUsageScreen(raw);
    assert.equal(parsed.ok, false);
    assert.notEqual(parsed.percent, 0.53);
});

test("snapshot records a failed meter and does not throw", async () => {
    const root = tempRoot();
    let deepseekCalls = 0;
    const line = await takeSnapshot({
        root,
        label: "start bare",
        time: "2026-10-21T00:00:00.000Z",
        grokLimit: async () => {
            throw new Error("tui down");
        },
        deepseekBalance: async () => {
            deepseekCalls++;
            throw new Error("net down");
        },
    });
    assert.equal(line.grok.ok, false);
    assert.match(line.grok.error, /tui down/);
    assert.equal(line.deepseek.ok, false);
    assert.match(line.deepseek.error, /net down/);
    assert.equal(line.claude.ok, false);
    assert.equal(line.claude.error, "not provided");
    const saved = fs.readFileSync(path.join(root, "subagents", "usage.jsonl"), "utf8").trim().split("\n");
    assert.equal(saved.length, 1);
    assert.equal(JSON.parse(saved[0]).label, "start bare");

    const only = await takeSnapshot({
        root,
        label: "end bare",
        time: "2026-10-21T01:00:00.000Z",
        only: "grok",
        grokLimit: async () => ({ ok: false, error: "parse failed", raw: "" }),
        deepseekBalance: async () => {
            deepseekCalls++;
            throw new Error("should not be called");
        },
    });
    assert.equal(only.grok.ok, false);
    assert.equal(only.grok.error, "parse failed");
    assert.equal(only.deepseek.ok, false);
    assert.equal(only.deepseek.error, "not requested");
    assert.equal(deepseekCalls, 1);

    const half = await takeSnapshot({
        root,
        label: "check",
        only: "deepseek",
        claude5h: "nope",
        claudeWeek: "1",
        deepseekBalance: async () => ({ ok: false, error: "no key" }),
    });
    assert.equal(half.claude.ok, false);
    assert.equal(half.claude.error, "claude limits are not numbers");
    assert.equal(half.grok.error, "not requested");
});

test("run note prints the summary line", () => {
    const grokDir = path.join(tempRoot(), "grok-run");
    fs.mkdirSync(grokDir);
    fs.writeFileSync(path.join(grokDir, "provider"), "grok\n");
    fs.writeFileSync(path.join(grokDir, "usage-start.json"), JSON.stringify({ grok: { ok: true, percent: 12 } }));
    fs.writeFileSync(path.join(grokDir, "usage-end.json"), JSON.stringify({ grok: { ok: true, percent: 14 } }));
    assert.equal(runNote(grokDir), "Grok limit: 12 % → 14 %");

    const dsDir = path.join(tempRoot(), "ds-run");
    fs.mkdirSync(dsDir);
    fs.writeFileSync(path.join(dsDir, "provider"), "deepseek\n");
    fs.writeFileSync(
        path.join(dsDir, "usage-start.json"),
        JSON.stringify({ deepseek: { ok: true, balance: 9.81, currency: "USD" } }),
    );
    fs.writeFileSync(
        path.join(dsDir, "usage-end.json"),
        JSON.stringify({ deepseek: { ok: true, balance: 9.79, currency: "USD" } }),
    );
    assert.equal(runNote(dsDir), "DeepSeek balance: $9.81 → $9.79");

    const broken = path.join(tempRoot(), "broken");
    fs.mkdirSync(broken);
    fs.writeFileSync(path.join(broken, "provider"), "grok\n");
    fs.writeFileSync(path.join(broken, "usage-start.json"), JSON.stringify({ ok: false, error: "snapshot produced no output" }));
    assert.equal(
        runNote(broken),
        "Grok limit: not recorded (start: snapshot produced no output; end: missing)",
    );
});

test("report lists script runs, a claude subagent, and snapshot deltas", async () => {
    const root = tempRoot();
    const run = path.join(root, "target", "subagents", "2026-10-21_12-00_grok_demo");
    fs.mkdirSync(run, { recursive: true });
    fs.writeFileSync(path.join(run, "provider"), "grok\n");
    fs.writeFileSync(path.join(run, "model"), "grok-4.7-build\n");
    fs.writeFileSync(path.join(run, "name"), "demo run\n");
    fs.writeFileSync(path.join(run, "started"), "1760000000\n");
    fs.writeFileSync(
        path.join(run, "result.json"),
        JSON.stringify({
            num_turns: 3,
            duration_ms: 5000,
            usage: { input_tokens: 100, output_tokens: 40, reasoning_tokens: 7 },
        }),
    );
    fs.writeFileSync(path.join(run, "cost-a"), "0.25\n");
    fs.writeFileSync(path.join(run, "cost-b"), "0.25\n");

    const archived = path.join(root, "target", "subagents", "archive", "old-deepseek");
    fs.mkdirSync(archived, { recursive: true });
    fs.writeFileSync(path.join(archived, "provider"), "deepseek\n");
    fs.writeFileSync(path.join(archived, "model"), "deepseek-v4-flash\n");
    fs.writeFileSync(path.join(archived, "name"), "old ds\n");
    fs.writeFileSync(path.join(archived, "started"), "1750000000\n");
    fs.writeFileSync(
        path.join(archived, "result.json"),
        JSON.stringify({ num_turns: 1, usage: { input_tokens: 8, output_tokens: 2, reasoning_tokens: 0 } }),
    );
    fs.writeFileSync(path.join(archived, "cost-1"), "0.001\n");

    const transcript = path.join(root, "claude-agent.jsonl");
    const usage = {
        input_tokens: 10,
        cache_read_input_tokens: 20,
        cache_creation_input_tokens: 5,
        output_tokens: 4,
        output_tokens_details: { thinking_tokens: 2 },
    };
    const msg = { message: { id: "m1", role: "assistant", model: "claude-sonnet-4-5", usage } };
    fs.writeFileSync(transcript, `${JSON.stringify({ message: { role: "user", content: "hi" } })}\n${JSON.stringify(msg)}\n${JSON.stringify(msg)}\n`);

    const folder = transcript.replace(/\\/g, "/");
    fs.mkdirSync(path.join(root, "subagents"), { recursive: true });
    fs.writeFileSync(
        path.join(root, "subagents", "registry.md"),
        [
            "| Начало | Задача | Модель | Процессы | Сессия | Папка запуска |",
            "|---|---|---|---|---|---|",
            `| 2026-10-21 09:00:00 | claude job | claude-sonnet | p | s | \`${folder}\` |`,
            `| 2026-10-21 09:00:00 | claude job | claude-sonnet | p | s | \`${folder}\` |`,
        ].join("\n"),
    );

    const snaps = [
        {
            time: "2026-10-21T08:00:00.000Z",
            label: "start demo run",
            claude: { ok: true, fiveHour: 30, week: 10 },
            grok: { ok: true, percent: 12 },
            deepseek: { ok: true, balance: 9.81, currency: "USD" },
        },
        {
            time: "2026-10-21T08:30:00.000Z",
            label: "end demo run",
            claude: { ok: true, fiveHour: 40, week: 12 },
            grok: { ok: true, percent: 14 },
            deepseek: { ok: true, balance: 9.79, currency: "USD" },
        },
        {
            time: "2026-10-21T09:00:00.000Z",
            label: "start broken",
            claude: { ok: false, error: "not provided" },
            grok: { ok: false, error: "tui down" },
            deepseek: { ok: false, error: "no key" },
        },
        {
            time: "2026-10-21T09:30:00.000Z",
            label: "end broken",
            claude: { ok: false, error: "not provided" },
            grok: { ok: false, error: "tui down" },
            deepseek: { ok: false, error: "no key" },
        },
    ];
    fs.writeFileSync(
        path.join(root, "subagents", "usage.jsonl"),
        `${snaps.map((s) => JSON.stringify(s)).join("\n")}\n{not json\n`,
    );

    const { markdown } = await writeReport(root);
    assert.match(markdown, /включает и работу головы/);
    assert.match(markdown, /grok-4\.7-build/);
    assert.match(markdown, /\$0\.5000 оценка xAI/);
    assert.match(markdown, /\| 3 \| 100 \| 40 \| 7 \|/);
    assert.match(markdown, /deepseek-v4-flash/);
    assert.match(markdown, /\| 1 \| 8 \| 2 \| — \| \$0\.0010 \|/);
    assert.doesNotMatch(markdown, /deepseek-v4-flash[^\n]*оценка xAI/);
    const claudeRows = markdown.split("\n").filter((l) => l.includes("claude-sonnet-4-5"));
    assert.equal(claudeRows.length, 1);
    assert.match(claudeRows[0], /\| sonnet \|/);
    assert.match(claudeRows[0], /claude-sonnet-4-5/);
    assert.match(claudeRows[0], /\| 1 \| 35 \| 4 \| 2 \|/);
    assert.match(markdown, /12 % → 14 % \(\+2 %\)/);
    assert.match(markdown, /30 % → 40 % \(\+10 %\)/);
    assert.match(markdown, /10 % → 12 % \(\+2 %\)/);
    assert.match(markdown, /\$9\.81 → \$9\.79/);
    assert.match(markdown, /не снят/);
    assert.match(markdown, /\| grok \| 1 \| 3 \| 100 \| 40 \| 7 \| \$0\.5000 \|/);
    assert.match(markdown, /\| deepseek \| 1 \| 1 \| 8 \| 2 \| 0 \| \$0\.0010 \|/);
    assert.match(markdown, /\| sonnet \| 1 \| 1 \| 35 \| 4 \| 2 \| — \|/);
});
