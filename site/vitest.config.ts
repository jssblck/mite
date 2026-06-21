import { defineConfig } from "vitest/config";

// The site is a static Astro build, but the authored sample-sentence data in
// src/data/ carries real logic (dictionary-form recovery, the part-of-speech
// legend derivation). That logic is pure TypeScript and is unit tested here.
export default defineConfig({
  test: {
    include: ["src/**/*.test.ts"],
  },
});
