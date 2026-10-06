#!/usr/bin/env node
// Render Glass's aurora (cas-675e) to a small image and write it into glass.css.
//
// The aurora's colours live in glass.css as --look-aurora-source: seven radial
// gradients per scheme. Painting those live behind see-through panels was too
// expensive: every update inside a panel repainted the gradients beneath it,
// and timing-sensitive journeys (HUB-J12 event flood, HUB-J14) timed out where
// base passed. A 320x200 JPEG stretched to the shell paints in a fraction of
// the time and is indistinguishable, because the gradients are soft.
//
// Run after changing any --look-aurora-source:
//   node scripts/render-glass-aurora.mjs
// It renders each source block in Chromium and rewrites the --look-aurora
// line that follows it.
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";

const path = fileURLToPath(new URL("../src/glass.css", import.meta.url));
const [WIDTH, HEIGHT, QUALITY] = [320, 200, 90];
let css = readFileSync(path, "utf8");
const pattern = /(--look-aurora-source:\s*)([\s\S]*?);(\s*)--look-aurora: [^\n]*/g;
const sources = [...css.matchAll(pattern)].map((match) => match[2].replace(/\s+/g, " ").trim());
if (sources.length !== 3) throw new Error(`expected 3 --look-aurora-source blocks (light, dark media, dark), found ${sources.length}`);

const browser = await chromium.launch({ channel: "chromium" });
const images = new Map();
try {
  for (const source of new Set(sources)) {
    const page = await browser.newPage({ viewport: { width: WIDTH, height: HEIGHT } });
    await page.setContent(`<body style="margin:0"><div style="width:${WIDTH}px;height:${HEIGHT}px;background:${source}"></div></body>`);
    images.set(source, (await page.screenshot({ type: "jpeg", quality: QUALITY })).toString("base64"));
    await page.close();
  }
} finally {
  await browser.close();
}

let index = 0;
css = css.replace(pattern, (_match, head, body, gap) => {
  const image = images.get(sources[index++]);
  return `${head}${body};${gap}--look-aurora: url("data:image/jpeg;base64,${image}") 0 0 / 100% 100% no-repeat var(--bg-root); /* rendered by scripts/render-glass-aurora.mjs */`;
});
writeFileSync(path, css);
console.log(`glass.css: rendered ${images.size} aurora image(s) for ${index} blocks`);
