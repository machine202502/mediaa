import fs from "node:fs";
import path from "node:path";
import { spawn } from "node:child_process";

let Plugin;
try {
    ({ Plugin } = await import("@opencode/plugin"));
} catch {}

const SCRIPT = path.join("scripts", "subagent-log.mjs");
const NODE_NAME = process.platform === "win32" ? "node.exe" : "node";

export function nodeBinary({ execPath = process.execPath, exists = (p) => fs.existsSync(p), pathValue = process.env.PATH || "" } = {}) {
    const fromPath = pathValue
        .split(path.delimiter)
        .filter(Boolean)
        .map((dir) => path.join(dir, NODE_NAME))
        .find((candidate) => exists(candidate));
    if (fromPath) return fromPath;
    const nvm = path.join(process.env.LOCALAPPDATA || "", "Author Software", "nvm", ".nodejs", NODE_NAME);
    if (exists(nvm)) return nvm;
    return execPath;
}

export function startMirror({ directory, ownerPid, execPath = process.execPath, exists = (p) => fs.existsSync(p), spawnFn = spawn }) {
    const script = path.join(directory, SCRIPT);
    if (!exists(script)) return null;
    const child = spawnFn(nodeBinary({ execPath, exists }), [script, "watch", "--owner", String(ownerPid)], {
        cwd: directory,
        detached: false,
        stdio: "ignore",
        windowsHide: true,
    });
    child.on?.("error", () => {});
    child.unref?.();
    return child;
}

let running = null;

const definition = {
    id: "subagents-mirror",
    setup(ctx) {
        if (running) return;
        const directory = ctx?.location?.directory || ctx?.directory || process.cwd();
        running = startMirror({ directory, ownerPid: process.pid });
        return () => {
            try {
                running?.kill();
            } catch {}
            running = null;
        };
    },
};

export default Plugin?.define ? Plugin.define(definition) : definition;
