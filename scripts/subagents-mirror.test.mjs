import test from "node:test";
import assert from "node:assert/strict";
import path from "node:path";
import { nodeBinary, startMirror } from "../.opencode/plugins/subagents-mirror/mirror.js";

const directory = "C:\\repo";

test("the mirror starts the watcher with the process that owns it", () => {
    const calls = [];
    const child = { on() {}, unref() {}, kill() {} };
    const spawnFn = (bin, args, options) => {
        calls.push({ bin, args, options });
        return child;
    };

    assert.equal(startMirror({ directory, ownerPid: 4242, execPath: "C:\\node\\node.exe", exists: (p) => p === path.join(directory, "scripts", "subagent-log.mjs"), spawnFn }), child);
    assert.equal(calls.length, 1);
    assert.equal(calls[0].bin, "C:\\node\\node.exe");
    assert.deepEqual(calls[0].args, [path.join(directory, "scripts", "subagent-log.mjs"), "watch", "--owner", "4242"]);
    assert.equal(calls[0].options.cwd, directory);
    assert.equal(calls[0].options.detached, false);
});

test("no watcher starts outside a repository that has the mirror script", () => {
    let spawned = 0;
    const spawnFn = () => {
        spawned += 1;
    };

    assert.equal(startMirror({ directory, ownerPid: 1, exists: () => false, spawnFn }), null);
    assert.equal(spawned, 0);
});

test("node comes from PATH before the runtime that runs the plugin", () => {
    const onPath = path.join("C:\\nodejs", process.platform === "win32" ? "node.exe" : "node");

    assert.equal(nodeBinary({ execPath: "C:\\bun\\bun.exe", exists: (p) => p === onPath, pathValue: "C:\\nodejs" }), onPath);
    assert.equal(nodeBinary({ execPath: "C:\\bun\\bun.exe", exists: () => false, pathValue: "" }), "C:\\bun\\bun.exe");
});
