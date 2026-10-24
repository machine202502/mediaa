import test from "node:test";
import assert from "node:assert/strict";
import { describe, matches, parsePushRefs, questionFor, whereForPush } from "./git-consent.mjs";

test("the question is about one exact commit and points at the rules", () => {
    const text = questionFor("push", "ветка main, коммит 957e793");
    assert.match(text, /человек дал явное согласие/i);
    assert.match(text, /«да» или «нет»/i);
    assert.match(text, /ветка main/);
    assert.match(text, /коммит 957e793/);
});

test("a push line names the branch and the commit that goes out", () => {
    const refs = parsePushRefs("refs/heads/main 957e793fd51da10db3cb672acd4abb4b5708e379 refs/heads/main 0f13c6d\n");
    const where = whereForPush(".", refs);
    assert.equal(where.branch, "main");
    assert.equal(where.commit, "957e793");
});

test("deleting a remote branch does not count as a push of a commit", () => {
    const refs = parsePushRefs("(delete) 0000000000000000000000000000000000000000 refs/heads/old 0f13c6d\n");
    const where = whereForPush(".", refs);
    assert.equal(where.commit, "");
});

test("consent matches only the same branch and the same commit", () => {
    const record = { op: "push", branch: "main", commit: "957e793" };
    assert.ok(matches(record, "push", { branch: "main", commit: "957e793" }));
    assert.ok(!matches(record, "push", { branch: "main", commit: "aaaaaaa" }), "another commit");
    assert.ok(!matches(record, "push", { branch: "feature/agent", commit: "957e793" }), "another branch");
    assert.ok(!matches(record, "commit", { branch: "main", commit: "957e793" }), "another operation");
    assert.ok(!matches(null, "push", { branch: "main", commit: "957e793" }), "no consent at all");
});

test("a commit consent is bound to the staged changes too", () => {
    const record = { op: "commit", branch: "main", commit: "957e793", content: "11223344" };
    assert.ok(matches(record, "commit", { branch: "main", commit: "957e793", content: "11223344" }));
    assert.ok(!matches(record, "commit", { branch: "main", commit: "957e793", content: "55667788" }));
});

test("the question reads well without a commit", () => {
    assert.equal(describe({ branch: "main" }), "ветка main");
    assert.equal(describe({}), "репозиторий");
    assert.equal(describe({ branch: "main", commit: "abc1234", content: "11223344" }), "ветка main, коммит abc1234, изменения 11223344");
});
