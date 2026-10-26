#!/usr/bin/env node
/**
 * Print a single-file report HTML to an A4 PDF the way cas-release-report's
 * pdf.md specifies: print media, light scheme, network blocked, disclosures
 * expanded, 15 mm margins, backgrounds printed.
 *
 * Usage:
 *   npm exec --yes --package=playwright -- node print-pdf.mjs <report.html> <out.pdf> [--author "Name"]
 *
 * --author sets the PDF Author metadata (Chromium otherwise leaves it empty and
 * stamps itself as Creator); it needs PyMuPDF (`python3 -c "import pymupdf"`).
 */
import { execFileSync } from 'node:child_process';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { resolvePlaywright } from './capture.mjs';

const argv = process.argv.slice(2);
const [src, out] = argv.filter((a, i) => !a.startsWith('--') && argv[i - 1] !== '--author');
const authorAt = argv.indexOf('--author');
const author = authorAt >= 0 ? argv[authorAt + 1] : null;
if (!src || !out) {
  console.error('usage: print-pdf.mjs <report.html> <out.pdf> [--author "Name"]');
  process.exit(2);
}

const { chromium } = await resolvePlaywright();
const browser = await chromium.launch({ headless: true });
const context = await browser.newContext({ colorScheme: 'light', javaScriptEnabled: true });
await context.route(/^https?:/, (route) => route.abort());
const page = await context.newPage();
const blocked = [];
page.on('requestfailed', (r) => { if (/^https?:/.test(r.url())) blocked.push(r.url()); });
await page.goto(pathToFileURL(resolve(src)).href, { waitUntil: 'load' });
await page.emulateMedia({ media: 'print', colorScheme: 'light' });
await page.evaluate(() => {
  document.querySelectorAll('details').forEach((d) => { d.open = true; });
  return document.fonts.ready;
});
await page.pdf({
  path: resolve(out), format: 'A4', printBackground: true, preferCSSPageSize: false,
  margin: { top: '15mm', right: '15mm', bottom: '15mm', left: '15mm' },
});
await browser.close();

if (author) {
  execFileSync('python3', ['-c', [
    'import sys, pymupdf',
    'd = pymupdf.open(sys.argv[1]); m = d.metadata',
    'm.update(author=sys.argv[2], creator=sys.argv[2], producer=sys.argv[2])',
    'd.set_metadata(m); d.saveIncr()',
  ].join('\n'), resolve(out), author], { stdio: 'inherit' });
}
if (blocked.length) {
  console.error(`network requests blocked (the HTML is not self-contained):\n${blocked.join('\n')}`);
  process.exit(1);
}
console.log(`wrote ${resolve(out)}`);
