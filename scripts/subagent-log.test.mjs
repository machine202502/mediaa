import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { rewriteRunLinks, repairDocLinks, renderGrokLog, grokRunFacts, followTick, claudeProjectDirs, ownerGone, sameProject } from "./subagent-log.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));

function tempRepo() {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), "subagent-log-"));
    fs.mkdirSync(path.join(root, "workflow"), { recursive: true });
    return root;
}

function write(root, rel, text) {
    const file = path.join(root, rel);
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, text);
}

function read(root, rel) {
    return fs.readFileSync(path.join(root, rel), "utf8");
}

test("archive rewrites the run's log and artifacts links only", () => {
    const root = tempRepo();
    write(
        root,
        "workflow/process.md",
        [
            "# Doc",
            "* [лог](../subagents/run1.md)",
            "* [артефакты](../subagents/artifacts/run1/report.md)",
            "* [другой запуск](../subagents/run1-long.md)",
            "* [другие артефакты](../subagents/artifacts/run1-long/report.md)",
            "* `subagents/run1.md` и `subagents/artifacts/run1`, просто run1.md",
            "",
        ].join("\n"),
    );
    write(root, "CLAUDE.md", "[лог](subagents/run1.md)\n[арт](subagents/artifacts/run1/report.md)\n");

    const changed = rewriteRunLinks(root, "run1.md", true);

    assert.equal(changed.length, 2);
    assert.deepEqual(
        read(root, "workflow/process.md").split("\n"),
        [
            "# Doc",
            "* [лог](../subagents/archive/run1.md)",
            "* [артефакты](../subagents/archive/artifacts/run1/report.md)",
            "* [другой запуск](../subagents/run1-long.md)",
            "* [другие артефакты](../subagents/artifacts/run1-long/report.md)",
            "* `subagents/archive/run1.md` и `subagents/archive/artifacts/run1`, просто run1.md",
            "",
        ],
    );
    assert.equal(read(root, "CLAUDE.md"), "[лог](subagents/archive/run1.md)\n[арт](subagents/archive/artifacts/run1/report.md)\n");
});

test("unarchive rewrites the run's links back and leaves live ones alone", () => {
    const root = tempRepo();
    write(
        root,
        "workflow/done.md",
        [
            "* [лог](../subagents/archive/run2.md)",
            "* [артефакты](../subagents/archive/artifacts/run2/report.md)",
            "* [уже живой](../subagents/run2.md)",
            "",
        ].join("\n"),
    );

    const changed = rewriteRunLinks(root, "run2", false);

    assert.equal(changed.length, 1);
    assert.equal(
        read(root, "workflow/done.md"),
        ["* [лог](../subagents/run2.md)", "* [артефакты](../subagents/artifacts/run2/report.md)", "* [уже живой](../subagents/run2.md)", ""].join("\n"),
    );
});

test("repair fixes both directions where only the other path exists", () => {
    const root = tempRepo();
    write(
        root,
        "workflow/process.md",
        [
            "* [лог, есть только архивный](../subagents/runArchived.md)",
            "* [лог, есть только живой](../subagents/archive/runLive.md)",
            "* [артефакты, есть только архивные](../subagents/artifacts/runArtifacts)",
            "* [существующий живой](../subagents/runBoth.md)",
            "* [нигде нет](../subagents/runNowhere.md)",
            "* [реестр](../subagents/registry.md)",
            "* [не лог](../subagents/notes.txt)",
            "",
        ].join("\n"),
    );
    write(root, "subagents/archive/runArchived.md", "archived\n");
    write(root, "subagents/runLive.md", "live\n");
    fs.mkdirSync(path.join(root, "subagents/archive/artifacts/runArtifacts"), { recursive: true });
    write(root, "subagents/runBoth.md", "live\n");
    write(root, "subagents/archive/registry.md", "archived registry\n");
    write(root, "subagents/notes.txt", "notes\n");

    const changed = repairDocLinks(root);

    assert.deepEqual(
        changed.map((f) => path.relative(root, f)),
        [path.join("workflow", "process.md")],
    );
    assert.deepEqual(
        read(root, "workflow/process.md").split("\n"),
        [
            "* [лог, есть только архивный](../subagents/archive/runArchived.md)",
            "* [лог, есть только живой](../subagents/runLive.md)",
            "* [артефакты, есть только архивные](../subagents/archive/artifacts/runArtifacts)",
            "* [существующий живой](../subagents/runBoth.md)",
            "* [нигде нет](../subagents/runNowhere.md)",
            "* [реестр](../subagents/registry.md)",
            "* [не лог](../subagents/notes.txt)",
            "",
        ],
    );
});

test("files without changes are not written", () => {
    const root = tempRepo();
    write(root, "workflow/unrelated.md", "Просто текст без ссылок.\n");
    write(root, "workflow/process.md", "* [чужой лог](../subagents/other.md)\n");
    const before = fs.statSync(path.join(root, "workflow/unrelated.md")).mtimeMs;

    rewriteRunLinks(root, "run1", true, { log: true, artifacts: false });
    repairDocLinks(root);

    assert.equal(fs.statSync(path.join(root, "workflow/unrelated.md")).mtimeMs, before);
});

test("CRLF line endings are kept", () => {
    const root = tempRepo();
    write(root, "workflow/process.md", "начало\r\n[лог](../subagents/run1.md)\r\nконец\r\n");

    rewriteRunLinks(root, "run1", true);

    assert.equal(read(root, "workflow/process.md"), "начало\r\n[лог](../subagents/archive/run1.md)\r\nконец\r\n");

    write(root, "workflow/process.md", "начало\r\n[лог](../subagents/runLive.md)\r\nконец\r\n");
    write(root, "subagents/archive/runLive.md", "archived\r\n");

    repairDocLinks(root);

    assert.equal(read(root, "workflow/process.md"), "начало\r\n[лог](../subagents/archive/runLive.md)\r\nконец\r\n");
});

test("grok flat stream renders the message, the tool and the model", () => {
    const md = renderGrokLog([
        { type: "thought", data: "The user wants one word." },
        { type: "tool_call", toolCallId: "c1", toolName: "read_file", rawInput: { path: "a.txt" } },
        { type: "tool_call_update", toolCallId: "c1", status: "completed", rawOutput: { text: "hello" } },
        { type: "text", data: "pong" },
        {
            type: "end",
            stopReason: "end_turn",
            sessionId: "sess-1",
            num_turns: 1,
            duration_ms: 1200,
            modelUsage: { "grok-4.7-build": { inputTokens: 10, outputTokens: 1 } },
        },
    ]);
    assert.match(md, /думает/);
    assert.match(md, /The user wants one word/);
    assert.match(md, /вызывает `read_file`/);
    assert.match(md, /a\.txt/);
    assert.match(md, /hello/);
    assert.match(md, /пишет/);
    assert.match(md, /pong/);
    assert.match(md, /\*\*Модель:\*\* grok-4\.7-build/);
    assert.match(md, /Модель: grok-4\.7-build/);
    assert.match(md, /готово/);
    assert.doesNotMatch(md, /fast/i);
});

test("grok ACP stream renders chunks, a tool result and the final answer", () => {
    const md = renderGrokLog([
        {
            method: "session/update",
            params: {
                sessionId: "sess-acp",
                update: { sessionUpdate: "user_message_chunk", content: { type: "text", text: "Reply with the word pong." } },
            },
        },
        {
            method: "session/update",
            params: {
                sessionId: "sess-acp",
                update: { sessionUpdate: "agent_thought_chunk", content: { type: "text", text: "No tools." } },
            },
        },
        {
            method: "session/update",
            params: {
                sessionId: "sess-acp",
                update: {
                    sessionUpdate: "tool_call",
                    toolCallId: "call-1",
                    title: "list_dir",
                    rawInput: { target_directory: "src" },
                    _meta: { "x.ai/tool": { name: "list_dir" } },
                },
            },
        },
        {
            method: "session/update",
            params: {
                sessionId: "sess-acp",
                update: {
                    sessionUpdate: "tool_call_update",
                    toolCallId: "call-1",
                    status: "completed",
                    rawOutput: { type: "ListDir", Content: "src/main.rs" },
                },
            },
        },
        {
            method: "session/update",
            params: {
                sessionId: "sess-acp",
                update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "po" } },
            },
        },
        {
            method: "session/update",
            params: {
                sessionId: "sess-acp",
                update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "ng" } },
            },
        },
        {
            method: "session/update",
            params: {
                sessionId: "sess-acp",
                update: {
                    sessionUpdate: "turn_completed",
                    stop_reason: "end_turn",
                    elapsed_ms: 4000,
                    usage: { numTurns: 1, modelUsage: { "grok-4.7-build": { modelCalls: 1 } } },
                },
            },
        },
    ]);
    assert.match(md, /вызывает `list_dir`/);
    assert.match(md, /src\/main\.rs/);
    assert.match(md, /pong/);
    assert.match(md, /Модель: grok-4\.7-build/);
    assert.match(md, /готово/);
    assert.doesNotMatch(md, /Reply with the word pong/);
});

test("a fast model in grok usage fails the run facts", () => {
    const ok = grokRunFacts([], {
        sessionId: "s",
        session: { costUsdTicks: 34_086_0000, primaryModelId: "grok-4.7-build", modelUsage: { "grok-4.7-build": { modelCalls: 1 } }, inputTokens: 3, outputTokens: 1, reasoningTokens: 0, totalTokens: 4, modelCalls: 1 },
    });
    assert.deepEqual(ok.fast, []);
    assert.equal(ok.sessionUsd, 0.034086);
    assert.equal(ok.tokens.input_tokens, 3);

    const bad = grokRunFacts([], {
        session: { costUsdTicks: 10_000_000_000, modelUsage: { "grok-4.7-build-fast": { modelCalls: 1 } }, primaryModelId: "grok-4.7-build-fast" },
    });
    assert.deepEqual(bad.fast, ["grok-4.7-build-fast"]);
    assert.match(bad.warning, /FAST MODEL USED: grok-4\.7-build-fast/);
    assert.match(bad.warning, /subscription limits/);
    assert.equal(bad.sessionUsd, 1);
});

test("the grok smoke stream renders pong on grok-4.7-build", () => {
    const file = path.join(here, "fixtures", "grok-pong-stream.jsonl");
    const events = fs.readFileSync(file, "utf8").trim().split("\n").map((line) => JSON.parse(line));
    const md = renderGrokLog(events);
    assert.match(md, /### .*думает/);
    assert.match(md, /do not use tools/);
    assert.match(md, /### .*пишет\n\npong\n/);
    assert.match(md, /\*\*Модель:\*\* grok-4\.7-build/);
    assert.match(md, /\*\*Статус:\*\* готово/);
    assert.match(md, /## Итог\n\nМодель: grok-4\.7-build\nХодов: 1\.\n\npong\n/);
    assert.doesNotMatch(md, /fast/i);
    const facts = grokRunFacts(events, null);
    assert.deepEqual(facts.models, ["grok-4.7-build"]);
    assert.equal(facts.text, "pong");
    assert.equal(facts.subtype, "success");
    assert.deepEqual(facts.fast, []);
});

test("follow renders a grok stream as lines are appended", () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), "follow-grow-"));
    const dir = path.join(root, "2026-10-21_12-00_grok-4.7_follow-grow");
    const out = path.join(root, "logs");
    fs.mkdirSync(dir);
    fs.writeFileSync(path.join(dir, "provider"), "grok\n");
    fs.writeFileSync(path.join(dir, "model"), "grok-4.7\n");
    fs.writeFileSync(path.join(dir, "name"), "follow grow\n");
    fs.writeFileSync(path.join(dir, "task.md"), "say hello\n");
    const stream = path.join(dir, "stream.jsonl");
    const index = {};
    const tick = () => followTick(dir, { outDir: out, index, isolated: true, quiet: true });
    const readLog = () => {
        const name = fs.readdirSync(out).find((f) => f.endsWith(".md"));
        return fs.readFileSync(path.join(out, name), "utf8");
    };

    fs.appendFileSync(stream, `${JSON.stringify({ type: "thought", data: "first thought" })}\n`);
    fs.appendFileSync(stream, '{"type":"text","data":"ZZZpartial');
    let step = tick();
    let md = readLog();
    const dialog = () => md.split("## Ход работы")[1] || "";
    assert.match(dialog(), /first thought/);
    assert.doesNotMatch(dialog(), /ZZZpartial/);
    assert.equal(step.finished, false);
    const lines = md.split("\n").length;

    fs.appendFileSync(stream, '"}\n');
    fs.appendFileSync(stream, `${JSON.stringify({ type: "text", data: " second" })}\n`);
    tick();
    md = readLog();
    assert.match(dialog(), /ZZZpartial/);
    assert.match(dialog(), /second/);
    assert.ok(md.split("\n").length > lines);

    fs.appendFileSync(
        stream,
        `${JSON.stringify({ type: "end", stopReason: "end_turn", sessionId: "s", num_turns: 1 })}\n`,
    );
    fs.appendFileSync(stream, `${JSON.stringify({ type: "text", data: "next turn" })}\n`);
    step = tick();
    md = readLog();
    assert.match(md, /next turn/);
    assert.equal(step.finished, false);
    assert.match(md, /работает/);

    const child = spawn(process.execPath, ["-e", "setInterval(() => {}, 1000)"], { stdio: "ignore" });
    try {
        fs.writeFileSync(path.join(dir, "pid"), `${child.pid}\n`);
        fs.appendFileSync(
            stream,
            `${JSON.stringify({ type: "end", stopReason: "end_turn", sessionId: "s", num_turns: 2 })}\n`,
        );
        step = tick();
        md = readLog();
        assert.equal(step.finished, false);
        assert.match(md, /работает/);
        assert.match(md, /next turn/);
    } finally {
        child.kill();
    }
});

test("the exact Claude project directory comes first", () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), "claude-projects-"));
    const exact = path.join(root, "C--shared-data-reps--old-gifcap2");
    fs.mkdirSync(exact, { recursive: true });
    fs.writeFileSync(path.join(exact, "session.jsonl"), `${JSON.stringify({ cwd: "C:\\shared-data\\reps\\.old\\gifcap2" })}\n`);

    assert.deepEqual(claudeProjectDirs(root, "C:\\shared-data\\reps\\.old\\gifcap2"), [exact]);
});

test("a moved repository still finds its Claude sessions by folder name", () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), "claude-projects-"));
    const moved = path.join(root, "C--shared-data-reps-gifcap2");
    fs.mkdirSync(moved, { recursive: true });
    fs.writeFileSync(path.join(moved, "session.jsonl"), `${JSON.stringify({ cwd: "C:\\shared-data\\reps\\gifcap2" })}\n`);
    const other = path.join(root, "C--work-other-gifcap2");
    fs.mkdirSync(other, { recursive: true });
    fs.writeFileSync(path.join(other, "session.jsonl"), `${JSON.stringify({ cwd: "C:\\work\\other\\project" })}\n`);

    assert.deepEqual(claudeProjectDirs(root, "C:\\shared-data\\reps\\.old\\gifcap2"), [moved]);
});

test("the watcher stops when the process that started it is gone", () => {
    const gone = () => {
        const error = new Error("gone");
        error.code = "ESRCH";
        throw error;
    };
    const denied = () => {
        const error = new Error("denied");
        error.code = "EPERM";
        throw error;
    };

    assert.equal(ownerGone(123, gone), true);
    assert.equal(ownerGone(123, denied), false);
    assert.equal(ownerGone(123, () => {}), false);
    assert.equal(ownerGone(0, gone), false);
});

test("a session keeps looking for its working directory past the first line", () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), "claude-projects-"));
    const moved = path.join(root, "C--shared-data-reps-gifcap2");
    fs.mkdirSync(moved, { recursive: true });
    fs.writeFileSync(
        path.join(moved, "session.jsonl"),
        [
            JSON.stringify({ type: "queue-operation", operation: "enqueue" }),
            JSON.stringify({ type: "queue-operation", operation: "dequeue" }),
            JSON.stringify({ type: "user", cwd: "C:\\shared-data\\reps\\gifcap2" }),
            "",
        ].join("\n"),
    );

    assert.deepEqual(claudeProjectDirs(root, "C:\\shared-data\\reps\\.old\\gifcap2"), [moved]);
});

test("a project keeps its identity after the folder is moved", () => {
    assert.equal(sameProject("C:\\shared-data\\reps\\gifcap2", "C:\\shared-data\\reps\\.old\\gifcap2"), true);
    assert.equal(sameProject("C:\\shared-data\\reps\\gifcap2\\", "c:/shared-data/reps/.old/gifcap2"), true);
    assert.equal(sameProject("C:\\work\\other\\project", "C:\\shared-data\\reps\\.old\\gifcap2"), false);
    assert.equal(sameProject("", "C:\\shared-data\\reps\\.old\\gifcap2"), false);
});
