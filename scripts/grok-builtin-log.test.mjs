import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { grokBuiltinSources, startedAt } from "./subagent-log.mjs";

function write(file, text) {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, text);
}

test("mirrors a child session of this repo and skips another repo", () => {
    const home = fs.mkdtempSync(path.join(os.tmpdir(), "grok-sessions-"));
    const sessions = path.join(home, "sessions");
    const repo = "C:\\shared-data\\reps\\gifcap2";
    const other = "C:\\somewhere\\else";
    const parent = path.join(sessions, "encoded", "parent-session");
    const child = path.join(sessions, "encoded", "child-session");
    const stranger = path.join(sessions, "other", "stranger-child");
    write(path.join(parent, "summary.json"), JSON.stringify({ info: { id: "parent-session", cwd: repo } }));
    write(path.join(parent, "updates.jsonl"), "{}\n");
    write(
        path.join(parent, "subagents", "ag1", "meta.json"),
        JSON.stringify({
            subagent_id: "ag1",
            parent_session_id: "parent-session",
            child_session_id: "child-session",
            subagent_type: "implementer-low",
            description: "write the probe",
            prompt: "write target/probe.txt",
            child_cwd: repo,
            effective_model_id: "grok-4.7",
        }),
    );
    write(
        path.join(child, "updates.jsonl"),
        JSON.stringify({
            method: "session/update",
            params: { update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "ok" } } },
        }) + "\n",
    );
    write(path.join(child, "summary.json"), JSON.stringify({ current_model_id: "grok-4.7", reasoning_effort: "low" }));
    write(path.join(stranger, "summary.json"), JSON.stringify({ info: { id: "stranger-parent", cwd: other } }));
    write(path.join(stranger, "updates.jsonl"), "{}\n");
    const strangerParent = path.join(sessions, "other", "stranger-parent");
    write(path.join(strangerParent, "summary.json"), JSON.stringify({ info: { id: "stranger-parent", cwd: other } }));
    write(path.join(strangerParent, "updates.jsonl"), "{}\n");
    write(
        path.join(strangerParent, "subagents", "meta.json"),
        JSON.stringify({
            subagent_id: "ag2",
            parent_session_id: "stranger-parent",
            child_session_id: "stranger-child",
            description: "not ours",
            child_cwd: other,
        }),
    );

    const found = grokBuiltinSources(sessions, repo);
    assert.equal(found.length, 1);
    assert.equal(found[0].kind, "grok-builtin");
    assert.equal(found[0].sessionId, "child-session");
    assert.equal(found[0].agentType, "implementer-low");
    assert.equal(found[0].model, "grok-4.7");
    assert.equal(found[0].effort, "low");
    assert.equal(found[0].title, "write the probe");
    assert.equal(found[0].task, "write target/probe.txt");
    assert.equal(path.basename(found[0].file), "updates.jsonl");
});

test("a grok timestamp in seconds is a real date", () => {
    const seconds = startedAt(1792662359, 0);
    assert.equal(seconds.toISOString(), "2026-10-22T09:45:59.000Z");
    assert.equal(startedAt(1792662359000, 0).toISOString(), "2026-10-22T09:45:59.000Z");
    assert.equal(startedAt("2026-10-22T09:45:48.523Z", 0).toISOString(), "2026-10-22T09:45:48.523Z");
});
