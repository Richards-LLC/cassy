#!/usr/bin/env node

import { execFileSync } from 'node:child_process';
import { readFile, mkdir, writeFile } from 'node:fs/promises';
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { homedir } from 'node:os';
import { fileURLToPath, pathToFileURL } from 'node:url';

const DEFAULT_VIEWPORTS = [
  { name: 'desktop', width: 1280, height: 800 },
  { name: 'phone', width: 390, height: 800 },
];
const DEFAULT_SCHEMES = ['light', 'dark'];
const CONTRAST_LIMIT = 4.5;
const LARGE_TEXT_LIMIT = 3;
const BOX_TOLERANCE = 1;

const PAGE_INSPECTION = ({ colorScheme, contrastLimit, largeTextLimit, boxTolerance }) => {
    const fallback = colorScheme === 'dark' ? [17, 24, 39, 1] : [255, 255, 255, 1];
    const body = document.body;

    const round = (value) => Math.round(value * 100) / 100;
    const sample = (value, length = 96) => value.replace(/\s+/g, ' ').trim().slice(0, length);
    const cssColor = (value) => {
      if (!value || value === 'transparent') return [0, 0, 0, 0];
      const hex = value.match(/^#([0-9a-f]{3,8})$/i);
      if (hex) {
        const raw = hex[1];
        const expanded = raw.length <= 4 ? raw.split('').map((char) => char + char).join('') : raw;
        if (expanded.length === 6 || expanded.length === 8) {
          return [
            parseInt(expanded.slice(0, 2), 16),
            parseInt(expanded.slice(2, 4), 16),
            parseInt(expanded.slice(4, 6), 16),
            expanded.length === 8 ? parseInt(expanded.slice(6, 8), 16) / 255 : 1,
          ];
        }
      }
      const rgb = value.match(/^rgba?\(\s*([\d.]+)[, ]+\s*([\d.]+)[, ]+\s*([\d.]+)(?:[, /]+\s*([\d.]+%?))?\s*\)$/i);
      if (rgb) {
        const alpha = rgb[4] === undefined ? 1 : (rgb[4].endsWith('%') ? Number.parseFloat(rgb[4]) / 100 : Number.parseFloat(rgb[4]));
        return [Number(rgb[1]), Number(rgb[2]), Number(rgb[3]), alpha];
      }
      return null;
    };
    const over = (foreground, background) => {
      const alpha = foreground[3] + background[3] * (1 - foreground[3]);
      if (alpha === 0) return [0, 0, 0, 0];
      return [
        (foreground[0] * foreground[3] + background[0] * background[3] * (1 - foreground[3])) / alpha,
        (foreground[1] * foreground[3] + background[1] * background[3] * (1 - foreground[3])) / alpha,
        (foreground[2] * foreground[3] + background[2] * background[3] * (1 - foreground[3])) / alpha,
        alpha,
      ];
    };
    const luminance = (color) => color.slice(0, 3).map((channel) => {
      const normalized = channel / 255;
      return normalized <= 0.03928 ? normalized / 12.92 : ((normalized + 0.055) / 1.055) ** 2.4;
    }).reduce((sum, channel, index) => sum + channel * [0.2126, 0.7152, 0.0722][index], 0);
    const ratio = (foreground, background) => {
      const light = Math.max(luminance(foreground), luminance(background));
      const dark = Math.min(luminance(foreground), luminance(background));
      return (light + 0.05) / (dark + 0.05);
    };
    const selectorFor = (element) => {
      if (element.id) return '#' + CSS.escape(element.id);
      const parts = [];
      let current = element;
      while (current && current.nodeType === 1 && current !== document.body) {
        let part = current.localName;
        if (current.classList.length) part += '.' + [...current.classList].slice(0, 2).map((name) => CSS.escape(name)).join('.');
        const siblings = current.parentElement ? [...current.parentElement.children].filter((child) => child.localName === current.localName) : [];
        if (siblings.length > 1) part += ':nth-of-type(' + (siblings.indexOf(current) + 1) + ')';
        parts.unshift(part);
        current = current.parentElement;
      }
      return parts.length ? parts.join(' > ') : 'body';
    };
    const ancestors = (element) => {
      const result = [];
      for (let current = element; current; current = current.parentElement) result.unshift(current);
      return result;
    };
    const ariaHidden = (element) => ancestors(element).some((current) => current.getAttribute('aria-hidden') === 'true');
    const visibility = (element) => {
      let opacity = 1;
      let hidden = false;
      for (const current of ancestors(element)) {
        const style = getComputedStyle(current);
        opacity *= Number.parseFloat(style.opacity || '1');
        hidden ||= style.display === 'none' || style.visibility === 'hidden' || style.visibility === 'collapse';
      }
      return { opacity, hidden: hidden || opacity <= 0 };
    };
    const backgroundFor = (element) => {
      let background = fallback;
      let hasUnverifiableImage = false;
      for (const current of ancestors(element)) {
        const style = getComputedStyle(current);
        if (style.backgroundImage && style.backgroundImage !== 'none') hasUnverifiableImage = true;
        const color = cssColor(style.backgroundColor);
        if (color) background = over(color, background);
      }
      return { background, hasUnverifiableImage };
    };
    const box = (rect) => ({ x: round(rect.x), y: round(rect.y), width: round(rect.width), height: round(rect.height), right: round(rect.right), bottom: round(rect.bottom) });
    const textNodes = [];
    const walker = document.createTreeWalker(document.documentElement, 4);
    let node;
    while ((node = walker.nextNode())) {
      const text = sample(node.nodeValue || '');
      if (!text || !node.parentElement) continue;
      const element = node.parentElement;
      if (element.closest('head, style, script, noscript, template')) continue;
      const range = document.createRange();
      range.selectNodeContents(node);
      const rect = range.getBoundingClientRect();
      const style = getComputedStyle(element);
      const fg = cssColor(style.color);
      const background = backgroundFor(element);
      const state = visibility(element);
      const item = {
        elementPath: selectorFor(element),
        selector: element.id ? '#' + CSS.escape(element.id) : selectorFor(element),
        text,
        box: box(rect),
        foreground: fg ? fg.slice(0, 3).map(Math.round) : null,
        background: background.background.slice(0, 3).map(Math.round),
        hasUnverifiableImage: background.hasUnverifiableImage,
        hidden: state.hidden,
        opacity: round(state.opacity),
        ariaHidden: ariaHidden(element),
        fontSize: Number.parseFloat(style.fontSize) || 16,
        fontWeight: Number.parseInt(style.fontWeight, 10) || 400,
        colorAlpha: fg ? round(fg[3]) : 0,
        node,
      };
      textNodes.push(item);
    }

    const findings = [];
    const add = (type, item, details = {}) => findings.push({ type, selector: item?.selector || item?.elementPath || 'document', elementPath: item?.elementPath || item?.selector || 'document', textSample: item?.text, ...details });
    const visibleText = textNodes.filter((item) => !item.hidden && !item.ariaHidden && item.box.width > 0 && item.box.height > 0);
    for (const item of textNodes) {
      if ((item.hidden || item.opacity <= 0 || item.colorAlpha <= 0 || item.box.width <= 0 || item.box.height <= 0) && !item.ariaHidden) {
        add('invisible-text', item, { reason: item.opacity <= 0 ? 'opacity-0' : item.box.width <= 0 || item.box.height <= 0 ? 'zero-size' : 'visibility-hidden' });
        continue;
      }
      if (item.ariaHidden || item.hidden || item.box.width <= 0 || item.box.height <= 0) continue;
      if (!item.foreground || item.hasUnverifiableImage) {
        add('unverifiable-contrast', item, { reason: item.hasUnverifiableImage ? 'background-image' : 'unsupported-color' });
        continue;
      }
      const foreground = over([item.foreground[0], item.foreground[1], item.foreground[2], item.colorAlpha], [item.background[0], item.background[1], item.background[2], 1]);
      const contrast = ratio(foreground, [item.background[0], item.background[1], item.background[2], 1]);
      const large = item.fontSize >= 24 || (item.fontSize >= 18.66 && item.fontWeight >= 700);
      const threshold = large ? largeTextLimit : contrastLimit;
      if (contrast < threshold) add('contrast', item, { foreground: foreground.slice(0, 3).map(Math.round), background: item.background, ratio: round(contrast), threshold, largeText: large });
    }

    const elements = [...document.querySelectorAll('*')];
    const viewport = { width: window.innerWidth, height: window.innerHeight };
    for (const element of elements) {
      const style = getComputedStyle(element);
      const rect = element.getBoundingClientRect();
      const path = selectorFor(element);
      const item = { selector: element.id ? '#' + CSS.escape(element.id) : path, elementPath: path };
      const overflowX = style.overflowX === 'hidden' || style.overflowX === 'clip';
      const overflowY = style.overflowY === 'hidden' || style.overflowY === 'clip';
      const contentExceedsBorder = element !== document.documentElement && element !== document.body && (element.scrollWidth > element.clientWidth + boxTolerance || element.scrollHeight > element.clientHeight + boxTolerance);
      if (contentExceedsBorder && !overflowX && !overflowY) add('content-overflow', item, { reason: 'content-exceeds-border-box', scrollWidth: element.scrollWidth, scrollHeight: element.scrollHeight, clientWidth: element.clientWidth, clientHeight: element.clientHeight });
      if ((overflowX && element.scrollWidth > element.clientWidth + boxTolerance) || (overflowY && element.scrollHeight > element.clientHeight + boxTolerance)) {
        add('clipped-content', item, { reason: 'scroll-size-exceeds-client-size', scrollWidth: element.scrollWidth, scrollHeight: element.scrollHeight, clientWidth: element.clientWidth, clientHeight: element.clientHeight });
        if (style.textOverflow !== 'ellipsis' && (element.scrollWidth > element.clientWidth + boxTolerance || element.scrollHeight > element.clientHeight + boxTolerance)) add('truncated-container', item, { reason: 'overflow-without-ellipsis', textOverflow: style.textOverflow, scrollWidth: element.scrollWidth, scrollHeight: element.scrollHeight, clientWidth: element.clientWidth, clientHeight: element.clientHeight });
      }
      if (rect.right < -boxTolerance || rect.left > viewport.width + boxTolerance) add('outside-viewport', item, { reason: 'horizontal-position', box: box(rect), viewport });
      if ((style.position === 'fixed' || style.position === 'absolute') && (rect.right > viewport.width + boxTolerance || rect.bottom > document.documentElement.scrollHeight + boxTolerance)) {
        const container = element.parentElement;
        if (container && (getComputedStyle(container).overflowX === 'hidden' || getComputedStyle(container).overflowY === 'hidden')) add('clipped-content', item, { reason: 'positioned-child-exceeds-container', box: box(rect) });
      }
    }

    for (const item of textNodes) {
      if (item.ariaHidden) continue;
      let ancestor = item.node.parentElement;
      while (ancestor) {
        const style = getComputedStyle(ancestor);
        if (style.overflowX === 'hidden' || style.overflowX === 'clip' || style.overflowY === 'hidden' || style.overflowY === 'clip') {
          const ancestorBox = ancestor.getBoundingClientRect();
          if (item.box.x < ancestorBox.x - boxTolerance || item.box.right > ancestorBox.right + boxTolerance || item.box.y < ancestorBox.y - boxTolerance || item.box.bottom > ancestorBox.bottom + boxTolerance) add('clipped-content', item, { reason: 'text-bounds-exceed-overflow-ancestor', ancestorPath: selectorFor(ancestor), ancestorBox: box(ancestorBox) });
        }
        ancestor = ancestor.parentElement;
      }
    }

    for (let left = 0; left < visibleText.length; left += 1) {
      for (let right = left + 1; right < visibleText.length; right += 1) {
        const a = visibleText[left].box;
        const b = visibleText[right].box;
        const aElement = visibleText[left].node.parentElement;
        const bElement = visibleText[right].node.parentElement;
        if (aElement === bElement || aElement.contains(bElement) || bElement.contains(aElement)) continue;
        const intersection = Math.max(0, Math.min(a.right, b.right) - Math.max(a.x, b.x)) * Math.max(0, Math.min(a.bottom, b.bottom) - Math.max(a.y, b.y));
        if (intersection > 2) add('overlapping-text', visibleText[left], { otherElementPath: visibleText[right].elementPath, otherTextSample: visibleText[right].text, intersectionArea: round(intersection) });
      }
    }

    return { findings, textNodes: textNodes.map(({ node: _node, ...item }) => item), viewport };
};

async function resolvePlaywright() {
  try {
    return await import('playwright');
  } catch {
    const explicit = process.env.PLAYWRIGHT_MODULE;
    const binary = explicit || (() => {
      try {
        return execFileSync('which', ['playwright'], { encoding: 'utf8' }).trim();
      } catch {
        return '';
      }
    })();
    let packageDir = explicit && !explicit.endsWith('/.bin/playwright')
      ? explicit
      : binary ? join(dirname(binary), '..', 'playwright') : '';
    if (!packageDir || !existsSync(join(packageDir, 'package.json'))) {
      const npxRoot = join(homedir(), '.npm', '_npx');
      const candidates = existsSync(npxRoot)
        ? readdirSync(npxRoot, { withFileTypes: true })
            .filter((entry) => entry.isDirectory())
            .map((entry) => join(npxRoot, entry.name, 'node_modules', 'playwright'))
            .filter((candidate) => existsSync(join(candidate, 'package.json')))
        : [];
      packageDir = candidates.at(-1) || '';
    }
    if (!packageDir) throw new Error('Playwright is required. Run with `npm exec --yes --package=playwright -- node scripts/visual-qa.mjs ...` or set PLAYWRIGHT_MODULE.');
    const packageJson = JSON.parse(readFileSync(join(packageDir, 'package.json'), 'utf8'));
    const entry = packageJson.exports?.['.']?.import || packageJson.module || packageJson.main || 'index.js';
    const entryPath = join(packageDir, typeof entry === 'string' ? entry : 'index.js');
    return await import(pathToFileURL(entryPath).href);
  }
}

function normalizeViewport(viewport) {
  if (typeof viewport === 'string') {
    const match = viewport.match(/^(\d+)x(\d+)$/);
    if (!match) throw new Error(`Invalid viewport ${viewport}; use WIDTHxHEIGHT.`);
    return { name: `${match[1]}x${match[2]}`, width: Number(match[1]), height: Number(match[2]) };
  }
  return { name: viewport.name || `${viewport.width}x${viewport.height}`, width: viewport.width, height: viewport.height };
}

async function loadAllowlist(allowlistPath) {
  if (!allowlistPath) return [];
  const parsed = JSON.parse(await readFile(allowlistPath, 'utf8'));
  if (!Array.isArray(parsed.entries)) throw new Error('Allowlist must be an object with an entries array.');
  return parsed.entries.map((entry, index) => {
    if (!entry || typeof entry !== 'object' || !entry.reason?.trim() || !entry.type || !entry.selector) throw new Error(`Allowlist entry ${index + 1} requires type, selector, and a non-empty reason.`);
    return { ...entry, reason: entry.reason.trim() };
  });
}

function allowlisted(finding, entries) {
  return entries.find((entry) => (entry.type === '*' || entry.type === finding.type) && (entry.selector === '*' || entry.selector === finding.selector || entry.selector === finding.elementPath));
}

function slug(value) {
  return value.replace(/^https?:\/\//, '').replace(/^file:\/\//, '').replace(/[^a-z0-9]+/gi, '-').replace(/^-|-$/g, '').slice(-80) || 'page';
}

function systemChromium() {
  if (process.env.CHROME_PATH) return process.env.CHROME_PATH;
  for (const candidate of ['google-chrome', 'chromium', 'chromium-browser']) {
    try {
      return execFileSync('which', [candidate], { encoding: 'utf8' }).trim();
    } catch {
      // Try the next supported system browser.
    }
  }
  return undefined;
}

function markdownReport(result) {
  const lines = [
    `# Visual QA — ${result.status}`,
    '',
    `**Summary:** ${result.status} · ${result.findings.length} finding(s) · ${result.suppressed.length} allowlisted · ${result.screenshots.length} screenshot(s)`,
    '',
    `Schemes: ${result.schemes.join(', ')}  `,
    `Viewports: ${result.viewports.map((viewport) => `${viewport.name} (${viewport.width}×${viewport.height})`).join(', ')}`,
    '',
    '## Findings',
    '',
  ];
  if (!result.findings.length) lines.push('No findings.');
  for (const finding of result.findings) {
    const location = `${finding.url} · ${finding.scheme} · ${finding.viewport.name}`;
    lines.push(`- **${finding.type}** — \`${finding.elementPath}\` — ${finding.textSample ? JSON.stringify(finding.textSample) : finding.reason || 'see JSON'} (${location})`);
    if (finding.ratio !== undefined) lines.push(`  - Contrast: ${finding.ratio}:1 (required ${finding.threshold}:1); foreground ${finding.foreground?.join(', ')}; background ${finding.background?.join(', ')}`);
  }
  lines.push('', '## Screenshots', '');
  for (const screenshot of result.screenshots) lines.push(`- [${screenshot.path}](${screenshot.path}) — ${screenshot.url} · ${screenshot.scheme} · ${screenshot.viewport.name}`);
  lines.push('', '## Method', '', 'Headless Chromium rendered each URL under the requested color schemes and viewports. Text nodes were checked for effective WCAG contrast, clipping, overlap, visibility, viewport escape, and fixed-size truncation.');
  return `${lines.join('\n')}\n`;
}

/**
 * Render and inspect one or more HTML URLs.
 * @param {{urls: string[], artifactDir?: string, schemes?: string[], viewports?: Array<{name?: string,width:number,height:number}|string>, allowlistPath?: string, strict?: boolean}} options
 */
export async function runVisualQa(options) {
  if (!options?.urls?.length) throw new Error('At least one file:// or http(s) URL is required.');
  const artifactDir = resolve(options.artifactDir || join(process.cwd(), 'docs/factory/data/visual-qa'));
  const inputUrls = options.urls;
  const urls = inputUrls.map((url) => /^(?:file|https?):\/\//i.test(url) ? url : pathToFileURL(resolve(url)).href);
  const schemes = options.schemes || DEFAULT_SCHEMES;
  const viewports = (options.viewports || DEFAULT_VIEWPORTS).map(normalizeViewport);
  const allowlist = await loadAllowlist(options.allowlistPath);
  await mkdir(artifactDir, { recursive: true });
  const playwright = await resolvePlaywright();
  const browser = await playwright.chromium.launch({ headless: true, executablePath: systemChromium() });
  const findings = [];
  const suppressed = [];
  const screenshots = [];
  try {
    for (const [urlIndex, url] of urls.entries()) {
      const source = inputUrls[urlIndex];
      for (const scheme of schemes) {
        for (const viewport of viewports) {
          const context = await browser.newContext({ colorScheme: scheme, viewport: { width: viewport.width, height: viewport.height } });
          const page = await context.newPage();
          try {
            await page.goto(url, { waitUntil: 'load' });
            await page.waitForTimeout(50);
            const recordFinding = (finding) => {
              const enriched = { ...finding, url: source, scheme, viewport };
              const exception = allowlisted(enriched, allowlist);
              if (exception) suppressed.push({ ...enriched, reason: exception.reason });
              else findings.push(enriched);
            };
            const inspection = await page.evaluate(PAGE_INSPECTION, { colorScheme: scheme, contrastLimit: CONTRAST_LIMIT, largeTextLimit: LARGE_TEXT_LIMIT, boxTolerance: BOX_TOLERANCE });
            for (const finding of inspection.findings) recordFinding(finding);

            const screenText = await page.locator('body').innerText().catch(() => '');
            await page.emulateMedia({ media: 'print' });
            const printText = await page.locator('body').innerText().catch(() => '');
            if (screenText.trim().length > 20 && printText.trim().length < Math.max(1, Math.floor(screenText.trim().length * 0.8))) {
              recordFinding({ type: 'print-loss', selector: 'body', elementPath: 'body', textSample: printText.trim().slice(0, 96), reason: 'print-media-hides-content', screenCharacters: screenText.trim().length, printCharacters: printText.trim().length });
            }
            await page.emulateMedia({ media: 'screen' });

            const noScriptContext = await browser.newContext({ colorScheme: scheme, viewport: { width: viewport.width, height: viewport.height }, javaScriptEnabled: false });
            const noScriptPage = await noScriptContext.newPage();
            try {
              await noScriptPage.goto(url, { waitUntil: 'load' });
              const noScriptText = await noScriptPage.locator('body').innerText().catch(() => '');
              if (screenText.trim().length > 20 && noScriptText.trim().length < Math.max(1, Math.floor(screenText.trim().length * 0.8))) {
                recordFinding({ type: 'javascript-disabled-loss', selector: 'body', elementPath: 'body', textSample: noScriptText.trim().slice(0, 96), reason: 'content-requires-javascript', screenCharacters: screenText.trim().length, javascriptDisabledCharacters: noScriptText.trim().length });
              }
            } finally {
              await noScriptContext.close();
            }

            const filename = `${slug(source)}-${scheme}-${viewport.name}.png`;
            await page.screenshot({ path: join(artifactDir, filename), fullPage: true });
            screenshots.push({ path: filename, url: source, scheme, viewport });
          } finally {
            await context.close();
          }
        }
      }
    }
  } finally {
    await browser.close();
  }
  const result = {
    status: findings.length ? 'FAIL' : 'PASS',
    exitCode: findings.length && options.strict ? 1 : 0,
    generatedAt: new Date().toISOString(),
    schemes,
    viewports,
    urls: inputUrls,
    findings,
    suppressed,
    screenshots,
    counts: findings.reduce((counts, finding) => ({ ...counts, [finding.type]: (counts[finding.type] || 0) + 1 }), {}),
  };
  result.markdown = markdownReport(result);
  const { markdown: _markdown, ...jsonResult } = result;
  await writeFile(join(artifactDir, 'visual-qa.json'), `${JSON.stringify(jsonResult, null, 2)}\n`);
  await writeFile(join(artifactDir, 'visual-qa.md'), result.markdown);
  return result;
}

function parseArgs(argv) {
  const options = { urls: [], strict: false, schemes: DEFAULT_SCHEMES, viewports: DEFAULT_VIEWPORTS };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === '--strict') options.strict = true;
    else if (arg === '--artifact-dir') options.artifactDir = argv[++index];
    else if (arg === '--allowlist') options.allowlistPath = argv[++index];
    else if (arg === '--scheme') options.schemes = [argv[++index]];
    else if (arg === '--viewport') options.viewports = [argv[++index]];
    else if (arg === '--help' || arg === '-h') options.help = true;
    else options.urls.push(arg);
  }
  return options;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const options = parseArgs(process.argv.slice(2));
  if (options.help || !options.urls.length) {
    console.log('Usage: npm exec --yes --package=playwright -- node scripts/visual-qa.mjs [--strict] [--artifact-dir DIR] [--allowlist FILE] [--scheme light|dark] [--viewport WIDTHxHEIGHT] URL...');
    process.exitCode = options.help ? 0 : 2;
  } else {
    try {
      const result = await runVisualQa(options);
      if (result.status === 'PASS') console.log('PASS');
      else for (const finding of result.findings) {
        const colors = finding.foreground && finding.background ? ` foreground=${finding.foreground.join(',')} background=${finding.background.join(',')} ratio=${finding.ratio ?? 'n/a'}` : '';
        console.log(`FAIL ${finding.type} ${finding.elementPath} text=${JSON.stringify(finding.textSample || '')}${colors}`);
      }
      process.exitCode = result.exitCode;
    } catch (error) {
      console.error(error instanceof Error ? error.message : error);
      process.exitCode = 2;
    }
  }
}
