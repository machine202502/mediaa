import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const DEFAULT_MANIFEST = "evals/data/librivox-mix-1/manifest.json";
const SAMPLE_RATE = 16000;

export function layout(manifest) {
  const gap = manifest.gap_seconds ?? 0.4;
  const placed = [];
  let lastEnd = null;
  let lastDuration = null;
  for (const [index, step] of manifest.timeline.entries()) {
    const piece = manifest.pieces[step.piece];
    if (!piece) throw new Error(`piece ${step.piece} (#${index + 1}) is not defined`);
    if (!manifest.readers[piece.reader]) throw new Error(`piece ${step.piece}: unknown reader ${piece.reader}`);
    if (!manifest.items[piece.item]) throw new Error(`piece ${step.piece}: unknown item ${piece.item}`);
    if (!(piece.duration > 0)) throw new Error(`piece ${step.piece}: duration must be positive`);
    if (!(piece.start >= 0)) throw new Error(`piece ${step.piece}: start must not be negative`);
    let start;
    if (lastEnd === null) {
      start = 0;
    } else if (step.overlap !== undefined) {
      if (!(step.overlap > 0)) throw new Error(`piece ${step.piece}: overlap must be positive`);
      if (step.overlap >= lastDuration) throw new Error(`piece ${step.piece}: overlap ${step.overlap} s is not shorter than the previous piece`);
      start = lastEnd - step.overlap;
    } else {
      start = lastEnd + gap;
    }
    const end = start + piece.duration;
    placed.push({ ...piece, name: step.piece, reader: piece.reader, sourceStart: piece.start, start, end, overlap: step.overlap ?? 0 });
    lastEnd = Math.max(lastEnd ?? 0, end);
    lastDuration = piece.duration;
  }
  return placed;
}

export function totalSeconds(placed) {
  return placed.reduce((end, piece) => Math.max(end, piece.end), 0);
}

export function rttm(manifest, placed) {
  return placed
    .slice()
    .sort((a, b) => a.start - b.start || a.reader.localeCompare(b.reader))
    .map((p) => `SPEAKER ${manifest.id} 1 ${p.start.toFixed(3)} ${p.duration.toFixed(3)} <NA> <NA> ${p.reader} <NA> <NA>`)
    .join("\n") + "\n";
}

export function sourceMarkdown(manifest, placed) {
  const items = Object.entries(manifest.items)
    .map(([id, item]) => `| \`${id}\` | ${item.title} | ${item.url} | ${item.license} (${item.license_url}) |`)
    .join("\n");
  const readers = Object.entries(manifest.readers)
    .map(([id, reader]) => `| \`${id}\` | ${reader.name} | ${reader.gender} |`)
    .join("\n");
  const pieces = placed
    .map((p, i) => `| ${i + 1} | ${p.name} | ${manifest.readers[p.reader].name} | ${p.file} | ${p.sourceStart.toFixed(1)}–${(p.sourceStart + p.duration).toFixed(1)} | ${p.start.toFixed(1)}–${p.end.toFixed(1)} | ${p.overlap ? `${p.overlap} s` : "—"} |`)
    .join("\n");
  return `# ${manifest.title}

* Лицензия: ${manifest.items[Object.keys(manifest.items)[0]].license} — только такой материал
  кладём в репозиторий.
* Это **синтетическая беседа** с точным эталоном: куски четырёх чтецов одного
  обще-достояние-аудиокниги положены на одну шкалу времени. Каждый кусок принадлежит одному
  известному чтецу, поэтому \`speakers.rttm\` верен по построению, а не разметкой на слух.
* ${manifest.description}
* Сборка: \`node scripts/make-diar-mix.mjs\` (нужен \`ffmpeg\` в PATH или в \`FFMPEG\`; исходные
  mp3 кэшируются в \`target/diar-src/\`, в репозиторий попадает только \`audio.opus\`).
  Только план, без звука: \`node scripts/make-diar-mix.mjs --plan\`.
* Формат: моно, 16 кГц, Opus ${manifest.bitrate}, длительность ${totalSeconds(placed).toFixed(1)} с.

## Источники

| Идентификатор archive.org | Название | Страница | Лицензия |
|---|---|---|---|
${items}

## Чтецы

| Ключ | Имя | Голос |
|---|---|---|
${readers}

## Куски и шкала времени

| № | Кусок | Чтец | Файл источника | Смещение в источнике, с | Время в записи, с | Перекрытие |
|---|---|---|---|---|---|---|
${pieces}
`;
}

async function download(url, out) {
  if (fs.existsSync(out) && fs.statSync(out).size > 0) return;
  const response = await fetch(url);
  if (!response.ok) throw new Error(`${url}: HTTP ${response.status}`);
  const bytes = Buffer.from(await response.arrayBuffer());
  if (bytes.length === 0) throw new Error(`${url}: empty response`);
  fs.mkdirSync(path.dirname(out), { recursive: true });
  fs.writeFileSync(out, bytes);
  console.log(`downloaded ${path.basename(out)} (${(bytes.length / 1048576).toFixed(1)} MB)`);
}

export function readWav(file) {
  const bytes = fs.readFileSync(file);
  if (bytes.toString("ascii", 0, 4) !== "RIFF" || bytes.toString("ascii", 8, 12) !== "WAVE") {
    throw new Error(`${file}: not a RIFF/WAVE file`);
  }
  let format = null;
  let data = null;
  let offset = 12;
  while (offset + 8 <= bytes.length) {
    const id = bytes.toString("ascii", offset, offset + 4);
    const size = bytes.readUInt32LE(offset + 4);
    const body = offset + 8;
    if (id === "fmt ") {
      format = {
        code: bytes.readUInt16LE(body),
        channels: bytes.readUInt16LE(body + 2),
        rate: bytes.readUInt32LE(body + 4),
        bits: bytes.readUInt16LE(body + 14),
      };
    } else if (id === "data") {
      data = bytes.subarray(body, Math.min(body + size, bytes.length));
    }
    offset = body + size + (size % 2);
  }
  if (!format || !data) throw new Error(`${file}: no fmt or data chunk`);
  if (format.code !== 1 || format.bits !== 16 || format.channels !== 1) {
    throw new Error(`${file}: expected 16-bit mono PCM, got code ${format.code}, ${format.bits} bits, ${format.channels} channels`);
  }
  return { rate: format.rate, samples: new Int16Array(data.buffer, data.byteOffset, data.length >> 1) };
}

export function writeWav(file, samples, rate) {
  const header = Buffer.alloc(44);
  header.write("RIFF", 0, "ascii");
  header.writeUInt32LE(36 + samples.length * 2, 4);
  header.write("WAVE", 8, "ascii");
  header.write("fmt ", 12, "ascii");
  header.writeUInt32LE(16, 16);
  header.writeUInt16LE(1, 20);
  header.writeUInt16LE(1, 22);
  header.writeUInt32LE(rate, 24);
  header.writeUInt32LE(rate * 2, 28);
  header.writeUInt16LE(2, 32);
  header.writeUInt16LE(16, 34);
  header.write("data", 36, "ascii");
  header.writeUInt32LE(samples.length * 2, 40);
  const body = Buffer.alloc(samples.length * 2);
  for (let i = 0; i < samples.length; i++) body.writeInt16LE(samples[i], i * 2);
  fs.writeFileSync(file, Buffer.concat([header, body]));
}

function ffmpeg(args) {
  const binary = process.env.FFMPEG || "ffmpeg";
  execFileSync(binary, ["-y", "-v", "error", ...args], { stdio: "inherit" });
}

async function build(manifestPath) {
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  const dir = path.dirname(manifestPath);
  const placed = layout(manifest);
  const rate = manifest.sample_rate ?? SAMPLE_RATE;
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(dir, "speakers.rttm"), rttm(manifest, placed));
  fs.writeFileSync(path.join(dir, "source.md"), sourceMarkdown(manifest, placed));
  console.log(`${manifest.id}: ${placed.length} pieces, ${totalSeconds(placed).toFixed(1)} s, ${Object.keys(manifest.readers).length} readers`);

  const buildDir = path.join(root, "target", "diar-src", "build", manifest.id);
  fs.mkdirSync(buildDir, { recursive: true });
  for (const piece of placed) {
    const item = manifest.items[piece.item];
    const source = path.join(root, "target", "diar-src", piece.item, piece.file);
    await download(`${item.url.replace("/details/", "/download/")}/${encodeURIComponent(piece.file)}`, source);
    const cut = path.join(buildDir, `${piece.name}.wav`);
    ffmpeg(["-ss", String(piece.sourceStart), "-t", String(piece.duration), "-i", source, "-vn", "-ac", "1", "-ar", String(rate), "-sample_fmt", "s16", "-f", "wav", cut]);
  }

  const total = Math.ceil(totalSeconds(placed) * rate);
  const master = new Int32Array(total);
  for (const piece of placed) {
    const { rate: pieceRate, samples } = readWav(path.join(buildDir, `${piece.name}.wav`));
    if (pieceRate !== rate) throw new Error(`${piece.name}: ${pieceRate} Hz, expected ${rate} Hz`);
    const at = Math.round(piece.start * rate);
    for (let i = 0; i < samples.length && at + i < total; i++) master[at + i] += samples[i];
  }
  const mixed = new Int16Array(total);
  for (let i = 0; i < total; i++) mixed[i] = Math.max(-32768, Math.min(32767, master[i]));
  const masterWav = path.join(buildDir, "master.wav");
  writeWav(masterWav, mixed, rate);

  const audio = path.join(dir, "audio.opus");
  ffmpeg(["-i", masterWav, "-ac", "1", "-ar", String(rate), "-c:a", "libopus", "-b:a", manifest.bitrate ?? "32k", "-application", "voip", audio]);
  const size = fs.statSync(audio).size;
  if (size > 5 * 1024 * 1024) throw new Error(`${audio}: ${(size / 1048576).toFixed(1)} MB is over the 5 MB budget`);
  console.log(`${audio}: ${(size / 1048576).toFixed(2)} MB`);
}

async function main() {
  const args = process.argv.slice(2);
  const plan = args.includes("--plan");
  const manifestArg = args.find((arg) => arg !== "--plan");
  const manifestPath = path.resolve(root, manifestArg ?? DEFAULT_MANIFEST);
  if (plan) {
    const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
    const placed = layout(manifest);
    const dir = path.dirname(manifestPath);
    fs.writeFileSync(path.join(dir, "speakers.rttm"), rttm(manifest, placed));
    fs.writeFileSync(path.join(dir, "source.md"), sourceMarkdown(manifest, placed));
    console.log(`${manifest.id}: ${placed.length} pieces, ${totalSeconds(placed).toFixed(1)} s (plan only: speakers.rttm and source.md)`);
    return;
  }
  await build(manifestPath);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((error) => {
    console.error(`make-diar-mix: ${error.message}`);
    process.exit(1);
  });
}
