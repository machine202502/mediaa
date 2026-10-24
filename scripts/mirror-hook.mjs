import path from "node:path";
import { execFileSync, spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const script = path.join(here, "subagent-log.mjs");

const MARKERS = ["claude", "grok"];
const SYSTEM = new Set([
    "explorer.exe",
    "services.exe",
    "svchost.exe",
    "wininit.exe",
    "winlogon.exe",
    "csrss.exe",
    "smss.exe",
    "system",
    "registry",
    "memcompression",
    "msmpeng.exe",
    "taskhostw.exe",
    "sihost.exe",
    "runtimebroker.exe",
    "dllhost.exe",
    "conhost.exe",
    "searchhost.exe",
    "startmenuexperiencehost.exe",
    "shellhost.exe",
]);

export function parseChain(text) {
    return String(text || "")
        .split("\n")
        .map((line) => line.trim())
        .filter(Boolean)
        .map((line) => {
            const [pid, name, parent] = line.split("|");
            return { pid: Number(pid), name: String(name || "").trim(), parent: Number(parent) };
        })
        .filter((entry) => Number.isInteger(entry.pid) && entry.pid > 0);
}

export function pickOwner(chain, markers = MARKERS, system = SYSTEM) {
    const marked = chain.find((entry) => markers.some((marker) => String(entry.name).toLowerCase().includes(marker)));
    if (marked) return marked.pid;
    const usable = chain.filter((entry) => !system.has(String(entry.name).toLowerCase()));
    return usable.length > 0 ? usable[usable.length - 1].pid : 0;
}

const WINDOWS_QUERY = [
    "$p = $PID",
    "for ($i = 0; $i -lt 12 -and $p; $i++) {",
    '  $x = Get-CimInstance Win32_Process -Filter "ProcessId=$p" -ErrorAction SilentlyContinue',
    "  if (-not $x) { break }",
    '  "$($x.ProcessId)|$($x.Name)|$($x.ParentProcessId)"',
    "  $p = $x.ParentProcessId",
    "}",
].join("; ");

function windowsChain() {
    try {
        return parseChain(execFileSync("powershell", ["-NoProfile", "-NonInteractive", "-Command", WINDOWS_QUERY], { encoding: "utf8", windowsHide: true, timeout: 10000 }));
    } catch {
        return [];
    }
}

function posixChain() {
    const chain = [];
    let pid = process.ppid;
    for (let i = 0; i < 12 && pid > 1; i++) {
        let line = "";
        try {
            line = execFileSync("ps", ["-o", "ppid=,comm=", "-p", String(pid)], { encoding: "utf8" }).trim();
        } catch {
            break;
        }
        if (!line) break;
        const [parent, ...rest] = line.split(/\s+/);
        chain.push({ pid, name: rest.join(" "), parent: Number(parent) });
        pid = Number(parent);
    }
    return chain;
}

function chain() {
    return process.platform === "win32" ? windowsChain() : posixChain();
}

export function start(chainEntries, execPath = process.execPath, run = spawnSync) {
    const owner = pickOwner(chainEntries);
    const args = owner > 0 ? [script, "watch", "--detach", "--owner", String(owner)] : [script, "watch", "--detach"];
    run(execPath, args, { stdio: "ignore", windowsHide: true, timeout: 10000 });
    return owner;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
    const entries = chain();
    if (process.argv.includes("--dry")) {
        for (const entry of entries) console.log(`${entry.pid} ${entry.name}`);
        console.log(`owner ${pickOwner(entries)}`);
    } else {
        start(entries);
    }
}
