// Add diamond-bullet detections that sibling captures of the same screen
// already label. Proof of omission: the shape classifier measured the glyph at
// the same coordinates in this capture's pixels, and sibling captures label
// the identical glyph at the identical position with annotation notes.
// Usage: bun scripts/add-omitted-diamond-labels.ts
import { readFileSync, writeFileSync, appendFileSync } from "fs";

const cases: { target: string; donor: string; text: string }[] = [
  { target: "capture-1780033100069", donor: "capture-1780033102827", text: "◆" },
  ...[
    "capture-1780033513272",
    "capture-1780033515690",
    "capture-1780033517770",
    "capture-1780033519935",
    "capture-1780033521936",
    "capture-1780033524277",
    "capture-1780033526221",
  ].map((target) => ({ target, donor: "capture-1780033528358", text: "◇" })),
];

const logLines: string[] = [];
logLines.push(`\n## 2026-06-09 — omitted diamond-bullet detections restored\n`);
logLines.push(
  "Sibling captures of the same screens label these bullets (with notes like\n" +
    '"Visible outline diamond bullet before the mechanics description"), and the\n' +
    "glyph classifier measured the same glyph at the same coordinates in each\n" +
    "of these captures' pixels. The detection was copied verbatim from the\n" +
    "sibling label (donor noted per entry).\n",
);

let added = 0;
for (const c of cases) {
  const targetPath = `eval/wuthering-waves/${c.target}/eval.json`;
  const donorPath = `eval/wuthering-waves/${c.donor}/eval.json`;
  const target = JSON.parse(readFileSync(targetPath, "utf8"));
  const donor = JSON.parse(readFileSync(donorPath, "utf8"));
  const donorDet = donor.detections.find((d: any) => d.text === c.text);
  if (!donorDet) {
    console.error(`donor ${c.donor} has no ${c.text} detection`);
    continue;
  }
  if (target.detections.some((d: any) => d.text === c.text)) {
    console.log(`${c.target} already has ${c.text}; skipping`);
    continue;
  }
  const det = JSON.parse(JSON.stringify(donorDet));
  const baseId = det.id;
  let id = baseId;
  let n = 1;
  while (target.detections.some((d: any) => d.id === id)) id = `${baseId}_${n++}`;
  if (id !== det.id) {
    for (const ch of det.characters) if (ch.token_id) ch.token_id = ch.token_id; // token ids are detection-scoped; keep
    det.id = id;
  }
  // Insert after the detection that precedes it in the donor, else append.
  target.detections.push(det);
  writeFileSync(targetPath, JSON.stringify(target, null, 2) + "\n");
  added++;
  logLines.push(
    `- \`${targetPath}\`: added "${c.text}" detection (id \`${det.id}\`, bounds ` +
      `(${det.bounds.x}, ${det.bounds.y}, ${det.bounds.width}, ${det.bounds.height})) copied from \`${c.donor}\`.`,
  );
}

appendFileSync("eval/LABEL-CHANGES.md", logLines.join("\n") + "\n");
console.log(`added ${added} diamond detections; logged to eval/LABEL-CHANGES.md`);
