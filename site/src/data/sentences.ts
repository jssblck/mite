/*
  Site constants + authored sample content.

  HARD CONSTRAINT: every Japanese string here is original, neutral, textbook-
  style example text written for this site. No game, visual novel, studio, or
  other third-party IP is referenced, and none of it is transcribed from a real
  capture. Keep it that way.
*/

// Canonical outbound links. The product is open source; the site links out to
// the repo rather than duplicating install docs.
export const GITHUB_URL = "https://github.com/jssblck/mite";
export const GITHUB_BOOTSTRAP_URL = `${GITHUB_URL}#quick-start`;
export const LICENSE_URL = `${GITHUB_URL}/blob/main/LICENSE`;
export const RELEASES_URL = `${GITHUB_URL}/releases`;

export const SITE_URL = "https://mite.jessica.black";
export const SITE_NAME = "Mite";
export const SITE_TAGLINE = "Read Japanese right where it appears.";
export const SITE_DESCRIPTION =
  "Mite is a reading aid for learning Japanese in games, visual novels, and other Windows apps. Point at a word and the reading and meaning are already there, on-device, in about 200ms.";

export type Pos =
  | "particle"
  | "noun"
  | "verb"
  | "adjective"
  | "adverb"
  | "other";

export type Seg = { base: string; rt?: string };

/** Plain text of a segment list, furigana stripped. */
export const segText = (segs: Seg[]): string =>
  segs.map((seg) => seg.base).join("");

export type Definition = {
  /** dictionary form, segmented so furigana renders over each kanji group */
  dict: Seg[];
  posLabel: string;
  gloss: string;
  note?: { form: string; text: string };
};

export type Word = {
  segs: Seg[];
  /** small reading shown above the word in the live overlay */
  reading?: string;
  pos: Pos;
  define?: Definition;
};

export type Sentence = {
  /** plain-text form, used for accessible labels and tests */
  text: string;
  english: string;
  words: Word[];
  /** index of the word whose popup is shown by default */
  active: number;
};

// "She is reading a new book" demonstrates dictionary-form recovery and an
// inflection note (the te-form + iru continuous), Mite's strongest trick.
export const HERO_SENTENCE: Sentence = {
  text: "彼女は新しい本を読んでいる。",
  english: "She is reading a new book.",
  active: 5,
  words: [
    {
      segs: [{ base: "彼女", rt: "かのじょ" }],
      reading: "かのじょ",
      pos: "noun",
      define: {
        dict: [{ base: "彼女", rt: "かのじょ" }],
        posLabel: "Pronoun",
        gloss: "she; her",
      },
    },
    {
      segs: [{ base: "は" }],
      pos: "particle",
      define: {
        dict: [{ base: "は" }],
        posLabel: "Particle",
        gloss: "topic marker: marks what the sentence is about",
      },
    },
    {
      segs: [{ base: "新", rt: "あたら" }, { base: "しい" }],
      reading: "あたらしい",
      pos: "adjective",
      define: {
        dict: [{ base: "新", rt: "あたら" }, { base: "しい" }],
        posLabel: "i-adjective",
        gloss: "new; fresh; recent",
      },
    },
    {
      segs: [{ base: "本", rt: "ほん" }],
      reading: "ほん",
      pos: "noun",
      define: {
        dict: [{ base: "本", rt: "ほん" }],
        posLabel: "Noun",
        gloss: "book; volume",
      },
    },
    {
      segs: [{ base: "を" }],
      pos: "particle",
      define: {
        dict: [{ base: "を" }],
        posLabel: "Particle",
        gloss: "direct-object marker",
      },
    },
    {
      segs: [{ base: "読", rt: "よ" }, { base: "んで" }],
      reading: "よんで",
      pos: "verb",
      define: {
        dict: [{ base: "読", rt: "よ" }, { base: "む" }],
        posLabel: "Verb (godan)",
        gloss: "to read",
        note: {
          form: "Continuous",
          text: 'te-form joined with iru: an action in progress ("is reading").',
        },
      },
    },
    {
      segs: [{ base: "いる" }],
      reading: "いる",
      pos: "verb",
      define: {
        dict: [{ base: "いる" }],
        posLabel: "Auxiliary verb",
        gloss: "marks ongoing or continuous action when following a te-form",
      },
    },
    { segs: [{ base: "。" }], pos: "other" },
  ],
};

// A longer second register for the color legend, exercising every grammar
// color: noun, particle, adverb, adjective, verb, and punctuation. The verb
// also shows dictionary-form recovery (飲みます from 飲む).
export const LEGEND_SENTENCE: Sentence = {
  text: "母は寒い朝にとても熱いコーヒーをゆっくり飲みます。",
  english: "On cold mornings, my mother slowly drinks very hot coffee.",
  active: -1,
  words: [
    {
      segs: [{ base: "母", rt: "はは" }],
      pos: "noun",
      define: {
        dict: [{ base: "母", rt: "はは" }],
        posLabel: "Noun",
        gloss: "mother",
      },
    },
    {
      segs: [{ base: "は" }],
      pos: "particle",
      define: {
        dict: [{ base: "は" }],
        posLabel: "Particle",
        gloss: "topic marker",
      },
    },
    {
      segs: [{ base: "寒", rt: "さむ" }, { base: "い" }],
      pos: "adjective",
      define: {
        dict: [{ base: "寒", rt: "さむ" }, { base: "い" }],
        posLabel: "i-adjective",
        gloss: "cold",
      },
    },
    {
      segs: [{ base: "朝", rt: "あさ" }],
      pos: "noun",
      define: {
        dict: [{ base: "朝", rt: "あさ" }],
        posLabel: "Noun",
        gloss: "morning",
      },
    },
    {
      segs: [{ base: "に" }],
      pos: "particle",
      define: {
        dict: [{ base: "に" }],
        posLabel: "Particle",
        gloss: "marks a point in time",
      },
    },
    {
      segs: [{ base: "とても" }],
      pos: "adverb",
      define: {
        dict: [{ base: "とても" }],
        posLabel: "Adverb",
        gloss: "very",
      },
    },
    {
      segs: [{ base: "熱", rt: "あつ" }, { base: "い" }],
      pos: "adjective",
      define: {
        dict: [{ base: "熱", rt: "あつ" }, { base: "い" }],
        posLabel: "i-adjective",
        gloss: "hot",
      },
    },
    {
      segs: [{ base: "コーヒー" }],
      pos: "noun",
      define: {
        dict: [{ base: "コーヒー" }],
        posLabel: "Noun",
        gloss: "coffee",
      },
    },
    {
      segs: [{ base: "を" }],
      pos: "particle",
      define: {
        dict: [{ base: "を" }],
        posLabel: "Particle",
        gloss: "direct-object marker",
      },
    },
    {
      segs: [{ base: "ゆっくり" }],
      pos: "adverb",
      define: {
        dict: [{ base: "ゆっくり" }],
        posLabel: "Adverb",
        gloss: "slowly",
      },
    },
    {
      segs: [{ base: "飲", rt: "の" }, { base: "みます" }],
      pos: "verb",
      define: {
        dict: [{ base: "飲", rt: "の" }, { base: "む" }],
        posLabel: "Verb (godan)",
        gloss: "to drink",
        note: {
          form: "Polite",
          text: "masu-form: the polite non-past of 飲む.",
        },
      },
    },
    { segs: [{ base: "。" }], pos: "other" },
  ],
};

export type PosMeta = { pos: Pos; label: string; examples: string[] };

const POS_LABELS: { pos: Pos; label: string }[] = [
  { pos: "noun", label: "Noun" },
  { pos: "verb", label: "Verb" },
  { pos: "adjective", label: "Adjective" },
  { pos: "adverb", label: "Adverb" },
  { pos: "particle", label: "Particle" },
  { pos: "other", label: "Other / unknown" },
];

const surfaceOf = (word: Word): string => segText(word.segs);

// The grammar-color legend. Colors are an information channel tied to grammar.
// Examples are lifted straight from LEGEND_SENTENCE (up to two distinct surface
// forms per part of speech, in order of appearance), so the legend and the live
// demo always show the same words.
export const POS_LEGEND: PosMeta[] = POS_LABELS.map(({ pos, label }) => {
  const examples: string[] = [];
  for (const word of LEGEND_SENTENCE.words) {
    if (word.pos !== pos) continue;
    const surface = surfaceOf(word);
    if (!examples.includes(surface)) examples.push(surface);
    if (examples.length === 2) break;
  }
  return { pos, label, examples };
});

// The name "Mite" is read みて: the te-form of 見る ("look"). In prose the name
// renders as a live dictionary term that defines itself with the very popup the
// overlay uses, so the brand demonstrates the product on its own name.
export const MITE_WORD: Word = {
  segs: [{ base: "みて" }],
  pos: "verb",
  define: {
    dict: [{ base: "見", rt: "み" }, { base: "る" }],
    posLabel: "Verb (ichidan)",
    gloss: "to see; to look; to watch",
    note: {
      form: "te-form",
      text: 'みて is the te-form of 見る, and how "Mite" is read.',
    },
  },
};
