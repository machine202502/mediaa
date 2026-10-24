#!/usr/bin/env node

import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

export const QUESTION =
    "Вопрос пользователю: человек дал явное согласие на %ACTION% (%WHERE%)? Ответ «да» или «нет».";

export const POINTER = [
    "Прочитать перед ответом: CLAUDE.md → Rules → «Never push until the user says so»",
    "и workflow/lead.md → «Коммиты и пуш».",
    "«Да» разрешает ровно один git-вызов: следующий коммит или пуш снова требует «да».",
    "Согласие даётся именно на этот коммит и эту ветку, а не «на пуши вообще».",
].join("\n");

const NO_TTY =
    "Хук git не смог спросить пользователя: нет терминала. Спроси пользователя сам, дождись «да» " +
    "и только тогда повтори с подтверждением: node scripts/git-consent.mjs approve %OP% --answer да";

const ANSWERS = new Set(["да", "yes", "y", "д"]);

function git(root, args, input) {
    return execFileSync("git", args, {
        cwd: root,
        encoding: "utf8",
        input,
        stdio: [input === undefined ? "ignore" : "pipe", "pipe", "ignore"],
    }).trim();
}

export function root(cwd = process.cwd()) {
    return git(cwd, ["rev-parse", "--show-toplevel"]);
}

export function gitDir(rootDir) {
    const dir = git(rootDir, ["rev-parse", "--absolute-git-dir"]);
    return dir;
}

export function facts(rootDir) {
    const branch = git(rootDir, ["rev-parse", "--abbrev-ref", "HEAD"]);
    const commit = git(rootDir, ["rev-parse", "--short", "HEAD"]);
    return { branch, commit };
}

export function parsePushRefs(text) {
    return String(text || "")
        .split(/\r?\n/)
        .map((line) => line.trim())
        .filter(Boolean)
        .map((line) => {
            const [localRef, localSha, remoteRef, remoteSha] = line.split(/\s+/);
            return { localRef, localSha, remoteRef, remoteSha };
        });
}

export function questionFor(op, where) {
    return QUESTION.replace("%ACTION%", opName(op)).replace("%WHERE%", where || "репозиторий");
}

function opName(op) {
    return op === "push" ? "пуш" : `git ${op}`;
}

export function consentPath(rootDir) {
    return path.join(gitDir(rootDir), "consent.json");
}

export function logPath(rootDir) {
    return path.join(gitDir(rootDir), "consent.log");
}

export function matches(record, op, where) {
    if (!record || record.op !== op) return false;
    if (record.branch !== where.branch) return false;
    if (where.commit && record.commit !== where.commit) return false;
    if (where.content && record.content !== where.content) return false;
    return true;
}

export function readConsent(rootDir) {
    try {
        return JSON.parse(fs.readFileSync(consentPath(rootDir), "utf8"));
    } catch {
        return null;
    }
}

export function writeConsent(rootDir, record) {
    fs.writeFileSync(consentPath(rootDir), `${JSON.stringify(record, null, 2)}\n`);
    fs.appendFileSync(
        logPath(rootDir),
        `${new Date().toISOString()} ${record.op} ${record.branch} ${record.commit} ${record.answer}\n`,
    );
}

export function consumeConsent(rootDir) {
    try {
        fs.unlinkSync(consentPath(rootDir));
    } catch {}
}

export function whereForPush(rootDir, refs) {
    const { branch } = facts(rootDir);
    const shas = refs.filter((r) => r.localSha && !/^0+$/.test(r.localSha)).map((r) => r.localSha.slice(0, 7));
    const remote = refs[0]?.remoteRef || "";
    const named = refs[0]?.localRef || "";
    const pushed = named.startsWith("refs/heads/") ? named.replace("refs/heads/", "") : branch;
    return { branch: pushed, commit: shas[0] || "", extra: shas.slice(1), remote };
}

export function whereForLocal(rootDir, op) {
    const { branch, commit } = facts(rootDir);
    if (op === "commit") return { branch, commit, content: stagedDigest(rootDir) };
    return { branch, commit };
}

export function stagedDigest(rootDir) {
    try {
        const diff = execFileSync("git", ["diff", "--cached", "--binary"], {
            cwd: rootDir,
            encoding: "utf8",
            stdio: ["ignore", "pipe", "ignore"],
            maxBuffer: 64 * 1024 * 1024,
        });
        return createHash("sha1").update(diff).digest("hex").slice(0, 8);
    } catch {
        return "";
    }
}

export function describe(where) {
    const bits = [];
    if (where.branch) bits.push(`ветка ${where.branch}`);
    if (where.commit) bits.push(`коммит ${where.commit}`);
    if (where.content) bits.push(`изменения ${where.content}`);
    return bits.join(", ") || "репозиторий";
}

export function check({ rootDir, op, where, ask, say }) {
    const existing = readConsent(rootDir);
    if (matches(existing, op, where)) {
        consumeConsent(rootDir);
        say(`${opName(op)}: согласие на ${describe(where)} принято`);
        return 0;
    }
    const whereText = describe(where);
    const question = questionFor(op, whereText);
    const answer = ask(question);
    if (answer === null) {
        say(`${question}\n${POINTER}\n${NO_TTY.replace("%OP%", op)}`);
        return 1;
    }
    if (!ANSWERS.has(answer.trim().toLowerCase())) {
        say(`${opName(op)} отменён: пользователь не дал согласия (${describe(where)}).`);
        return 1;
    }
    writeConsent(rootDir, { op, branch: where.branch, commit: where.commit, content: where.content, answer: answer.trim(), at: new Date().toISOString() });
    consumeConsent(rootDir);
    say(`${opName(op)}: пользователь ответил «да» на ${whereText}`);
    return 0;
}

function askOnConsole(question) {
    const fd = openConsole();
    if (fd === null) return null;
    try {
        const prompt = `\n${question}\n${POINTER}\n\nОтвет (да/нет): `;
        fs.writeSync(fd, prompt);
        const buffer = Buffer.alloc(1);
        let line = "";
        while (true) {
            const read = fs.readSync(fd, buffer, 0, 1, null);
            if (read === 0) break;
            const ch = buffer.toString("utf8");
            if (ch === "\r" || ch === "\n") break;
            line += ch;
        }
        fs.writeSync(fd, "\n");
        return line;
    } catch {
        return null;
    } finally {
        try {
            fs.closeSync(fd);
        } catch {}
    }
}

function openConsole() {
    if (process.env.GIT_CONSENT_NO_TTY) return null;
    try {
        if (process.platform === "win32") return fs.openSync("CONIN$", "r");
        return fs.openSync("/dev/tty", "r");
    } catch {
        return null;
    }
}

export function run(argv) {
    const [command, rawOp, ...rest] = argv;
    const op = (rawOp || "").toLowerCase();
    const say = (text) => process.stderr.write(`${text}\n`);
    if (!["push", "commit", "rebase", "merge", "cherry-pick", "am"].includes(op)) {
        say("usage: git-consent.mjs check|approve <push|commit|rebase|merge|cherry-pick|am> [--answer да]");
        return 2;
    }
    const rootDir = root();
    if (command === "approve") {
        const at = rest.indexOf("--answer");
        const where = whereForLocal(rootDir, op);
        const answer = at >= 0 ? rest[at + 1] : askOnConsole(questionFor(op, describe(where)));
        if (answer === null || !ANSWERS.has(String(answer).trim().toLowerCase())) {
            say(`${questionFor(op, describe(where))}\n${POINTER}`);
            say("Согласие не записано: нужен ответ «да».");
            return 1;
        }
        writeConsent(rootDir, { op, branch: where.branch, commit: where.commit, content: where.content, answer: "да", at: new Date().toISOString() });
        say(`Записано согласие «да» на ${opName(op)} (${describe(where)}); действует на один вызов.`);
        return 0;
    }
    if (command === "status") {
        const record = readConsent(rootDir);
        say(record ? JSON.stringify(record) : "согласия нет");
        return 0;
    }
    if (command !== "check") {
        say("usage: git-consent.mjs check|approve|status <op>");
        return 2;
    }
    const input = process.stdin.isTTY ? "" : readStdin();
    const refs = op === "push" ? parsePushRefs(input) : [];
    if (op === "push" && refs.length === 0) return 0;
    const where = op === "push" ? whereForPush(rootDir, refs) : whereForLocal(rootDir, op);
    return check({ rootDir, op, where, ask: askOnConsole, say });
}

function readStdin() {
    try {
        return fs.readFileSync(0, "utf8");
    } catch {
        return "";
    }
}

const isMain = process.argv[1] && /git-consent\.mjs$/.test(process.argv[1].replace(/\\/g, "/"));

if (isMain) process.exit(run(process.argv.slice(2)));
