// Aggregate failure analysis over per-capture eval reports in target/eval/corpus.
// Usage: bun scripts/analyze-eval-failures.ts [reportDir]
import { readdirSync, readFileSync } from "fs";
import { join } from "path";

const dir = process.argv[2] ?? "target/eval/corpus";
const files = readdirSync(dir).filter((f) => f.endsWith(".json"));

type Counter = Map<string, number>;
const bump = (m: Counter, k: string, n = 1) => m.set(k, (m.get(k) ?? 0) + n);
const top = (m: Counter, n: number) =>
  [...m.entries()].sort((a, b) => b[1] - a[1]).slice(0, n);

const missedDetections: Counter = new Map(); // expected text of unmatched detections
const missedByCapture: Counter = new Map();
const charDiffPairs: Counter = new Map(); // expected->actual char substitutions
const charDiffExamples: Map<string, string[]> = new Map();
const fieldFailures: Counter = new Map(); // which metadata field failed
const tokenFieldFailures: Counter = new Map(); // surface|field|expected|actual
const unexpectedTexts: Counter = new Map();
const unmatchedTokenSpans: Counter = new Map(); // token existed but span mismatch
const partialBoundsCaptures: Counter = new Map();

let totalExpected = 0;
let totalUnmatched = 0;
let totalCharErrors = 0;
let totalTokens = 0;
let totalTokenCredit = 0;
let detectionsWithTextMismatch = 0;
let matchedDetections = 0;
let partialBoundsCredit = 0; // detections matched but detection_score < 1

for (const file of files) {
  const report = JSON.parse(readFileSync(join(dir, file), "utf8"));
  for (const det of report.detections) {
    totalExpected++;
    if (!det.actual) {
      totalUnmatched++;
      bump(missedDetections, det.expected_text);
      bump(missedByCapture, file);
      continue;
    }
    matchedDetections++;
    if (det.detection_score < 0.9999) {
      partialBoundsCredit++;
      bump(partialBoundsCaptures, file);
    }
    if (det.char_edit_distance > 0) {
      detectionsWithTextMismatch++;
      totalCharErrors += det.char_edit_distance;
      for (const diff of det.character_differences ?? []) {
        const key = `${diff.expected ?? "∅"} -> ${diff.actual ?? "∅"} (${diff.kind})`;
        bump(charDiffPairs, key);
        const ex = charDiffExamples.get(key) ?? [];
        if (ex.length < 3)
          ex.push(`${file}: "${det.expected_text}" vs "${det.actual.text}"`);
        charDiffExamples.set(key, ex);
      }
    }
    for (const tok of det.token_scores ?? []) {
      totalTokens++;
      totalTokenCredit += tok.metadata_score;
      if (tok.metadata_score >= 0.9999) continue;
      if (!tok.actual) {
        bump(unmatchedTokenSpans, `${tok.expected.surface} (span miss)`);
        continue;
      }
      for (const fs of tok.field_scores ?? []) {
        if (fs.passed) continue;
        bump(fieldFailures, fs.field);
        bump(
          tokenFieldFailures,
          `${tok.expected.surface} | ${fs.field} | exp=${JSON.stringify(fs.expected)} | act=${JSON.stringify(fs.actual)}`,
        );
      }
    }
  }
  for (const ua of report.unexpected_actual ?? []) {
    bump(unexpectedTexts, `${ua.text} [h=${Math.round(ua.text_box.rect.height)}]`);
  }
}

console.log(`reports: ${files.length}`);
console.log(
  `expected detections: ${totalExpected}, unmatched: ${totalUnmatched}, matched-with-partial-bounds: ${partialBoundsCredit}`,
);
console.log(
  `detections with char errors: ${detectionsWithTextMismatch}, total char edit distance: ${totalCharErrors}`,
);
console.log(
  `tokens: ${totalTokens}, token credit: ${totalTokenCredit.toFixed(1)} (${((totalTokenCredit / totalTokens) * 100).toFixed(2)}%)`,
);

console.log(`\n== captures with most missed detections ==`);
for (const [k, v] of top(missedByCapture, 15)) console.log(`${v}\t${k}`);

console.log(`\n== most-missed expected texts (top 40) ==`);
for (const [k, v] of top(missedDetections, 40)) console.log(`${v}\t${k}`);

console.log(`\n== char substitution pairs (top 50) ==`);
for (const [k, v] of top(charDiffPairs, 50)) {
  console.log(`${v}\t${k}`);
  for (const ex of charDiffExamples.get(k) ?? []) console.log(`\t\t${ex}`);
}

console.log(`\n== metadata field failure counts ==`);
for (const [k, v] of top(fieldFailures, 20)) console.log(`${v}\t${k}`);

console.log(`\n== token span misses (top 30) ==`);
for (const [k, v] of top(unmatchedTokenSpans, 30)) console.log(`${v}\t${k}`);

console.log(`\n== token field failures (top 60) ==`);
for (const [k, v] of top(tokenFieldFailures, 60)) console.log(`${v}\t${k}`);

console.log(`\n== unexpected actual texts (top 30) ==`);
for (const [k, v] of top(unexpectedTexts, 30)) console.log(`${v}\t${k}`);
