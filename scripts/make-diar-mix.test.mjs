import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { layout, readWav, rttm, sourceMarkdown, totalSeconds, writeWav } from "./make-diar-mix.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

function smallManifest(overrides = {}) {
  return {
    id: "mix-test",
    gap_seconds: 0.4,
    items: { item: { title: "Item", url: "https://archive.org/details/item", license: "public domain", license_url: "https://example.org" } },
    readers: { a: { name: "A", gender: "female" }, b: { name: "B", gender: "male" } },
    description: "two readers",
    pieces: {
      a1: { item: "item", file: "a.mp3", reader: "a", start: 10, duration: 5 },
      b1: { item: "item", file: "b.mp3", reader: "b", start: 20, duration: 12 },
    },
    timeline: [{ piece: "a1" }, { piece: "b1" }],
    ...overrides,
  };
}

test("pieces are placed with a gap and an overlap shortens the start", () => {
  const placed = layout(smallManifest());
  assert.deepEqual(
    placed.map((p) => [p.name, p.start, p.end]),
    [
      ["a1", 0, 5],
      ["b1", 5.4, 17.4],
    ],
  );
  const overlapping = layout(
    smallManifest({ timeline: [{ piece: "a1" }, { piece: "b1", overlap: 1.5 }] }),
  );
  assert.equal(overlapping[1].start, 3.5);
  assert.equal(totalSeconds(overlapping), 15.5);
});

test("bad pieces and overlaps are rejected", () => {
  assert.throws(() => layout(smallManifest({ timeline: [{ piece: "nope" }] })), /not defined/);
  assert.throws(
    () => layout(smallManifest({ timeline: [{ piece: "a1" }, { piece: "b1", overlap: 99 }] })),
    /not shorter/,
  );
  const unknownReader = smallManifest();
  unknownReader.pieces.a1.reader = "zzz";
  assert.throws(() => layout(unknownReader), /unknown reader/);
});

test("rttm has one line per piece with its reader and exact clock", () => {
  const manifest = smallManifest({ timeline: [{ piece: "a1" }, { piece: "b1", overlap: 1.5 }] });
  const lines = rttm(manifest, layout(manifest)).trim().split("\n");
  assert.equal(lines.length, 2);
  assert.equal(lines[0], "SPEAKER mix-test 1 0.000 5.000 <NA> <NA> a <NA> <NA>");
  assert.equal(lines[1], "SPEAKER mix-test 1 3.500 12.000 <NA> <NA> b <NA> <NA>");
});

test("source.md names both readers and the build command", () => {
  const manifest = smallManifest();
  const text = sourceMarkdown(manifest, layout(manifest));
  assert.match(text, /scripts\/make-diar-mix\.mjs/);
  assert.match(text, /\| `a` \| A \| female \|/);
  assert.match(text, /\| `b` \| B \| male \|/);
});

test("wav round-trip keeps the samples", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "diar-mix-"));
  const file = path.join(dir, "piece.wav");
  const samples = new Int16Array([0, 1234, -1234, 32767, -32768]);
  writeWav(file, samples, 16000);
  const wav = readWav(file);
  assert.equal(wav.rate, 16000);
  assert.deepEqual([...wav.samples], [...samples]);
  fs.rmSync(dir, { recursive: true, force: true });
});

test("the committed mix is four readers, ~11.5 minutes, with four overlaps and a monologue", () => {
  const manifest = JSON.parse(fs.readFileSync(path.join(root, "evals/data/librivox-mix-1/manifest.json"), "utf8"));
  const placed = layout(manifest);
  assert.equal(Object.keys(manifest.readers).length, 4);
  assert.equal(placed.length, 23);
  assert.equal(placed.filter((p) => p.overlap > 0).length, 4);
  assert.ok(placed.some((p) => p.reader === "mark-chulsky" && p.duration >= 180), "the long monologue");
  const total = totalSeconds(placed);
  assert.ok(total > 600 && total < 750, `total ${total}`);
  for (const piece of placed) {
    assert.ok(piece.duration >= 5 && piece.duration <= 240, `${piece.name}: ${piece.duration} s`);
    assert.ok(piece.start >= 0);
  }
});
