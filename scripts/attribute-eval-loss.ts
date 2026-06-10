// Attribute aggregate-score loss to concrete causes, in corpus-aggregate points.
// Mirrors CorpusScoreTotals: detection/char/meta computed over summed credits and
// denominators, weighted 0.35/0.40/0.25.
// Usage: bun scripts/attribute-eval-loss.ts [reportDir]
import { readdirSync, readFileSync } from "fs";
import { join } from "path";

const dir = process.argv[2] ?? "target/eval/corpus256";
const files = readdirSync(dir).filter((f) => f.endsWith(".json"));

let detCredit = 0,
  detDenom = 0,
  charErr = 0,
  charDenom = 0,
  metaCredit = 0,
  metaDenom = 0;

// loss buckets (in raw credit units, converted to weighted points later)
let detLossUnmatched = 0; // expected with no actual
let detLossPartial = 0; // matched but detection_score < 1
let detLossUnexpected = 0; // denominator inflation by unexpected actuals
const axisFail: Map<string, number> = new Map();
let charLossUnmatched = 0;
let charLossMismatch = 0;
let charLossUnexpected = 0;
let metaLossUnmatchedDet = 0; // tokens inside unmatched detections
let metaLossSpanMiss = 0; // matched detection, token span not found
let metaLossFields = 0; // matched token, some fields failed
let metaLossUnexpected = 0; // unexpected actual token denominator
const fieldLoss: Map<string, number> = new Map();
const spanMissBySurface: Map<string, number> = new Map();
const partialWorst: [number, string, string][] = [];
const bump = (m: Map<string, number>, k: string, n = 1) =>
  m.set(k, (m.get(k) ?? 0) + n);

for (const file of files) {
  const r = JSON.parse(readFileSync(join(dir, file), "utf8"));
  for (const det of r.detections) {
    detDenom += 1;
    detCredit += det.detection_score;
    const expLen = [...det.expected_text].length;
    const actLen = det.actual ? [...det.actual.text].length : 0;
    charDenom += Math.max(expLen, actLen);
    charErr += det.char_edit_distance;
    if (!det.actual) {
      detLossUnmatched += 1;
      charLossUnmatched += det.char_edit_distance;
    } else {
      charLossMismatch += det.char_edit_distance;
      if (det.detection_score < 0.9999) {
        const loss = 1 - det.detection_score;
        detLossPartial += loss;
        partialWorst.push([loss, file, det.expected_text]);
        const d = det.bounds_delta,
          t = det.bounds_tolerance;
        if (d && t) {
          for (const ax of ["x", "y", "width", "height"] as const)
            if (d[ax] > t[ax]) bump(axisFail, ax);
        }
      }
    }
    for (const tok of det.token_scores ?? []) {
      metaDenom += 1;
      metaCredit += tok.metadata_score;
      const loss = 1 - tok.metadata_score;
      if (loss <= 0.0001) continue;
      if (!det.actual) metaLossUnmatchedDet += loss;
      else if (!tok.actual) {
        metaLossSpanMiss += loss;
        bump(spanMissBySurface, tok.expected.surface);
      } else {
        metaLossFields += loss;
        const failed = (tok.field_scores ?? []).filter((f: any) => !f.passed);
        for (const f of failed)
          bump(fieldLoss, f.field, 1 / (tok.field_scores.length || 1));
      }
    }
  }
  for (const ua of r.unexpected_actual ?? []) {
    detDenom += 1;
    detLossUnexpected += 1;
    const len = [...ua.text].length;
    charDenom += len;
    charErr += len;
    charLossUnexpected += len;
    const tokens = Math.max(ua.tokens?.length ?? 0, 1);
    metaDenom += tokens;
    metaLossUnexpected += tokens;
  }
}

const detScore = detCredit / detDenom;
const charScore = 1 - charErr / charDenom;
const metaScore = metaCredit / metaDenom;
const agg = detScore * 0.35 + charScore * 0.4 + metaScore * 0.25;
console.log(
  `scores: det ${(detScore * 100).toFixed(2)} char ${(charScore * 100).toFixed(2)} meta ${(metaScore * 100).toFixed(2)} agg ${(agg * 100).toFixed(2)}`,
);

// Convert each bucket to aggregate points: bucket_credit/denom * weight * 100
const detPts = (x: number) => ((x / detDenom) * 0.35 * 100).toFixed(3);
const charPts = (x: number) => ((x / charDenom) * 0.4 * 100).toFixed(3);
const metaPts = (x: number) => ((x / metaDenom) * 0.25 * 100).toFixed(3);

console.log(`\n== aggregate-point loss attribution ==`);
console.log(`detection.unmatched-expected   ${detPts(detLossUnmatched)} pts (${detLossUnmatched})`);
console.log(`detection.partial-bounds       ${detPts(detLossPartial)} pts (${detLossPartial.toFixed(1)} credit over ${partialWorst.length} dets)`);
console.log(`detection.unexpected-actuals   ${detPts(detLossUnexpected)} pts (${detLossUnexpected})`);
console.log(`characters.unmatched-expected  ${charPts(charLossUnmatched)} pts (${charLossUnmatched} chars)`);
console.log(`characters.matched-mismatch    ${charPts(charLossMismatch)} pts (${charLossMismatch} chars)`);
console.log(`characters.unexpected-actuals  ${charPts(charLossUnexpected)} pts (${charLossUnexpected} chars)`);
console.log(`metadata.unmatched-detections  ${metaPts(metaLossUnmatchedDet)} pts`);
console.log(`metadata.token-span-miss       ${metaPts(metaLossSpanMiss)} pts`);
console.log(`metadata.field-failures        ${metaPts(metaLossFields)} pts`);
console.log(`metadata.unexpected-actuals    ${metaPts(metaLossUnexpected)} pts`);

console.log(`\n== bounds axis failures (delta > free tolerance) ==`);
for (const [k, v] of axisFail) console.log(`${k}: ${v}`);

console.log(`\n== field loss (approx tokens-worth) ==`);
for (const [k, v] of [...fieldLoss.entries()].sort((a, b) => b[1] - a[1]))
  console.log(`${v.toFixed(1)}\t${k}`);

console.log(`\n== span-miss surfaces (top 30) ==`);
for (const [k, v] of [...spanMissBySurface.entries()]
  .sort((a, b) => b[1] - a[1])
  .slice(0, 30))
  console.log(`${v}\t${JSON.stringify(k)}`);

console.log(`\n== worst partial-bounds detections (top 25) ==`);
partialWorst.sort((a, b) => b[0] - a[0]);
for (const [loss, file, text] of partialWorst.slice(0, 25))
  console.log(`${loss.toFixed(2)}\t${file}\t${JSON.stringify(text)}`);
