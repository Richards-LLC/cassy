// Renders every round-3 Pebble state.
//
//  - The primary palette (Paper) gets the full state set at 1280 and 390 in
//    light and dark, because Paper follows the hub tokens and has both.
//  - Each pinned palette gets the thread screen at 1280 and 390 and the list
//    screen at 1280. A pinned palette is rendered twice, once under each OS
//    colour-scheme preference, and the two screenshots must be byte-identical
//    — that is the proof the palette is pinned rather than inverted. Only one
//    copy is written.
//
// Exits non-zero on any horizontal overflow or any pinned palette that flips.
import { mkdir, rm, writeFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { launchBrowser } from '../../hub-mobile/browser-tools.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const DESKTOP = { name: 'desktop', width: 1280, height: 800 };
const MOBILE = { name: 'mobile', width: 390, height: 844 };

const PRIMARY = ['thread-a', 'thread-b', 'list', 'evidence', 'empty', 'pairs'];
const PINNED = ['graphite', 'mono', 'ember', 'slate'];

const only = process.argv[2];
const out = resolve(here, 'png');
if (!only) await rm(out, { recursive: true, force: true });
await mkdir(out, { recursive: true });

const browser = await launchBrowser();
let overflows = 0;
let flips = 0;

async function shoot(state, vp, scheme, { write = true } = {}) {
  const context = await browser.newContext({
    viewport: { width: vp.width, height: vp.height },
    deviceScaleFactor: 2,
    colorScheme: scheme,
  });
  const page = await context.newPage();
  await page.goto(pathToFileURL(resolve(here, `${state}.html`)).href);
  await page.waitForTimeout(300);
  const buffer = await page.screenshot({ fullPage: true });
  const m = await page.evaluate(() => ({
    height: document.documentElement.scrollHeight,
    overflow: document.documentElement.scrollWidth > document.documentElement.clientWidth,
  }));
  await context.close();
  if (m.overflow) overflows += 1;
  return { buffer, ...m, write };
}

try {
  for (const state of PRIMARY) {
    if (only && only !== state) continue;
    for (const vp of [DESKTOP, MOBILE]) for (const scheme of ['light', 'dark']) {
      const r = await shoot(state, vp, scheme);
      await writeFile(resolve(out, `${state}-${vp.name}-${scheme}.png`), r.buffer);
      console.log(`${state}-${vp.name}-${scheme}.png height=${r.height}${r.overflow ? ' XOVERFLOW' : ''}`);
    }
  }

  // Paper's comparison frames: the primary palette, light.
  for (const [state, vps] of [['scheme-paper-thread', [DESKTOP, MOBILE]], ['scheme-paper-list', [DESKTOP]]]) {
    if (only && only !== state) continue;
    for (const vp of vps) {
      const r = await shoot(state, vp, 'light');
      await writeFile(resolve(out, `${state}-${vp.name}.png`), r.buffer);
      console.log(`${state}-${vp.name}.png height=${r.height}${r.overflow ? ' XOVERFLOW' : ''}`);
    }
  }

  for (const key of PINNED) {
    for (const [suffix, vps] of [['thread', [DESKTOP, MOBILE]], ['list', [DESKTOP]]]) {
      const state = `scheme-${key}-${suffix}`;
      if (only && only !== state) continue;
      for (const vp of vps) {
        const light = await shoot(state, vp, 'light');
        const dark = await shoot(state, vp, 'dark');
        const pinned = light.buffer.equals(dark.buffer);
        if (!pinned) flips += 1;
        await writeFile(resolve(out, `${state}-${vp.name}.png`), light.buffer);
        console.log(`${state}-${vp.name}.png height=${light.height}${light.overflow ? ' XOVERFLOW' : ''} pinned=${pinned ? 'ok' : 'FLIPPED'}`);
      }
    }
  }
} finally {
  await browser.close();
}

const problems = [];
if (overflows) problems.push(`${overflows} render(s) overflow horizontally`);
if (flips) problems.push(`${flips} pinned palette render(s) changed with the OS colour-scheme`);
console.log(problems.length ? `FAIL: ${problems.join('; ')}` : 'no horizontal overflow; every pinned palette held under both OS preferences');
process.exit(problems.length ? 1 : 0);
