#!/usr/bin/env node
// Render Cassy Cloud's raster icons from public/favicon.svg (cas-8951).
//
// favicon.svg is the one brand geometry: the three Cassy ribbons on the violet
// Send tile. Browsers without SVG favicons, the iOS home screen and the web
// manifest need PNGs, so this script paints them in Chromium:
//   favicon-16.png, favicon-32.png, icon-192.png, icon-512.png  rounded tile, transparent corners
//   apple-touch-icon.png (180)                                   full-bleed square; iOS rounds it
//   icon-maskable-192.png, icon-maskable-512.png                 full-bleed, ribbons inside the 80% safe circle
//
// Run after changing favicon.svg:
//   node scripts/render-brand-icons.mjs
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";

const publicDir = new URL("../public/", import.meta.url);
const source = readFileSync(new URL("favicon.svg", publicDir), "utf8");
const TILE_RADIUS = 'rx="114"';
const MARK_SCALE = "scale(1.2)";
for (const needle of [TILE_RADIUS, MARK_SCALE]) {
  if (!source.includes(needle)) throw new Error(`favicon.svg no longer contains ${needle}; update render-brand-icons.mjs`);
}

const fullBleed = source.replaceAll(TILE_RADIUS, 'rx="0"');
// At scale 0.95 the ribbons' bounding box reaches at most 188px from the tile
// centre, inside the maskable safe zone (a centred circle of radius 0.4 × 512 = 205).
const maskable = fullBleed.replace(MARK_SCALE, "scale(0.95)");

const outputs = [
  ["favicon-16.png", source, 16],
  ["favicon-32.png", source, 32],
  ["icon-192.png", source, 192],
  ["icon-512.png", source, 512],
  ["apple-touch-icon.png", fullBleed, 180],
  ["icon-maskable-192.png", maskable, 192],
  ["icon-maskable-512.png", maskable, 512],
];

const browser = await chromium.launch({ channel: "chromium" });
try {
  for (const [name, svg, size] of outputs) {
    const page = await browser.newPage({ viewport: { width: size, height: size }, deviceScaleFactor: 1 });
    const src = `data:image/svg+xml,${encodeURIComponent(svg)}`;
    await page.setContent(`<body style="margin:0;background:transparent"><img src="${src}" width="${size}" height="${size}" style="display:block"></body>`);
    await page.locator("img").evaluate((img) => img.decode());
    const png = await page.screenshot({ type: "png", omitBackground: true, clip: { x: 0, y: 0, width: size, height: size } });
    writeFileSync(fileURLToPath(new URL(name, publicDir)), png);
    await page.close();
  }
} finally {
  await browser.close();
}
console.log(`rendered ${outputs.length} icons from public/favicon.svg`);
