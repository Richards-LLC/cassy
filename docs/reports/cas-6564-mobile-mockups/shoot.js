/* Regenerates every screenshot in ./screens.
 *
 * Each mock-up state is captured as a 412x915 screenshot of the .screen element
 * (B5 landscape is 915x412), in both themes. Every file is also loaded at a real
 * 412x915 viewport first, and the run exits non-zero on any console error.
 *
 *   node shoot.js            # headless (fine: these are static pages)
 *   HEADED=1 node shoot.js   # headed Chromium
 *
 * Needs playwright on NODE_PATH, e.g.
 *   NODE_PATH=/path/to/node_modules node shoot.js
 */
const { chromium } = require('playwright');
const path = require('path');
const fs = require('fs');

const DIR = __dirname;
const OUT = path.join(DIR, 'screens');
const PLAN = [
  ['a-inbox', ['inbox', 'expanded', 'empty', 'terminal']],
  ['b-deck', ['supervisor', 'worker', 'streaming', 'terminal', 'landscape']],
  ['c-voice', ['idle', 'listening', 'confirm', 'answer']],
];

(async () => {
  fs.mkdirSync(OUT, { recursive: true });
  const browser = await chromium.launch({ headless: process.env.HEADED !== '1' });
  const problems = [];
  let shots = 0;

  for (const [file, states] of PLAN) {
    /* 1. console-error check at the real device viewport */
    const probe = await browser.newContext({ viewport: { width: 412, height: 915 }, deviceScaleFactor: 2 });
    const page = await probe.newPage();
    page.on('console', (m) => { if (m.type() === 'error') problems.push(`${file}: console.error ${m.text()}`); });
    page.on('pageerror', (e) => problems.push(`${file}: pageerror ${e.message}`));
    for (const state of states) {
      await page.goto('file://' + path.join(DIR, file + '.html') + '#' + state);
      await page.waitForTimeout(400);
    }
    await probe.close();

    /* 2. per-state screenshots of the phone screen itself */
    for (const theme of ['dark', 'light']) {
      const ctx = await browser.newContext({ viewport: { width: 1500, height: 1100 }, deviceScaleFactor: 2 });
      const p = await ctx.newPage();
      p.on('pageerror', (e) => problems.push(`${file}[${theme}]: pageerror ${e.message}`));
      for (const state of states) {
        await p.goto('file://' + path.join(DIR, file + '.html') + '?theme=' + theme + '#' + state);
        await p.waitForTimeout(state === 'streaming' ? 3200 : 700);
        const el = await p.$('#screen');
        const box = await el.boundingBox();
        const name = `${file}-${state}-${theme}.png`;
        await el.screenshot({ path: path.join(OUT, name) });
        shots += 1;
        console.log(`${name}  ${Math.round(box.width)}x${Math.round(box.height)}`);
      }
      await ctx.close();
    }
  }

  await browser.close();
  console.log(`\n${shots} screenshots -> ${OUT}`);
  if (problems.length) {
    console.log('PROBLEMS:\n' + problems.join('\n'));
    process.exit(1);
  }
  console.log('no console errors');
})();
