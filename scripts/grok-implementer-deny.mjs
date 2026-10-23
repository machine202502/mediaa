#!/usr/bin/env node

import fs from "node:fs";

const GIT_BAD = new Set(["commit", "add", "reset", "push", "stash", "checkout", "restore", "rm", "clean", "rebase", "merge", "switch"]);

function words(segment) {
    return String(segment)
        .trim()
        .replace(/^&\s*/, "")
        .split(/\s+/)
        .filter(Boolean);
}

export function denialReason(command) {
    const segments = String(command || "").split(/&&|\|\||[;\n|]/);
    for (const segment of segments) {
        const raw = words(segment);
        let i = 0;
        while (i < raw.length && /^[A-Za-z_][A-Za-z0-9_]*=/.test(raw[i])) i++;
        const cmd = (raw[i] || "").replace(/^["']|["']$/g, "").toLowerCase();
        const rest = raw.slice(i + 1);
        const bare = cmd.replace(/\.exe$/, "");
        if (bare === "git") {
            let sub = "";
            for (let k = 0; k < rest.length; k++) {
                const w = rest[k].toLowerCase();
                if ((w === "-c" || w === "-C") && rest[k + 1]) {
                    k++;
                    continue;
                }
                if (w.startsWith("-")) continue;
                sub = w.replace(/^["']|["']$/g, "");
                break;
            }
            if (GIT_BAD.has(sub)) return `git ${sub} is denied for an implementer`;
        }
        if (bare === "cargo") {
            const sub = rest.map((w) => w.toLowerCase()).find((w) => !w.startsWith("-"));
            if (sub === "run" || sub === "install") return `cargo ${sub} is denied for an implementer`;
        }
        if (["rm", "rmdir", "del", "erase", "remove-item", "kill", "taskkill", "stop-process", "npm", "npx", "pip", "winget", "choco", "scoop"].includes(bare)) {
            return `${bare} is denied for an implementer`;
        }
        if (bare === "grok") {
            const flag = rest.map((w) => w.toLowerCase()).find((w) => w === "-p" || w === "--prompt" || w.startsWith("--prompt="));
            if (flag) return "grok -p / --prompt is denied for an implementer";
        }
        if (bare === "claude") return "claude is denied for an implementer";
    }
    return "";
}

function readStdin() {
    try {
        return fs.readFileSync(0, "utf8");
    } catch {
        return "";
    }
}

function commandOf(event) {
    const input = event?.toolInput || event?.tool_input || {};
    if (typeof input === "string") return input;
    if (typeof input.command === "string") return input.command;
    if (typeof input.cmd === "string") return input.cmd;
    return "";
}

const isMain = process.argv[1] && /grok-implementer-deny\.mjs$/.test(process.argv[1].replace(/\\/g, "/"));

if (isMain) {
    const raw = readStdin();
    let event;
    try {
        event = JSON.parse(raw);
    } catch {
        process.stderr.write("implementer deny: unreadable hook input\n");
        process.exit(2);
    }
    const command = commandOf(event);
    const tool = String(event.toolName || event.tool_name || "");
    if (!command) {
        if (/run_terminal_command|^bash$/i.test(tool)) {
            process.stderr.write("implementer deny: shell call has no command\n");
            process.exit(2);
        }
        process.exit(0);
    }
    const reason = denialReason(command);
    if (reason) process.stdout.write(JSON.stringify({ decision: "deny", reason }));
    process.exit(0);
}
