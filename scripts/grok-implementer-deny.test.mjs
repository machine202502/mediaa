// The implementer's command guard matches scripts/subagent.sh --deny.
// Run: node --test scripts/grok-implementer-deny.test.mjs

import test from "node:test";
import assert from "node:assert/strict";
import { denialReason } from "./grok-implementer-deny.mjs";

test("allows ordinary work", () => {
    for (const command of ["git status --short", "git diff", "cargo test --release -q --workspace", "node scripts/subagent-log.mjs once", "Get-ChildItem"]) {
        assert.equal(denialReason(command), "", command);
    }
});

test("denies commits, destructive git, cargo run, delete, and nested agents", () => {
    const denied = [
        "git add -A",
        "git commit -m x",
        "git -C C:\\repo push",
        "git status; git reset --hard",
        "cargo run --release -p mediaa",
        "cargo install somepkg",
        "Remove-Item foo.txt",
        "del foo.txt",
        "Stop-Process -Id 1",
        "npm install",
        "grok -p hi",
        "claude -p hi",
    ];
    for (const command of denied) assert.notEqual(denialReason(command), "", command);
});
