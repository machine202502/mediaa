// Tests for the links the log mirror keeps right when a run moves to subagents/archive/ and back.
// Run: node --test scripts/

import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { rewriteRunLinks, repairDocLinks } from "./subagent-log.mjs";

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
