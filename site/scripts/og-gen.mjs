// Generates the Open Graph card (public/og.png, 1200x630) from an on-brand HTML
// template. Run with: node scripts/og-gen.mjs
//
// The card mirrors the live site: the reticle mark + みて wordmark (the stylized
// brand, same as the header), the reading-aid framing, and the redesigned
// definition popup (furigana headword, a part-of-speech pill, a gloss, and an
// inflection note) floating over a grammar-colored sentence. Colors track
// src/styles/tokens.css.
//
// Playwright is intentionally not a tracked dependency: the committed og.png is
// the source of truth, and the generator is only needed when it changes. Install
// it on demand:
//   npm install --no-save playwright && npx playwright install chromium
import { chromium } from "playwright";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const outPath = resolve(here, "../public/og.png");

const html = `<!doctype html><html><head><meta charset="utf-8">
<style>
  @import url('https://fonts.googleapis.com/css2?family=Geist+Mono:wght@400;500&family=Hanken+Grotesk:wght@500;600;700;800&family=Noto+Sans+JP:wght@500;700&display=swap');
  * { margin: 0; box-sizing: border-box; }
  html, body { width: 1200px; height: 630px; }
  body {
    /* Tokens: --color-bg, mirrored from src/styles/tokens.css. */
    font-family: 'Hanken Grotesk', system-ui, sans-serif;
    background-color: oklch(0.146 0.008 256);
    background-image:
      linear-gradient(oklch(0.97 0 0 / 0.03) 1px, transparent 1px),
      linear-gradient(90deg, oklch(0.97 0 0 / 0.03) 1px, transparent 1px);
    background-size: 48px 48px;
    color: oklch(0.972 0.004 256);
    padding: 56px 72px;
    display: flex;
    flex-direction: column;
    justify-content: space-between;

    /* Part-of-speech channel, mirrored from tokens.css. */
    --pos-noun: oklch(0.77 0.115 234);
    --pos-verb: oklch(0.81 0.14 159);
    --pos-particle: oklch(0.84 0.13 82);
    --pos-adjective: oklch(0.78 0.155 50);
    --pos-noun-soft: oklch(0.77 0.115 234 / 0.16);
    --pos-verb-soft: oklch(0.81 0.14 159 / 0.14);
    --pos-particle-soft: oklch(0.84 0.13 82 / 0.14);
    --pos-adjective-soft: oklch(0.78 0.155 50 / 0.15);
  }

  /* ---- brand row: reticle mark + みて wordmark (matches the site header) ---- */
  .top { display: flex; align-items: center; gap: 13px; }
  .mark { width: 42px; height: 42px; flex: none; }
  .brand {
    font-family: 'Noto Sans JP', sans-serif; font-weight: 700; font-size: 35px;
    letter-spacing: -0.01em; line-height: 1; color: oklch(0.972 0.004 256);
  }
  .tag {
    margin-left: auto; font-family: 'Geist Mono', monospace; font-size: 15px;
    color: oklch(0.752 0.012 256); border: 1px solid oklch(0.305 0.012 256);
    border-radius: 999px; padding: 8px 16px; letter-spacing: 0.02em;
  }

  /* ---- headline + reading-aid subhead (kept to the left lane so the centered
     popup never collides with it) ---- */
  .head { max-width: 540px; }
  h1 {
    font-size: 44px; font-weight: 800; line-height: 1.06; letter-spacing: -0.03em;
    color: oklch(0.868 0.006 256);
  }
  h1 .strong { color: oklch(0.972 0.004 256); }
  .lede {
    margin-top: 16px; font-size: 20px; font-weight: 500; line-height: 1.4;
    color: oklch(0.752 0.012 256);
  }

  /* ---- overlay demo: redesigned popup over a grammar-colored sentence.
     Centered so the popup, which floats up over the right-of-center verb, sits
     clear of the left-lane headline. ---- */
  .demo { position: relative; display: flex; justify-content: center; }
  .sent {
    font-family: 'Noto Sans JP', sans-serif; font-weight: 500; font-size: 44px;
    letter-spacing: 0.02em; display: inline-flex; align-items: flex-end; gap: 2px;
  }
  .w {
    padding: 3px 5px; border-radius: 5px;
    /* Calm per-part-of-speech underline at rest. */
    box-shadow: inset 0 -0.16em 0 -0.02em var(--pos-soft);
  }
  .noun { --pos: var(--pos-noun); --pos-soft: var(--pos-noun-soft); }
  .verb { --pos: var(--pos-verb); --pos-soft: var(--pos-verb-soft); }
  .part { --pos: var(--pos-particle); --pos-soft: var(--pos-particle-soft); }
  .adj  { --pos: var(--pos-adjective); --pos-soft: var(--pos-adjective-soft); }
  /* The engaged word: full underline + soft fill, anchoring the popup. */
  .w.active {
    position: relative; background: var(--pos-soft);
    box-shadow: inset 0 -0.16em 0 -0.02em var(--pos);
  }

  .pop {
    position: absolute; bottom: calc(100% + 15px); left: 50%;
    transform: translateX(-50%);
    width: max-content; max-width: 340px; text-align: left;
    background: oklch(0.198 0.011 256 / 0.97);
    border: 1px solid oklch(0.42 0.014 256); border-radius: 12px;
    box-shadow:
      0 1px 0 0 oklch(0.99 0 0 / 0.05) inset,
      0 22px 48px -16px oklch(0 0 0 / 0.75);
    padding: 14px 16px;
  }
  .pop > * { display: block; }
  .pop::after {
    content: ""; position: absolute; top: 100%; left: 50%;
    width: 11px; height: 11px;
    background: oklch(0.198 0.011 256 / 0.97);
    border-right: 1px solid oklch(0.42 0.014 256);
    border-bottom: 1px solid oklch(0.42 0.014 256);
    transform: translate(-50%, -6px) rotate(45deg);
  }
  .pop-dict {
    font-family: 'Noto Sans JP', sans-serif; font-weight: 700;
    font-size: 26px; line-height: 1.2; color: oklch(0.972 0.004 256);
  }
  .pop-dict ruby { ruby-align: center; ruby-position: over; }
  .pop-dict rt {
    font-family: 'Noto Sans JP', sans-serif; font-size: 0.5em; font-weight: 500;
    color: oklch(0.752 0.012 256); letter-spacing: 0;
  }
  .pop-pos {
    width: max-content; margin-top: 9px;
    font-family: 'Geist Mono', monospace; font-size: 12px; font-weight: 500;
    letter-spacing: 0.08em; text-transform: uppercase;
    color: var(--pos-verb);
    border: 1px solid color-mix(in oklch, var(--pos-verb) 45%, transparent);
    background: var(--pos-verb-soft);
    border-radius: 999px; padding: 3px 10px;
  }
  .pop-gloss {
    margin-top: 9px; font-size: 17px; line-height: 1.4;
    color: oklch(0.868 0.006 256);
  }
  .pop-note {
    margin-top: 9px; padding-top: 9px;
    border-top: 1px solid oklch(0.305 0.012 256);
    font-size: 14px; line-height: 1.4; color: oklch(0.752 0.012 256);
  }
  .pop-note b { color: oklch(0.868 0.006 256); font-weight: 600; }
  .punct { padding: 3px 2px; }

  /* ---- foot: an honest instrument readout + the domain ---- */
  .foot {
    display: flex; align-items: center; justify-content: space-between;
    font-family: 'Geist Mono', monospace; font-size: 18px;
    color: oklch(0.64 0.013 256); letter-spacing: 0.01em;
  }
</style></head>
<body>
  <div class="top">
    <svg class="mark" viewBox="0 0 24 24" fill="none">
      <g stroke="oklch(0.972 0.004 256)" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
        <path d="M3 7.5V4.5C3 3.67 3.67 3 4.5 3H7.5"/>
        <path d="M16.5 3H19.5C20.33 3 21 3.67 21 4.5V7.5"/>
        <path d="M21 16.5V19.5C21 20.33 20.33 21 19.5 21H16.5"/>
        <path d="M7.5 21H4.5C3.67 21 3 20.33 3 19.5V16.5"/>
      </g>
      <rect x="7" y="11" width="10" height="2" rx="1" fill="oklch(0.972 0.004 256)" opacity="0.5"/>
      <rect x="7" y="11" width="4" height="2" rx="1" fill="oklch(0.83 0.115 78)"/>
    </svg>
    <span class="brand" lang="ja">みて</span>
    <span class="tag">reading aid for Japanese</span>
  </div>

  <div class="head">
    <h1>Read the Japanese<br><span class="strong">right where it appears.</span></h1>
    <p class="lede">A reading aid for Japanese in games and visual novels, on-device in about 200ms.</p>
  </div>

  <div class="demo">
    <span class="sent" lang="ja">
      <span class="w noun">彼女</span><span class="w part">は</span><span class="w adj">新しい</span><span class="w noun">本</span><span class="w part">を</span><span class="w verb active">読んでいる<span class="pop">
          <span class="pop-dict"><ruby>読<rt>よ</rt></ruby>む</span>
          <span class="pop-pos">Verb / godan</span>
          <span class="pop-gloss">to read</span>
          <span class="pop-note"><b>Continuous</b> te-form + iru: in progress.</span>
        </span></span><span class="punct">。</span>
    </span>
  </div>

  <div class="foot">
    <span>LOCAL &middot; ON-DEVICE &middot; WINDOWS &middot; AGPL-3.0</span>
    <span>mite.jessica.black</span>
  </div>
</body></html>`;

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1200, height: 630 } });
await page.setContent(html, { waitUntil: "networkidle" });
await page.evaluate(() => document.fonts.ready);
await page.waitForTimeout(400);
await page.screenshot({ path: outPath });
await browser.close();
console.log("wrote", outPath);
