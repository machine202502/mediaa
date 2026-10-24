import test from "node:test";
import assert from "node:assert/strict";
import path from "node:path";
import { parseChain, pickOwner, start } from "./mirror-hook.mjs";

const chain = parseChain(
    ["900|powershell.exe|800", "800|node.exe|700", "700|cmd.exe|600", "600|claude.exe|500", "500|explorer.exe|400"].join("\n"),
);

test("the hook chain keeps process ids, names and parents", () => {
    assert.deepEqual(chain[3], { pid: 600, name: "claude.exe", parent: 500 });
    assert.equal(chain.length, 5);
    assert.deepEqual(parseChain("bad line\n\n"), []);
});

test("the owner is the nearest Claude or Grok process in the chain", () => {
    assert.equal(pickOwner(chain), 600);
    assert.equal(pickOwner(parseChain("900|node.exe|800\n800|grok.exe|700\n700|explorer.exe|1")), 800);
});

test("without a known CLI the owner is the last process that is not part of the system", () => {
    const plain = parseChain("900|powershell.exe|800\n800|node.exe|700\n700|cmd.exe|600\n600|WindowsTerminal.exe|500\n500|explorer.exe|400");
    assert.equal(pickOwner(plain), 600);
    assert.equal(pickOwner(parseChain("900|powershell.exe|1\n1|explorer.exe|0")), 900);
    assert.equal(pickOwner([]), 0);
});

test("the hook starts a detached watcher owned by the CLI", () => {
    const calls = [];
    const run = (execPath, args, options) => calls.push({ execPath, args, options });

    assert.equal(start(chain, "C:\\node\\node.exe", run), 600);
    assert.equal(calls.length, 1);
    assert.equal(calls[0].execPath, "C:\\node\\node.exe");
    assert.deepEqual(calls[0].args.slice(1), ["watch", "--detach", "--owner", "600"]);
    assert.equal(calls[0].args[0], path.join(path.dirname(new URL(import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, "$1")), "subagent-log.mjs"));
    assert.equal(calls[0].options.stdio, "ignore");
});

test("an unknown owner leaves the old detached behaviour", () => {
    const calls = [];
    start([], "C:\\node\\node.exe", (execPath, args) => calls.push(args));

    assert.deepEqual(calls[0].slice(1), ["watch", "--detach"]);
});
