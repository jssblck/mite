// Apply provable label-bounds fixes from the audit harness output.
// - Updates detection.bounds to the pixel-measured glyph box.
// - Affine-remaps each character's bounds from the old detection rect to the
//   new one (containment is preserved; char boxes are unscored metadata).
// - Appends an entry per change to eval/LABEL-CHANGES.md.
// Usage: bun scripts/apply-label-bounds-fixes.ts [auditJson]
import { readFileSync, writeFileSync, appendFileSync, existsSync } from "fs";

const auditPath = process.argv[2] ?? "target/eval/label-bounds-audit.json";
const findings = JSON.parse(readFileSync(auditPath, "utf8"));
const logPath = "eval/LABEL-CHANGES.md";

if (!existsSync(logPath)) {
  writeFileSync(
    logPath,
    "# Eval Label Changes\n\n" +
      "Record of manual label corrections, with the evidence that justified\n" +
      "each change. Labels are only edited when provably incorrect; see\n" +
      "docs/eval-metadata.md for the policy.\n",
  );
}

const byEval = new Map<string, any[]>();
for (const f of findings) {
  if (f.verdict !== "label-bounds-provably-wrong" || !f.proposed_bounds) continue;
  const list = byEval.get(f.eval) ?? [];
  list.push(f);
  byEval.set(f.eval, list);
}

let changed = 0;
const logLines: string[] = [];
const today = "2026-06-09";
logLines.push(`\n## ${today} — bounds corrections from pixel audit\n`);
logLines.push(
  "Detections whose drawn bounds provably exclude the glyph rows: the OCR box\n" +
    "sits entirely on the pixel-measured glyph band (horizontal-gradient row\n" +
    "energy) while >=40% of that band falls outside the labeled bounds. Bounds\n" +
    "replaced with the measured glyph box; character boxes remapped affinely.\n" +
    "Tool: examples/audit_label_bounds.rs (report: target/eval/label-bounds-audit.json).\n",
);

for (const [evalPath, list] of byEval) {
  const spec = JSON.parse(readFileSync(evalPath, "utf8"));
  for (const f of list) {
    const det = spec.detections.find((d: any) => d.id === f.detection_id);
    if (!det) {
      console.error(`MISSING detection ${f.detection_id} in ${evalPath}`);
      continue;
    }
    const o = det.bounds;
    const n = f.proposed_bounds;
    const sx = n.width / o.width;
    const sy = n.height / o.height;
    for (const ch of det.characters) {
      ch.bounds = {
        x: n.x + (ch.bounds.x - o.x) * sx,
        y: n.y + (ch.bounds.y - o.y) * sy,
        width: ch.bounds.width * sx,
        height: ch.bounds.height * sy,
      };
    }
    det.bounds = { x: n.x, y: n.y, width: n.width, height: n.height };
    changed++;
    const fmt = (r: any) =>
      `(${r.x.toFixed(0)}, ${r.y.toFixed(0)}, ${r.width.toFixed(0)}, ${r.height.toFixed(0)})`;
    logLines.push(
      `- \`${evalPath.replaceAll("\\", "/")}\` \`${f.detection_id}\` ("${f.text}"): ` +
        `bounds ${fmt(o)} -> ${fmt(det.bounds)}; glyph band y=${f.glyph_band_y[0]}..${f.glyph_band_y[1]}, ` +
        `label covered ${(f.label_band_overlap * 100).toFixed(0)}% of band, OCR rows on band ${(f.ocr_band_overlap * 100).toFixed(0)}%.`,
    );
  }
  writeFileSync(evalPath, JSON.stringify(spec, null, 2) + "\n");
}

appendFileSync(logPath, logLines.join("\n") + "\n");
console.log(`applied ${changed} bounds fixes across ${byEval.size} eval files; logged to ${logPath}`);
