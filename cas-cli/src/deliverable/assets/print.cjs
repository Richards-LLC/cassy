// Print a single-file deliverable HTML to PDF, one file per paper format.
// Lifted from the release report's renderer, with the SOW print-pdf.mjs
// author metadata and blocked-request refusal. Run by `cas deliverable finalize`:
//   node print.cjs REPORT.html OUTPUT_DIR FORMATS [AUTHOR]
// FORMATS is a comma list such as A4,Letter; each writes
// OUTPUT_DIR/<name>-<format>.pdf. Print media, light scheme, network blocked,
// disclosures expanded, 15 mm margins, backgrounds printed, a token folio.
// Exit 2: Playwright missing. Exit 3: the page asked the network for something,
// so the HTML is not self-contained.
const fs = require('node:fs/promises');
const path = require('node:path');
const { execFileSync } = require('node:child_process');
const { pathToFileURL } = require('node:url');
(async () => {
  let playwright;
  try { playwright = require(process.env.PLAYWRIGHT_MODULE || 'playwright'); }
  catch (error) {
    console.error('Playwright is not installed:', error.message);
    process.exitCode = 2;
    return;
  }
  const [input, output, formats, author] = process.argv.slice(2);
  if (!input || !output || !formats) throw new Error('expected REPORT.html OUTPUT_DIR FORMATS [AUTHOR]');
  await fs.mkdir(output, { recursive: true });
  const browser = await playwright.chromium.launch({ headless: true });
  const blocked = [];
  const written = [];
  try {
    const page = await browser.newPage();
    await page.route(/^https?:/, route => { blocked.push(route.request().url()); return route.abort(); });
    await page.goto(pathToFileURL(path.resolve(input)).href, { waitUntil: 'load' });
    await page.emulateMedia({ media: 'print', colorScheme: 'light' });
    await page.evaluate(async () => {
      await document.fonts.ready;
      document.querySelectorAll('details').forEach(panel => { panel.open = true; });
      // cas-692a: a link resolving to a local file is dead in every reader's
      // copy; keep its text and drop the target.
      document.querySelectorAll('a[href]').forEach(link => {
        if (link.href.startsWith('file:')) link.removeAttribute('href');
      });
    });
    // The running folio wears the report's own tokens: its title and page
    // count in the mono stack and muted ink the page declares.
    const folio = await page.evaluate(() => {
      const style = getComputedStyle(document.documentElement);
      const token = name => style.getPropertyValue(name).trim();
      return { color: token('--ink-muted') || 'GrayText', font: token('--font-mono') || 'monospace' };
    });
    const attr = text => text.replace(/[<>&"]/g, c => ({ '<': '&lt;', '>': '&gt;', '&': '&amp;', '"': '&quot;' }[c]));
    for (const format of formats.split(',')) {
      const file = path.join(output, path.basename(input, '.html') + '-' + format + '.pdf');
      await page.pdf({
        path: file,
        format, printBackground: true, preferCSSPageSize: false,
        margin: { top: '15mm', right: '15mm', bottom: '15mm', left: '15mm' },
        displayHeaderFooter: true, headerTemplate: '<span></span>',
        footerTemplate: `<div style="width:100%;margin:0 15mm;display:flex;justify-content:space-between;gap:12px;font:7px ${attr(folio.font)};color:${attr(folio.color)}"><span class="title"></span><span><span class="pageNumber"></span> / <span class="totalPages"></span></span></div>`
      });
      written.push(file);
    }
  } finally { await browser.close(); }
  // Chromium leaves Author empty and stamps itself as Creator.
  if (author) {
    for (const file of written) {
      execFileSync('python3', ['-c', [
        'import sys, pymupdf',
        'd = pymupdf.open(sys.argv[1]); m = d.metadata',
        'm.update(author=sys.argv[2], creator=sys.argv[2], producer=sys.argv[2])',
        'd.set_metadata(m); d.saveIncr()',
      ].join('\n'), file, author], { stdio: 'inherit' });
    }
  }
  if (blocked.length) {
    console.error(`network requests blocked (the HTML is not self-contained): ${blocked.join(', ')}`);
    process.exitCode = 3;
  }
})().catch(error => { console.error(error.stack || error); process.exitCode = 1; });
