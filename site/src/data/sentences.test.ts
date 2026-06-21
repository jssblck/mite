import { describe, expect, it } from "vitest";
import {
  HERO_SENTENCE,
  LEGEND_SENTENCE,
  MITE_WORD,
  POS_LEGEND,
  type Sentence,
  segText,
} from "./sentences";

describe("segText", () => {
  it("joins segment bases, dropping furigana", () => {
    expect(segText([{ base: "新", rt: "あたら" }, { base: "しい" }])).toBe(
      "新しい",
    );
  });
});

/** A sentence's words, concatenated, must equal its plain-text form. */
function reconstruct(sentence: Sentence): string {
  return sentence.words.map((w) => segText(w.segs)).join("");
}

describe("sample sentences", () => {
  it("HERO_SENTENCE words reconstruct its text", () => {
    expect(reconstruct(HERO_SENTENCE)).toBe(HERO_SENTENCE.text);
  });

  it("LEGEND_SENTENCE words reconstruct its text", () => {
    expect(reconstruct(LEGEND_SENTENCE)).toBe(LEGEND_SENTENCE.text);
  });

  it("the default-active hero word has a definition to show", () => {
    const active = HERO_SENTENCE.words[HERO_SENTENCE.active];
    expect(active?.define).toBeDefined();
  });
});

describe("POS_LEGEND", () => {
  it("covers every part of speech in stable order", () => {
    expect(POS_LEGEND.map((p) => p.pos)).toEqual([
      "noun",
      "verb",
      "adjective",
      "adverb",
      "particle",
      "other",
    ]);
  });

  it("derives examples only from the legend sentence, max two per pos", () => {
    const surfaces = new Set(
      LEGEND_SENTENCE.words.map((w) => segText(w.segs)),
    );
    for (const entry of POS_LEGEND) {
      expect(entry.examples.length).toBeLessThanOrEqual(2);
      for (const example of entry.examples) {
        expect(surfaces.has(example)).toBe(true);
      }
    }
  });

  it("shows verb examples lifted from the legend sentence", () => {
    const verb = POS_LEGEND.find((p) => p.pos === "verb");
    expect(verb?.examples).toContain("飲みます");
  });
});

describe("MITE_WORD", () => {
  it("recovers the dictionary form 見る from the te-form みて", () => {
    expect(segText(MITE_WORD.segs)).toBe("みて");
    expect(segText(MITE_WORD.define!.dict)).toBe("見る");
  });
});
