// Generates the Open Graph card (public/og.png, 1200x630) from an on-brand HTML
// template. Run with: node scripts/og-gen.mjs
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
  @import url('https://fonts.googleapis.com/css2?family=Geist+Mono:wght@400;500&family=Hanken+Grotesk:wght@600;700;800&family=Noto+Sans+JP:wght@500;700&display=swap');
  * { margin: 0; box-sizing: border-box; }
  html, body { width: 1200px; height: 630px; }
  body {
    font-family: 'Hanken Grotesk', system-ui, sans-serif;
    background-color: oklch(0.146 0.008 256);
    background-image:
      linear-gradient(oklch(0.97 0 0 / 0.03) 1px, transparent 1px),
      linear-gradient(90deg, oklch(0.97 0 0 / 0.03) 1px, transparent 1px);
    background-size: 48px 48px;
    color: oklch(0.972 0.004 256);
    padding: 76px 80px;
    display: flex;
    flex-direction: column;
    justify-content: space-between;
  }
  .top { display: flex; align-items: center; gap: 14px; }
  .mark { width: 40px; height: 40px; }
  .word {
    font-family: 'Geist Mono', monospace; font-weight: 500; font-size: 30px;
    letter-spacing: -0.01em; color: oklch(0.972 0.004 256);
  }
  .tag {
    margin-left: auto; font-family: 'Geist Mono', monospace; font-size: 16px;
    color: oklch(0.752 0.012 256); border: 1px solid oklch(0.305 0.012 256);
    border-radius: 999px; padding: 8px 16px; letter-spacing: 0.02em;
  }
  h1 {
    font-size: 82px; font-weight: 800; line-height: 1.0; letter-spacing: -0.035em;
    max-width: 18ch; color: oklch(0.972 0.004 256);
  }
  h1 .dim { color: oklch(0.752 0.012 256); }
  .demo { display: flex; align-items: center; gap: 22px; }
  .jp {
    font-family: 'Noto Sans JP', sans-serif; font-weight: 500; font-size: 40px;
    letter-spacing: 0.02em; display: inline-flex; gap: 2px;
  }
  .jp .w {
    padding: 2px 4px; border-radius: 4px;
    box-shadow: inset 0 -0.16em 0 -0.02em var(--pos);
  }
  .verb { --pos: oklch(0.81 0.14 159); }
  .noun { --pos: oklch(0.77 0.115 234); }
  .part { --pos: oklch(0.84 0.13 82); }
  .adj { --pos: oklch(0.78 0.155 50); }
  .reading {
    margin-left: auto; font-family: 'Geist Mono', monospace; font-size: 17px;
    color: oklch(0.752 0.012 256); display: inline-flex; align-items: center; gap: 9px;
  }
  .reading .dot { width: 9px; height: 9px; border-radius: 999px; background: oklch(0.83 0.115 78); }
  .foot { font-family: 'Geist Mono', monospace; font-size: 18px; color: oklch(0.64 0.013 256); }
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
    <span class="word">mite</span>
    <span class="tag">local japanese ocr overlay</span>
  </div>

  <h1>Point at the word. <span class="dim">The meaning is there.</span></h1>

  <div class="demo">
    <span class="jp">
      <span class="w noun">彼女</span><span class="w part">は</span><span class="w adj">新しい</span><span class="w noun">本</span><span class="w part">を</span><span class="w verb">読んでいる</span>
    </span>
    <span class="reading"><span class="dot"></span>MITE / READING</span>
  </div>

  <div class="foot">mite.jessica.black</div>
</body></html>`;

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1200, height: 630 } });
await page.setContent(html, { waitUntil: "networkidle" });
await page.evaluate(() => document.fonts.ready);
await page.waitForTimeout(400);
await page.screenshot({ path: outPath });
await browser.close();
console.log("wrote", outPath);
