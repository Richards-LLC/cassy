#!/usr/bin/env node

import { execFileSync } from 'node:child_process';
import { readFile, mkdir, writeFile } from 'node:fs/promises';
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { homedir } from 'node:os';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { inflateSync } from 'node:zlib';

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
    const hasHorizontalScroller = (element) => ancestors(element).some((current) => {
      const overflow = getComputedStyle(current).overflowX;
      return overflow === 'auto' || overflow === 'scroll';
    });
    const ariaHidden = (element) => ancestors(element).some((current) => current.getAttribute('aria-hidden') === 'true');
    const nonVisualReason = (element) => {
      if (!element) return null;
      if (ariaHidden(element)) return 'aria-hidden';
      if (element.closest('svg title, svg desc')) return 'svg-accessibility-text';
      if (element.closest('.skip, .sr, .sr-only, .visually-hidden, .visuallyHidden, [data-visual-qa-hidden]')) return 'accessibility-helper';
      return null;
    };
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
      const ignoredReason = nonVisualReason(element);
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
        ignored: Boolean(ignoredReason),
        ignoredReason,
        statusLike: Boolean(element.closest('.tag, .status, [role="status"]')),
        node,
      };
      textNodes.push(item);
    }

    const findings = [];
    const infos = [];
    const add = (type, item, details = {}) => findings.push({ type, selector: item?.selector || item?.elementPath || 'document', elementPath: item?.elementPath || item?.selector || 'document', textSample: item?.text, ...details });
    const addInfo = (type, item, details = {}) => infos.push({ type, selector: item?.selector || item?.elementPath || 'document', elementPath: item?.elementPath || item?.selector || 'document', textSample: item?.text, ...details });
    const visibleText = textNodes.filter((item) => !item.ignored && !item.hidden && !item.ariaHidden && item.box.width > 0 && item.box.height > 0);
    for (const item of textNodes) {
      if (item.ignored || item.ariaHidden || item.box.width <= 0 || item.box.height <= 0) continue;
      if (item.hidden || item.opacity <= 0 || item.colorAlpha <= 0) {
        add('invisible-text', item, { reason: item.opacity <= 0 ? 'opacity-0' : item.box.width <= 0 || item.box.height <= 0 ? 'zero-size' : 'visibility-hidden' });
        continue;
      }
      if (!item.foreground || item.hasUnverifiableImage) {
        addInfo('unverifiable-contrast', item, { reason: item.hasUnverifiableImage ? 'background-image' : 'unsupported-color' });
        continue;
      }
      const foreground = over([item.foreground[0], item.foreground[1], item.foreground[2], item.colorAlpha], [item.background[0], item.background[1], item.background[2], 1]);
      const contrast = ratio(foreground, [item.background[0], item.background[1], item.background[2], 1]);
      const large = item.fontSize >= 24 || (item.fontSize >= 18.66 && item.fontWeight >= 700);
      const threshold = large ? largeTextLimit : contrastLimit;
      if (contrast < threshold) {
        const details = { foreground: foreground.slice(0, 3).map(Math.round), background: item.background, ratio: round(contrast), threshold, largeText: large };
        if (item.statusLike) addInfo('off-token-contrast', item, { ...details, reason: 'status-label-pair-is-not-a-sanctioned-token' });
        else add('contrast', item, details);
      }
    }

    const elements = [...document.querySelectorAll('*')];
    const viewport = { width: window.innerWidth, height: window.innerHeight };
    const documentWidth = Math.max(document.documentElement.clientWidth, document.documentElement.scrollWidth, document.body?.scrollWidth || 0);
    for (const element of elements) {
      if (nonVisualReason(element)) continue;
      const style = getComputedStyle(element);
      const rect = element.getBoundingClientRect();
      const path = selectorFor(element);
      const item = { selector: element.id ? '#' + CSS.escape(element.id) : path, elementPath: path };
      const overflowX = style.overflowX === 'hidden' || style.overflowX === 'clip';
      const overflowY = style.overflowY === 'hidden' || style.overflowY === 'clip';
      const contentExceedsBorder = element !== document.documentElement && element !== document.body && (element.scrollWidth > element.clientWidth + boxTolerance || element.scrollHeight > element.clientHeight + boxTolerance);
      const clipped = (overflowX && element.scrollWidth > element.clientWidth + boxTolerance) || (overflowY && element.scrollHeight > element.clientHeight + boxTolerance);
      if (clipped) {
        add('content-overflow', item, { reason: 'content-exceeds-clipped-border-box', scrollWidth: element.scrollWidth, scrollHeight: element.scrollHeight, clientWidth: element.clientWidth, clientHeight: element.clientHeight });
        add('clipped-content', item, { reason: 'scroll-size-exceeds-client-size', scrollWidth: element.scrollWidth, scrollHeight: element.scrollHeight, clientWidth: element.clientWidth, clientHeight: element.clientHeight });
        if (style.textOverflow !== 'ellipsis' && (element.scrollWidth > element.clientWidth + boxTolerance || element.scrollHeight > element.clientHeight + boxTolerance)) add('truncated-container', item, { reason: 'overflow-without-ellipsis', textOverflow: style.textOverflow, scrollWidth: element.scrollWidth, scrollHeight: element.scrollHeight, clientWidth: element.clientWidth, clientHeight: element.clientHeight });
      }
      if (!hasHorizontalScroller(element) && (rect.left < -boxTolerance || rect.right > documentWidth + boxTolerance)) add('outside-viewport', item, { reason: 'horizontal-escape-beyond-document', box: box(rect), documentWidth, viewport });
      if ((style.position === 'fixed' || style.position === 'absolute') && (rect.right > viewport.width + boxTolerance || rect.bottom > document.documentElement.scrollHeight + boxTolerance)) {
        const container = element.parentElement;
        if (container && (getComputedStyle(container).overflowX === 'hidden' || getComputedStyle(container).overflowY === 'hidden')) add('clipped-content', item, { reason: 'positioned-child-exceeds-container', box: box(rect) });
      }
    }

    for (const item of textNodes) {
      if (item.ignored || item.ariaHidden || item.box.width <= 0 || item.box.height <= 0) continue;
      const svgBoundary = item.node.parentElement?.closest('svg');
      let ancestor = item.node.parentElement;
      while (ancestor) {
        if (ancestor === svgBoundary) break;
        if (ancestor !== item.node.parentElement && ['auto', 'scroll'].includes(getComputedStyle(ancestor).overflowX)) break;
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
        const aStyle = getComputedStyle(aElement);
        const bStyle = getComputedStyle(bElement);
        const aPositioned = ['absolute', 'fixed', 'sticky'].includes(aStyle.position) || aStyle.transform !== 'none';
        const bPositioned = ['absolute', 'fixed', 'sticky'].includes(bStyle.position) || bStyle.transform !== 'none';
        if (!aPositioned && !bPositioned) continue;
        const aAncestors = ancestors(aElement);
        const bAncestors = ancestors(bElement);
        let commonIndex = -1;
        for (let index = aAncestors.length - 1; index >= 0; index -= 1) {
          if (bAncestors.includes(aAncestors[index])) {
            commonIndex = index;
            break;
          }
        }
        const commonAncestor = commonIndex < 0 ? null : aAncestors[commonIndex];
        const bCommonIndex = commonAncestor ? bAncestors.indexOf(commonAncestor) : -1;
        const aDistance = commonIndex < 0 ? Number.POSITIVE_INFINITY : aAncestors.length - commonIndex - 1;
        const bDistance = bCommonIndex < 0 ? Number.POSITIVE_INFINITY : bAncestors.length - bCommonIndex - 1;
        if (!commonAncestor || commonAncestor === document.body || commonAncestor === document.documentElement || aDistance > 3 || bDistance > 3 || hasHorizontalScroller(aElement) || hasHorizontalScroller(bElement)) continue;
        const intersection = Math.max(0, Math.min(a.right, b.right) - Math.max(a.x, b.x)) * Math.max(0, Math.min(a.bottom, b.bottom) - Math.max(a.y, b.y));
        const smallerArea = Math.max(1, Math.min(a.width * a.height, b.width * b.height));
        if (intersection > 2 && intersection / smallerArea >= 0.2) add('overlapping-text', visibleText[left], { otherElementPath: visibleText[right].elementPath, otherTextSample: visibleText[right].text, intersectionArea: round(intersection), overlapRatio: round(intersection / smallerArea) });
      }
    }

    for (const figure of document.querySelectorAll('figure')) {
      if (nonVisualReason(figure)) continue;
      const caption = figure.querySelector(':scope > figcaption');
      if (!caption || nonVisualReason(caption)) continue;
      const figureBox = figure.getBoundingClientRect();
      const captionBox = caption.getBoundingClientRect();
      const previous = caption.previousElementSibling;
      const contentBottom = previous ? previous.getBoundingClientRect().bottom : figureBox.top;
      const captionOverlapsContent = captionBox.top < contentBottom - boxTolerance;
      const captionEscapesFigure = captionBox.bottom > figureBox.bottom + boxTolerance;
      if (captionOverlapsContent || captionEscapesFigure) {
        const captionItem = { selector: caption.id ? `#${CSS.escape(caption.id)}` : selectorFor(caption), elementPath: selectorFor(caption), text: sample(caption.textContent || ''), box: box(captionBox) };
        add(captionOverlapsContent ? 'overlapping-text' : 'clipped-content', captionItem, { reason: captionOverlapsContent ? 'figure-caption-layout-overlap' : 'figure-caption-overflow', otherElementPath: selectorFor(figure), otherTextSample: '', intersectionArea: round(Math.max(0, Math.min(captionBox.right, figureBox.right) - Math.max(captionBox.left, figureBox.left)) * Math.max(0, Math.min(captionBox.bottom, contentBottom) - Math.max(captionBox.top, figureBox.top))) });
      }
    }

    return { findings, infos, textNodes: textNodes.map(({ node: _node, ...item }) => item), viewport };
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

function decodePng(png) {
  const bytes = Buffer.isBuffer(png) ? png : Buffer.from(png);
  if (bytes.toString('ascii', 1, 4) !== 'PNG') throw new Error('Visual QA screenshot is not a PNG.');
  let offset = 8;
  let width;
  let height;
  let bitDepth;
  let colorType;
  const idat = [];
  while (offset < bytes.length) {
    const length = bytes.readUInt32BE(offset);
    const type = bytes.toString('ascii', offset + 4, offset + 8);
    const data = bytes.subarray(offset + 8, offset + 8 + length);
    offset += 12 + length;
    if (type === 'IHDR') {
      width = data.readUInt32BE(0);
      height = data.readUInt32BE(4);
      bitDepth = data[8];
      colorType = data[9];
    } else if (type === 'IDAT') {
      idat.push(data);
    } else if (type === 'IEND') {
      break;
    }
  }
  if (bitDepth !== 8 || ![2, 6].includes(colorType)) throw new Error('Visual QA only supports 8-bit RGB/RGBA screenshots.');
  const channels = colorType === 6 ? 4 : 3;
  const stride = width * channels;
  const raw = inflateSync(Buffer.concat(idat));
  const pixels = Buffer.alloc(height * stride);
  let rawOffset = 0;
  for (let y = 0; y < height; y += 1) {
    const filter = raw[rawOffset++];
    const rowStart = y * stride;
    for (let x = 0; x < stride; x += 1) {
      const left = x >= channels ? pixels[rowStart + x - channels] : 0;
      const above = y ? pixels[rowStart - stride + x] : 0;
      const upperLeft = y && x >= channels ? pixels[rowStart - stride + x - channels] : 0;
      const value = raw[rawOffset++];
      if (filter === 0) pixels[rowStart + x] = value;
      else if (filter === 1) pixels[rowStart + x] = (value + left) & 255;
      else if (filter === 2) pixels[rowStart + x] = (value + above) & 255;
      else if (filter === 3) pixels[rowStart + x] = (value + Math.floor((left + above) / 2)) & 255;
      else if (filter === 4) {
        const p = left + above - upperLeft;
        const pa = Math.abs(p - left);
        const pb = Math.abs(p - above);
        const pc = Math.abs(p - upperLeft);
        pixels[rowStart + x] = (value + (pa <= pb && pa <= pc ? left : pb <= pc ? above : upperLeft)) & 255;
      } else throw new Error(`Unsupported PNG filter ${filter}.`);
    }
  }
  return { width, height, channels, pixels };
}

function sampleScreenshotBackground(png, itemBox) {
  if (!itemBox?.width || !itemBox?.height) return null;
  const image = decodePng(png);
  const x0 = Math.max(0, Math.floor(itemBox.x));
  const x1 = Math.min(image.width - 1, Math.ceil(itemBox.right));
  const y0 = Math.max(0, Math.floor(itemBox.y));
  const y1 = Math.min(image.height - 1, Math.ceil(itemBox.bottom));
  const points = [];
  for (let x = x0; x <= x1; x += Math.max(1, Math.ceil((x1 - x0) / 8))) {
    points.push([x, y0 - 2], [x, y1 + 2]);
  }
  for (let y = y0; y <= y1; y += Math.max(1, Math.ceil((y1 - y0) / 8))) {
    points.push([x0 - 2, y], [x1 + 2, y]);
  }
  const samples = points.filter(([x, y]) => x >= 0 && y >= 0 && x < image.width && y < image.height).map(([x, y]) => {
    const offset = (y * image.width + x) * image.channels;
    return [image.pixels[offset], image.pixels[offset + 1], image.pixels[offset + 2]];
  });
  if (!samples.length) return null;
  return samples[0].map((_, channel) => Math.round(samples.reduce((sum, pixel) => sum + pixel[channel], 0) / samples.length));
}

function rgbLuminance(color) {
  return color.map((channel) => {
    const normalized = channel / 255;
    return normalized <= 0.03928 ? normalized / 12.92 : ((normalized + 0.055) / 1.055) ** 2.4;
  }).reduce((sum, channel, index) => sum + channel * [0.2126, 0.7152, 0.0722][index], 0);
}

function rgbContrast(first, second) {
  const light = Math.max(rgbLuminance(first), rgbLuminance(second));
  const dark = Math.min(rgbLuminance(first), rgbLuminance(second));
  return Math.round(((light + 0.05) / (dark + 0.05)) * 100) / 100;
}

function markdownReport(result) {
  const lines = [
    `# Visual QA — ${result.status}`,
    '',
    `**Summary:** ${result.status} · ${result.findings.length} finding(s) · ${result.infoFindings.length} informational · ${result.suppressed.length} allowlisted · ${result.screenshots.length} screenshot(s)`,
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
  lines.push('', '## Informational checks', '');
  if (!result.infoFindings.length) lines.push('None.');
  for (const finding of result.infoFindings) {
    const location = `${finding.url} · ${finding.scheme} · ${finding.viewport.name}`;
    lines.push(`- **${finding.type}** — \`${finding.elementPath}\` — ${finding.reason || 'see JSON'} (${location})`);
    if (finding.sampledBackground) lines.push(`  - Screenshot sample: background ${finding.sampledBackground.join(', ')}${finding.sampledRatio === undefined ? '' : `; ratio ${finding.sampledRatio}:1`}`);
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
  const infoFindings = [];
  const suppressed = [];
  const screenshots = [];
  const seen = new Set();
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
            const recordFinding = (finding, informational = false) => {
              const enriched = { ...finding, url: source, scheme, viewport };
              const key = [informational ? 'info' : 'finding', enriched.type, enriched.selector || enriched.elementPath, enriched.otherElementPath || '', scheme, viewport.name].join('|');
              if (seen.has(key)) return;
              seen.add(key);
              const exception = allowlisted(enriched, allowlist);
              if (exception) suppressed.push({ ...enriched, reason: exception.reason });
              else if (informational) infoFindings.push(enriched);
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

            const screenshotBuffer = await page.screenshot({ path: join(artifactDir, `${slug(source)}-${scheme}-${viewport.name}.png`), fullPage: true });
            for (const info of inspection.infos) {
              const sampledBackground = sampleScreenshotBackground(screenshotBuffer, info.box);
              const sampledRatio = sampledBackground && info.foreground ? rgbContrast(info.foreground, sampledBackground) : undefined;
              recordFinding({ ...info, sampledBackground, sampledRatio }, true);
            }

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
    infoFindings,
    suppressed,
    screenshots,
    counts: findings.reduce((counts, finding) => ({ ...counts, [finding.type]: (counts[finding.type] || 0) + 1 }), {}),
    infoCounts: infoFindings.reduce((counts, finding) => ({ ...counts, [finding.type]: (counts[finding.type] || 0) + 1 }), {}),
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
