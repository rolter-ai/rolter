#!/usr/bin/env bun
// social preview renderer (#855). `assets/og.svg` is the source of truth; this
// turns it into the two committed images — `user-docs/images/og.png` for the
// Mintlify `seo.metatags` entry and `ui/public/og.png` for the dashboard's own
// `og:image`, which has to be vendored because rolter must run air-gapped.
//
// the render is deterministic: chromium rasterises the svg at exactly
// 1200x630 with no device-pixel scaling, and Geist comes from node_modules
// through a data: @font-face rather than from a font service, so the same
// checkout produces the same bytes on any machine.
//
// run from ui/: `bun scripts/render-og.ts`
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";

import { chromium } from "playwright";

const root = join(import.meta.dir, "..", "..");
const width = 1200;
const height = 630;

const outputs = [
  join(root, "user-docs", "images", "og.png"),
  join(root, "ui", "public", "og.png"),
];

// the latin subsets only: the image carries no cyrillic or vietnamese glyphs,
// and shipping the whole family would triple the inlined payload
const faces = [
  { family: "Geist Variable", file: "@fontsource-variable/geist/files/geist-latin-wght-normal.woff2" },
  { family: "Geist Mono Variable", file: "@fontsource-variable/geist-mono/files/geist-mono-latin-wght-normal.woff2" },
];

function fontFace({ family, file }: { family: string; file: string }): string {
  const bytes = readFileSync(join(import.meta.dir, "..", "node_modules", file));
  return `@font-face {
      font-family: "${family}";
      font-style: normal;
      font-weight: 100 900;
      src: url(data:font/woff2;base64,${bytes.toString("base64")}) format("woff2-variations");
    }`;
}

const svg = readFileSync(join(root, "assets", "og.svg"), "utf8");
const html = `<!doctype html>
<html><head><meta charset="utf-8" /><style>
  ${faces.map(fontFace).join("\n  ")}
  html, body { margin: 0; padding: 0; background: #111113; }
  svg { display: block; }
</style></head><body>${svg}</body></html>`;

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width, height }, deviceScaleFactor: 1 });
await page.setContent(html, { waitUntil: "load" });
await page.evaluate(() => document.fonts.ready);
const png = await page.screenshot({ clip: { x: 0, y: 0, width, height } });
await browser.close();

for (const output of outputs) {
  mkdirSync(dirname(output), { recursive: true });
  writeFileSync(output, png);
  console.log(`wrote ${output} (${(png.byteLength / 1024).toFixed(1)} KB)`);
}
