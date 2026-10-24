#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const hooks = ["pre-commit", "pre-push", "pre-rebase", "pre-merge-commit"];

try {
    execFileSync("git", ["config", "core.hooksPath", ".githooks"], { cwd: root, stdio: "inherit" });
    for (const hook of hooks) {
        try {
            fs.chmodSync(path.join(root, ".githooks", hook), 0o755);
        } catch {}
    }
    const current = execFileSync("git", ["config", "--get", "core.hooksPath"], { cwd: root, encoding: "utf8" }).trim();
    process.stdout.write(`git hooks: core.hooksPath = ${current}\n`);
} catch (error) {
    process.stderr.write(`git hooks: could not install (${error.message})\n`);
    process.exit(1);
}
