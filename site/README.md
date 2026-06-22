# Mite marketing site

The source for [mite.jessica.black](https://mite.jessica.black): a single-page
explainer for what Mite is and why you'd want it, plus an FAQ.

It is a static [Astro](https://astro.build) site. The Rust crate in the
repository root is untouched by it; the Node toolchain is scoped entirely to this
directory.

## Develop

```sh
cd site
npm install
npm run dev      # http://localhost:4321
```

```sh
npm run build    # static output to site/dist
npm run preview  # serve the production build locally
npm run check    # astro type/diagnostics check
npm test         # unit tests for the sample-sentence data (vitest)
```

## Structure

- `src/pages/index.astro` composes the home page from the section components;
  `src/pages/faq.astro` is the FAQ.
- `src/components/` holds one component per section (`Hero`, `Moment`,
  `HowItWorks`, `WhyItHolds`, `GrammarColor`, `HonestFit`, `GetStarted`) plus
  shared atoms: `Header`, `Footer`, `SectionHead`, `Icon`, the brand set
  (`MiteMark`, `Wordmark`, `MiteWord`, `MiteTerm`, `MiteText`), and the pure-CSS
  overlay demo (`OverlayLine`, `DefinePopup`, `Ruby`).
- `src/data/sentences.ts` is the single source of authored sample content:
  site constants, the two demo sentences, and the part-of-speech legend derived
  from them. Every Japanese string is original, neutral, textbook-style example
  text. No third-party IP. `sentences.test.ts` guards that logic.
- `src/styles/tokens.css` is the design system: surfaces, the ink ramp, the
  single amber accent, fonts, and the colorblind-safe part-of-speech palette
  (the only hues that leave the monochrome chrome, and only in product demos).
  `src/styles/global.css` is the reset, base type, and shared classes (buttons,
  the overlay demo, the self-defining みて term, on-load motion).
- `public/` holds static assets: `CNAME` (custom domain), `favicon.svg`,
  the PWA icons and `manifest.json`, `robots.txt`, `sitemap.xml`, and `og.png`
  (the Open Graph card).

## Fonts

The three faces (Hanken Grotesk, Geist Mono, and Noto Sans JP for the Japanese)
load from Google Fonts in `src/layouts/Base.astro`. Google's dynamic subsetting
keeps the Japanese face small; self-hosting the full CJK range would be several
megabytes, so the hosted stylesheet is the deliberate choice here.

## The Open Graph image

`public/og.png` is generated from an on-brand template. To regenerate it (for
example after changing the headline), install Playwright once and run the script:

```sh
npm install --no-save playwright && npx playwright install chromium
node scripts/og-gen.mjs
```

Playwright is intentionally not a tracked dependency: the committed `og.png` is
the source of truth, and the generator is only needed when it changes.

## Deploying

Pushes to `main` that touch `site/**` trigger
[`.github/workflows/site.yml`](../.github/workflows/site.yml), which builds the
site and deploys it to GitHub Pages.

A custom domain needs three things set up once, outside this repo:

1. **DNS:** a `CNAME` record for `mite.jessica.black` pointing at
   `jssblck.github.io`.
2. **Repo settings -> Pages:** set the build source to **GitHub Actions**, and
   set the custom domain to `mite.jessica.black` (this matches `public/CNAME`,
   which the build copies into `dist/`). Enable **Enforce HTTPS** once the
   certificate is issued.
3. The first successful run of the workflow publishes the site.
