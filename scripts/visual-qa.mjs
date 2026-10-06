#!/usr/bin/env node

import { execFileSync } from 'node:child_process';
import { readFile, mkdir, writeFile, rm, mkdtemp } from 'node:fs/promises';
import { existsSync, readFileSync, readdirSync, realpathSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { homedir, tmpdir } from 'node:os';
import { createRequire } from 'node:module';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { inflateSync, inflateRawSync, deflateRawSync } from 'node:zlib';

const SECRET_KEY = /^(?:authorization|cookies?|set-cookie|__session|x-firebase-.*|(?:id|refresh|access)[_-]?token|token|password|client[_-]?secret)$/i;

/** Sanitize diagnostics before console output or artifact serialization. */
export function redactQaText(value, secrets = []) {
  let text = value instanceof Error ? value.message : String(value);
  for (const secret of secrets) if (typeof secret === 'string' && secret) text = text.split(secret).join('[REDACTED]');
  return text
    .replace(/\b(?:authorization|cookies?|set-cookie|x-firebase-[\w-]+)["']?\s*[:=][^\r\n]*/gi, '[REDACTED header]')
    .replace(/\bBearer\s+[A-Za-z0-9._~+/-]+=*/gi, '[REDACTED]')
    .replace(/(\b(?:__session|(?:id|refresh|access)[_-]?token|token|password|client[_-]?secret)\b["']?\s*[:=]\s*)(?:"[^"]*"|'[^']*'|[^;,\s}\]]+)/gi, '$1[REDACTED]')
    .replace(/\beyJ[A-Za-z0-9_-]*\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\b/g, '[REDACTED]')
    .replace(/\bAMf-vB[A-Za-z0-9_-]+\b/g, '[REDACTED]');
}

export function redactQaValue(value, secrets = []) {
  if (typeof value === 'string') {
    // Playwright evaluate arguments and Firebase storage often embed JSON text.
    try {
      const parsed = JSON.parse(value);
      if (parsed && typeof parsed === 'object') return JSON.stringify(redactQaValue(parsed, secrets));
    } catch { /* Plain diagnostic text. */ }
    return redactQaText(value, secrets);
  }
  if (Array.isArray(value)) return value.map((item) => redactQaValue(item, secrets));
  if (!value || typeof value !== 'object') return value;
  return Object.fromEntries(Object.entries(value).map(([key, item]) => [key,
    key.toLowerCase() === 'cookies' ? [] :
    key === 'v' && SECRET_KEY.test(value.k ?? '') ? (/^cookies$/i.test(value.k) ? { a: [] } : { s: '[REDACTED]' }) :
    SECRET_KEY.test(key) || (key === 'value' && SECRET_KEY.test(value.name ?? ''))
      ? '[REDACTED]' : redactQaValue(item, secrets)]));
}

/** Drain page and context routes before close; ignore late handler rejections. */
export async function closeQaContext(context) {
  try {
    for (const page of context.pages()) await page.unrouteAll({ behavior: 'ignoreErrors' });
    await context.unrouteAll({ behavior: 'ignoreErrors' });
    await context.close();
  } catch (error) {
    throw new Error(redactQaText(error));
  }
}

function zipCrc(bytes) {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit++) crc = (crc >>> 1) ^ (crc & 1 ? 0xedb88320 : 0);
  }
  return (crc ^ 0xffffffff) >>> 0;
}

// Read the central directory (Playwright uses data descriptors), then rebuild
// a normal deflated ZIP. No optional ZIP dependency is needed by installed skills.
function scrubTraceZip(zip, secrets) {
  if (zip.length < 22) throw new Error('Invalid QA trace ZIP');
  let end = zip.length - 22;
  while (end >= Math.max(0, zip.length - 65557) && zip.readUInt32LE(end) !== 0x06054b50) end--;
  if (end < 0 || zip.readUInt32LE(end) !== 0x06054b50) throw new Error('Invalid QA trace ZIP');
  const count = zip.readUInt16LE(end + 10);
  let cursor = zip.readUInt32LE(end + 16);
  if (count === 0xffff || cursor === 0xffffffff || zip.readUInt16LE(end + 4) || zip.readUInt16LE(end + 6)) throw new Error('Unsupported QA trace ZIP');
  const bodies = [], directory = [];
  let offset = 0;
  for (let index = 0; index < count; index++) {
    if (zip.readUInt32LE(cursor) !== 0x02014b50) throw new Error('Invalid QA trace entry');
    const flags = zip.readUInt16LE(cursor + 8), method = zip.readUInt16LE(cursor + 10);
    const size = zip.readUInt32LE(cursor + 20), nameLength = zip.readUInt16LE(cursor + 28);
    const localOffset = zip.readUInt32LE(cursor + 42);
    if ((flags & 1) || ![0, 8].includes(method) || size === 0xffffffff || localOffset === 0xffffffff) throw new Error('Unsupported QA trace entry');
    const name = zip.subarray(cursor + 46, cursor + 46 + nameLength);
    if (zip.readUInt32LE(localOffset) !== 0x04034b50) throw new Error('Invalid QA trace local entry');
    const start = localOffset + 30 + zip.readUInt16LE(localOffset + 26) + zip.readUInt16LE(localOffset + 28);
    let bytes = zip.subarray(start, start + size);
    if (method === 8) bytes = inflateRawSync(bytes);
    const text = bytes.toString('utf8');
    // Images remain byte-identical. Scrub all UTF-8 resources, including
    // extensionless JSON response bodies, sources, network and action records.
    if (Buffer.from(text).equals(bytes)) {
      bytes = Buffer.from(text.split('\n').map((line) => {
        try { return JSON.stringify(redactQaValue(JSON.parse(line), secrets)); }
        catch { return redactQaText(line, secrets); }
      }).join('\n'));
    }
    const compressed = deflateRawSync(bytes), crc = zipCrc(bytes);
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50); local.writeUInt16LE(20, 4); local.writeUInt16LE(0x800, 6); local.writeUInt16LE(8, 8);
    local.writeUInt32LE(crc, 14); local.writeUInt32LE(compressed.length, 18); local.writeUInt32LE(bytes.length, 22); local.writeUInt16LE(name.length, 26);
    const central = Buffer.alloc(46);
    central.writeUInt32LE(0x02014b50); central.writeUInt16LE(20, 4); central.writeUInt16LE(20, 6); central.writeUInt16LE(0x800, 8); central.writeUInt16LE(8, 10);
    central.writeUInt32LE(crc, 16); central.writeUInt32LE(compressed.length, 20); central.writeUInt32LE(bytes.length, 24); central.writeUInt16LE(name.length, 28); central.writeUInt32LE(offset, 42);
    bodies.push(local, name, compressed); directory.push(central, name);
    offset += local.length + name.length + compressed.length;
    cursor += 46 + nameLength + zip.readUInt16LE(cursor + 30) + zip.readUInt16LE(cursor + 32);
  }
  const central = Buffer.concat(directory), footer = Buffer.alloc(22);
  footer.writeUInt32LE(0x06054b50); footer.writeUInt16LE(count, 8); footer.writeUInt16LE(count, 10); footer.writeUInt32LE(central.length, 12); footer.writeUInt32LE(offset, 16);
  return Buffer.concat([...bodies, central, footer]);
}

/** Publish only a scrubbed trace; failed scrubs remove the private raw archive. */
export async function saveQaTrace(context, path, { secrets = [] } = {}) {
  const privateDir = await mkdtemp(join(tmpdir(), 'qa-trace-'));
  const privatePath = join(privateDir, 'raw.zip');
  try {
    await context.tracing.stop({ path: privatePath });
    await writeFile(path, scrubTraceZip(await readFile(privatePath), secrets));
  } catch (error) {
    await rm(path, { force: true });
    throw new Error(redactQaText(error, secrets));
  } finally {
    await rm(privateDir, { recursive: true, force: true });
  }
}

/**
 * Scrub a finished Playwright test-runner trace.zip. The runner's own
 * test.trace keeps every assertion's outcome, which the close gate counts; only
 * credentials are replaced. A failed scrub writes nothing at `output`.
 */
export async function scrubQaTraceFile(input, output, { secrets = [] } = {}) {
  try {
    await writeFile(output, scrubTraceZip(await readFile(input), secrets));
  } catch (error) {
    await rm(output, { force: true });
    throw new Error(redactQaText(error, secrets));
  }
}

const DEFAULT_VIEWPORTS = [
  { name: 'desktop', width: 1280, height: 800 },
  { name: 'phone', width: 390, height: 800 },
];
const DEFAULT_SCHEMES = ['light', 'dark'];
const CONTRAST_LIMIT = 4.5;
const LARGE_TEXT_LIMIT = 3;
const BOX_TOLERANCE = 1;

const PAGE_INSPECTION = ({ colorScheme, contrastLimit, largeTextLimit, boxTolerance, allowlistEntries = [] }) => {
    const fallback = colorScheme === 'dark' ? [17, 24, 39, 1] : [255, 255, 255, 1];
    const body = document.body;

    const round = (value) => Math.round(value * 100) / 100;
    const sample = (value, length = 96) => value.replace(/\s+/g, ' ').trim().slice(0, length);
    const convertedColors = new Map();
    let colorContext;
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
      // Let the rendering browser convert CSS Color 4 (color(), Lab/LCH,
      // OKLab/OKLCH, color-mix(), etc.) to the same sRGB pixels we inspect.
      // A detached, cached 1px canvas avoids duplicating gamut conversions or
      // adding an element to the inspected page. Preserve explicit alpha;
      // otherwise byte rounding could turn a translucent layer into opaque.
      if (convertedColors.has(value)) return convertedColors.get(value);
      let color = null;
      if (CSS.supports('color', value)) {
        if (!colorContext) {
          const canvas = document.createElement('canvas');
          canvas.width = canvas.height = 1;
          colorContext = canvas.getContext('2d', { colorSpace: 'srgb', willReadFrequently: true });
        }
        if (colorContext) {
          // fillStyle leaves its old value when assignment is unsupported.
          // Two sentinels distinguish that from a valid color equal to either.
          colorContext.fillStyle = '#010203';
          colorContext.fillStyle = value;
          const first = colorContext.fillStyle;
          colorContext.fillStyle = '#040506';
          colorContext.fillStyle = value;
          if (colorContext.fillStyle === first) {
            colorContext.clearRect(0, 0, 1, 1);
            colorContext.fillRect(0, 0, 1, 1);
            const pixels = colorContext.getImageData(0, 0, 1, 1).data;
            const opacity = value.match(/\/\s*([-+\d.eE]+%?)\s*\)$/);
            const alpha = opacity ? Math.min(1, Math.max(0, Number.parseFloat(opacity[1]) / (opacity[1].endsWith('%') ? 100 : 1))) : pixels[3] / 255;
            color = [pixels[0], pixels[1], pixels[2], alpha];
          }
        }
      }
      convertedColors.set(value, color);
      return color;
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
    // GH #1081: a visually-hidden helper keeps a positioned box of at most one
    // pixel and clips what it holds, so screen readers still read it. Its text
    // can never fit that box by design, whatever class names it carries.
    const visuallyHiddenBox = (current) => {
      const style = getComputedStyle(current);
      if (style.position !== 'absolute' && style.position !== 'fixed') return false;
      const rect = current.getBoundingClientRect();
      if (rect.width > 1.5 || rect.height > 1.5) return false;
      return (style.clip && style.clip !== 'auto') || /inset\(\s*50%/.test(style.clipPath || '') || ['hidden', 'clip'].includes(style.overflowX) || ['hidden', 'clip'].includes(style.overflowY);
    };
    // A closed off-canvas drawer is moved fully off the screen and holds
    // nothing a keyboard can reach; its text is not on the page for anyone.
    // An off-screen subtree a keyboard CAN reach is still reported.
    const focusableSelector = 'a[href], button, input, select, textarea, summary, iframe, [tabindex], [contenteditable=""], [contenteditable="true"]';
    const reachable = (element) => !element.disabled && element.tabIndex >= 0 && !element.closest('[inert]') && visibility(element).hidden === false;
    const offCanvasCache = new Map();
    const closedOffCanvas = (element) => {
      for (const current of ancestors(element)) {
        if (current === document.documentElement || current === document.body) continue;
        if (!offCanvasCache.has(current)) {
          const rect = current.getBoundingClientRect();
          const off = rect.width > 0 && rect.height > 0 && (rect.right <= boxTolerance || rect.left >= window.innerWidth - boxTolerance || rect.bottom <= boxTolerance);
          const keyboard = off && !current.closest('[inert]') && [current, ...current.querySelectorAll(focusableSelector)].some((candidate) => candidate.matches(focusableSelector) && reachable(candidate));
          offCanvasCache.set(current, off && !keyboard);
        }
        if (offCanvasCache.get(current)) return true;
      }
      return false;
    };
    // A closed <details> keeps a layout box for its content without drawing
    // it, so that content can sit past a scroller's range; it is folded, not
    // clipped. Only its summary is on screen, and opening it brings the rest
    // into the page's flow.
    const collapsedDisclosure = (element) => {
      for (let details = element.closest('details:not([open])'); details; details = details.parentElement?.closest('details:not([open])') ?? null) {
        const summary = details.querySelector(':scope > summary');
        if (!summary || !summary.contains(element)) return true;
      }
      return false;
    };
    // Content the engine skips rendering (an ancestor with
    // `content-visibility: hidden`, which is also how Chromium folds a closed
    // <details>) keeps a layout box but draws nothing. checkVisibility() is
    // the engine's own answer; `content-visibility: auto` off-screen content
    // stays visible to it, because scrolling renders it.
    const skippedContent = (element) => typeof element.checkVisibility === 'function' && !element.checkVisibility();
    const NON_TEXT_INPUTS = new Set(['checkbox', 'radio', 'range', 'color', 'file', 'hidden', 'button', 'submit', 'reset', 'image']);
    const editableField = (element) => (element.tagName === 'INPUT' && !NON_TEXT_INPUTS.has((element.getAttribute('type') || 'text').toLowerCase()))
      || element.tagName === 'TEXTAREA'
      || (element.hasAttribute('contenteditable') && element.isContentEditable === true);
    // A multi-line clamp is the design, not lost text: it hides the later
    // lines and draws its own ellipsis at the cut. The legacy form
    // (-webkit-line-clamp) clamps only a vertical -webkit-box, which engines
    // report as flow-root, and always draws that ellipsis whatever
    // text-overflow says; on any other box the count does nothing and hidden
    // lines are lost. The standard line-clamp clamps any block. Either way the
    // box must clip vertically and not also overflow sideways.
    const lineClampBox = (element, style = getComputedStyle(element)) => {
      const count = (value) => Number.parseInt(value || '', 10);
      const legacy = count(style.webkitLineClamp) > 0 && style.webkitBoxOrient === 'vertical';
      const standard = count(style.getPropertyValue('line-clamp')) > 0;
      return (legacy || standard) && style.display !== 'inline'
        && (style.overflowY === 'hidden' || style.overflowY === 'clip')
        && element.scrollWidth <= element.clientWidth + boxTolerance;
    };
    const nonVisualReason = (element) => {
      if (!element) return null;
      if (ariaHidden(element)) return 'aria-hidden';
      if (element.closest('svg title, svg desc')) return 'svg-accessibility-text';
      if (element.closest('.skip, .sr, .sr-only, .visually-hidden, .visuallyHidden, .nuxt-route-announcer, [data-visual-qa-hidden]')) return 'accessibility-helper';
      if (ancestors(element).some(visuallyHiddenBox)) return 'visually-hidden';
      if (closedOffCanvas(element)) return 'closed-off-canvas';
      if (collapsedDisclosure(element)) return 'collapsed-disclosure';
      if (skippedContent(element)) return 'content-skipped';
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
      // A skipped subtree is neither drawn nor focusable.
      hidden ||= skippedContent(element);
      return { opacity, hidden: hidden || opacity <= 0 };
    };
    const backgroundFor = (element) => {
      let background = fallback;
      let hasUnverifiableImage = false;
      let hasUnverifiableColor = false;
      for (const current of ancestors(element)) {
        const style = getComputedStyle(current);
        const color = cssColor(style.backgroundColor);
        // An opaque descendant covers the ancestor's unknown background.
        // Its own image still paints above its color, so inspect that last.
        if (color?.[3] === 1) {
          hasUnverifiableImage = false;
          hasUnverifiableColor = false;
        }
        if (!color) hasUnverifiableColor = true;
        if (color) background = over(color, background);
        if (style.backgroundImage && style.backgroundImage !== 'none') hasUnverifiableImage = true;
      }
      return { background, hasUnverifiableImage, hasUnverifiableColor };
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
        hasUnverifiableColor: background.hasUnverifiableColor,
        hidden: state.hidden,
        opacity: round(state.opacity),
        ariaHidden: ariaHidden(element),
        fontSize: Number.parseFloat(style.fontSize) || 16,
        fontWeight: Number.parseInt(style.fontWeight, 10) || 400,
        colorAlpha: fg ? fg[3] : null,
        ignored: Boolean(ignoredReason),
        ignoredReason,
        statusLike: Boolean(element.closest('.tag, .status, [role="status"]')),
        node,
        element,
      };
      textNodes.push(item);
    }

    const findings = [];
    const infos = [];
    const invalidAllowlistSelectors = [];
    for (const entry of allowlistEntries) {
      if (entry.selector === '*') continue;
      try {
        document.querySelector(entry.selector);
      } catch (error) {
        invalidAllowlistSelectors.push({
          type: 'invalid-allowlist-selector',
          selector: entry.selector,
          elementPath: 'document',
          textSample: entry.selector,
          reason: error instanceof Error ? error.message : String(error),
        });
      }
    }
    const allowlistedBy = (element, type) => allowlistEntries.filter((entry) => {
      if (entry.type !== '*' && entry.type !== type) return false;
      if (entry.selector === '*') return true;
      try {
        return element.matches(entry.selector) || Boolean(element.closest(entry.selector));
      } catch {
        return false;
      }
    }).map(({ type: entryType, selector, reason }) => ({ type: entryType, selector, reason }));
    const findingFor = (type, item, details = {}) => ({
      type,
      selector: item?.selector || item?.elementPath || 'document',
      elementPath: item?.elementPath || item?.selector || 'document',
      // Identity evidence for cross-build comparison, independent of CSS names.
      // Preserve ancestorBox as well so historical clipping reports still pair.
      textSample: item?.text || (item?.element ? sample(item.element.textContent || '') : undefined),
      textBounds: item?.box || (item?.element ? box(item.element.getBoundingClientRect()) : undefined),
      allowlistedBy: item?.element ? allowlistedBy(item.element, type) : [],
      ...details,
    });
    const add = (type, item, details = {}) => findings.push(findingFor(type, item, details));
    const addInfo = (type, item, details = {}) => infos.push(findingFor(type, item, details));
    const visibleText = textNodes.filter((item) => !item.ignored && !item.hidden && !item.ariaHidden && item.box.width > 0 && item.box.height > 0);
    for (const item of textNodes) {
      if (item.ignored || item.ariaHidden || item.box.width <= 0 || item.box.height <= 0) continue;
      if (item.hidden || item.opacity <= 0 || item.colorAlpha === 0) {
        add('invisible-text', item, { reason: item.opacity <= 0 ? 'opacity-0' : item.colorAlpha === 0 ? 'color-alpha-0' : 'visibility-hidden' });
        continue;
      }
      if (!item.foreground || item.hasUnverifiableImage || item.hasUnverifiableColor) {
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
      const item = { selector: element.id ? '#' + CSS.escape(element.id) : path, elementPath: path, element };
      const overflowX = style.overflowX === 'hidden' || style.overflowX === 'clip';
      const overflowY = style.overflowY === 'hidden' || style.overflowY === 'clip';
      const contentExceedsBorder = element !== document.documentElement && element !== document.body && (element.scrollWidth > element.clientWidth + boxTolerance || element.scrollHeight > element.clientHeight + boxTolerance);
      // A long value scrolling sideways inside an editable field is how
      // editing works, not clipped copy: the field's own horizontal scroll is
      // not measured. A container that clips the field itself still is.
      const editsInPlace = editableField(element);
      const clipped = (overflowX && !editsInPlace && element.scrollWidth > element.clientWidth + boxTolerance) || (overflowY && element.scrollHeight > element.clientHeight + boxTolerance);
      // GH #1081: an explicit single-line ellipsis is the design, not lost text.
      const singleLineEllipsis = overflowX && style.textOverflow === 'ellipsis' && element.scrollHeight <= element.clientHeight + boxTolerance;
      const intentionalClamp = lineClampBox(element, style);
      const intentionalEllipsis = singleLineEllipsis || intentionalClamp;
      if (clipped && !intentionalEllipsis) {
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

    // Each axis is judged on its own (GH #1073). A vertical scroller
    // (`overflow-x: hidden; overflow-y: auto`) clips sideways but scrolls
    // down: text below its fold is reachable, so it ends the vertical walk
    // instead of counting as a clip boundary on both axes. Only text outside
    // the scroller's scrollable range is unreachable. The horizontal axis is
    // unchanged: a horizontal scroller above the text's own parent ends it.
    for (const item of textNodes) {
      if (item.ignored || item.ariaHidden || item.box.width <= 0 || item.box.height <= 0) continue;
      const svgBoundary = item.node.parentElement?.closest('svg');
      let ancestor = item.node.parentElement;
      let checkX = true;
      let checkY = true;
      while (ancestor && (checkX || checkY)) {
        if (ancestor === svgBoundary) break;
        const style = getComputedStyle(ancestor);
        if (ancestor !== item.node.parentElement && ['auto', 'scroll'].includes(style.overflowX)) checkX = false;
        const clipsX = checkX && (style.overflowX === 'hidden' || style.overflowX === 'clip');
        const clipsY = checkY && (style.overflowY === 'hidden' || style.overflowY === 'clip');
        const scrollsY = checkY && (style.overflowY === 'auto' || style.overflowY === 'scroll');
        if (clipsX || clipsY || scrollsY) {
          const ancestorBox = ancestor.getBoundingClientRect();
          const ellipsisX = clipsX && style.textOverflow === 'ellipsis' && ancestor.scrollHeight <= ancestor.clientHeight + boxTolerance;
          const outsideX = clipsX && !ellipsisX && (item.box.x < ancestorBox.x - boxTolerance || item.box.right > ancestorBox.right + boxTolerance);
          // A clamping box decides which lines are drawn; the hidden lines'
          // range is not lost text, so the walk stops on Y there.
          const clampY = clipsY && lineClampBox(ancestor, style);
          const outsideY = clipsY && !clampY && (item.box.y < ancestorBox.y - boxTolerance || item.box.bottom > ancestorBox.bottom + boxTolerance);
          if (outsideX || outsideY) add('clipped-content', item, { reason: 'text-bounds-exceed-overflow-ancestor', ancestorPath: selectorFor(ancestor), ancestorBox: box(ancestorBox) });
          // An ellipsising box decides which part of its line is
          // drawn. The text range still measures the whole unellipsised line,
          // so a clipping ancestor further up (a title row with overflow-x:
          // clip) would see that phantom width; the walk stops on X here.
          if (ellipsisX) checkX = false;
          if (clampY) checkY = false;
          if (scrollsY) {
            const contentTop = ancestorBox.y + ancestor.clientTop - ancestor.scrollTop;
            const contentBottom = contentTop + ancestor.scrollHeight;
            if (item.box.y < contentTop - boxTolerance || item.box.bottom > contentBottom + boxTolerance) add('clipped-content', item, { reason: 'text-outside-scroll-range', ancestorPath: selectorFor(ancestor), ancestorBox: box(ancestorBox), scrollTop: ancestor.scrollTop, scrollHeight: ancestor.scrollHeight });
            checkY = false;
          }
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
        const captionItem = { selector: caption.id ? `#${CSS.escape(caption.id)}` : selectorFor(caption), elementPath: selectorFor(caption), text: sample(caption.textContent || ''), box: box(captionBox), element: caption };
        add(captionOverlapsContent ? 'overlapping-text' : 'clipped-content', captionItem, { reason: captionOverlapsContent ? 'figure-caption-layout-overlap' : 'figure-caption-overflow', otherElementPath: selectorFor(figure), otherTextSample: '', intersectionArea: round(Math.max(0, Math.min(captionBox.right, figureBox.right) - Math.max(captionBox.left, figureBox.left)) * Math.max(0, Math.min(captionBox.bottom, contentBottom) - Math.max(captionBox.top, figureBox.top))) });
      }
    }

    return { findings, infos, invalidAllowlistSelectors, textNodes: textNodes.map(({ node: _node, element: _element, ...item }) => item), viewport };
};

const STABLE_SEMVER = /^(\d+)\.(\d+)\.(\d+)$/;

function stableVersion(version) {
  const match = typeof version === 'string' ? version.match(STABLE_SEMVER) : null;
  return match ? match.slice(1).map(Number) : null;
}

function compareVersions(left, right) {
  for (let index = 0; index < 3; index += 1) if (left[index] !== right[index]) return left[index] - right[index];
  return 0;
}

function readPackageVersion(packageDir) {
  try {
    return JSON.parse(readFileSync(join(packageDir, 'package.json'), 'utf8')).version;
  } catch {
    return undefined;
  }
}

/**
 * Choose a Playwright package from the npx cache by version, never by
 * directory order: the highest stable release wins and prereleases (alphas,
 * betas, next builds) are never chosen. Returns null when no stable release is
 * cached, so the caller refuses rather than running on whatever sorts last.
 * @param {string} npxRoot the `_npx` cache directory
 * @returns {{packageDir: string, version: string} | null}
 */
export function selectCachedPlaywright(npxRoot = join(homedir(), '.npm', '_npx')) {
  if (!existsSync(npxRoot)) return null;
  let best = null;
  for (const entry of readdirSync(npxRoot, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue;
    const packageDir = join(npxRoot, entry.name, 'node_modules', 'playwright');
    const version = readPackageVersion(packageDir);
    const parsed = stableVersion(version);
    if (!parsed) continue;
    if (!best || compareVersions(parsed, best.parsed) > 0) best = { packageDir, version, parsed };
  }
  return best ? { packageDir: best.packageDir, version: best.version } : null;
}

async function importPlaywrightPackage(packageDir) {
  const packageJson = JSON.parse(readFileSync(join(packageDir, 'package.json'), 'utf8'));
  const entry = packageJson.exports?.['.']?.import || packageJson.module || packageJson.main || 'index.js';
  const entryPath = join(packageDir, typeof entry === 'string' ? entry : 'index.js');
  return await import(pathToFileURL(entryPath).href);
}

/**
 * Resolve Playwright and say which one: `npm exec --package=playwright` (the
 * supported entry, which installs the current release) puts its bin on PATH;
 * PLAYWRIGHT_MODULE names an explicit package; the npx cache is only consulted
 * by version (see selectCachedPlaywright).
 * @returns {Promise<{playwright: any, version: string, source: string}>}
 */
export async function resolvePlaywright() {
  try {
    const playwright = await import('playwright');
    let version = 'unknown';
    try {
      version = createRequire(import.meta.url)('playwright/package.json').version ?? version;
    } catch {
      // The module loaded; its manifest is only for the version line.
    }
    return { playwright, version, source: 'node_modules' };
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
    let source = explicit ? 'PLAYWRIGHT_MODULE' : 'PATH';
    if (!packageDir || !existsSync(join(packageDir, 'package.json'))) {
      const cached = selectCachedPlaywright();
      packageDir = cached?.packageDir || '';
      source = 'npx cache (highest stable)';
    }
    if (!packageDir) throw new Error('Playwright is required. Run with `npm exec --yes --package=playwright -- node scripts/visual-qa.mjs ...` or set PLAYWRIGHT_MODULE.');
    return { playwright: await importPlaywrightPackage(packageDir), version: readPackageVersion(packageDir) ?? 'unknown', source: `${source}: ${packageDir}` };
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
    `Playwright: ${result.playwrightVersion}  `,
    `Schemes: ${result.schemes.join(', ')}  `,
    `Viewports: ${result.viewports.map((viewport) => `${viewport.name} (${viewport.width}×${viewport.height})`).join(', ')}`,
    '',
    ...(result.pageDeclarations.length ? [
      '## Page declarations', '',
      ...result.pageDeclarations.map((page) => `- ${page.url}: JavaScript required — ${page.reason}`),
      '',
    ] : []),
    '## Findings',
    '',
  ];
  if (!result.findings.length) lines.push('No findings.');
  for (const finding of result.findings) {
    const location = `${finding.url}${finding.state ? ` · state ${finding.state}` : ''} · ${finding.scheme} · ${finding.viewport.name}`;
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
  if (result.journeyRuns) {
    lines.push('', `## Journey states — ${result.journey.name}`, '');
    for (const run of result.journeyRuns) {
      lines.push(`- **${run.state}** · ${run.scheme} · ${run.viewport.name} — screenshot [${run.screenshot}](${run.screenshot})${run.trace ? ` · trace [${run.trace}](${run.trace})` : ''}`);
      for (const step of run.steps) lines.push(`  ${step.index}. ${step.status === 'ok' ? 'ok' : step.status.toUpperCase()} — \`${step.step}\`${step.error ? ` — ${step.error}` : ''}`);
    }
  }
  lines.push('', '## Screenshots', '');
  for (const screenshot of result.screenshots) lines.push(`- [${screenshot.path}](${screenshot.path}) — ${screenshot.url}${screenshot.state ? ` · state ${screenshot.state}` : ''} · ${screenshot.scheme} · ${screenshot.viewport.name}`);
  lines.push('', '## Method', '', `Headless Chromium rendered each URL under the requested color schemes and viewports${result.journeyRuns ? ', then drove each declared journey state (routed responses, offline, steps) and inspected the page it reached' : ''}. Text nodes were checked for effective WCAG contrast, clipping, overlap, visibility, viewport escape, and fixed-size truncation.`);
  return `${lines.join('\n')}\n`;
}

/* ---- Declared journeys ---------------------------------------------------
 * A resting page can pass while the states a user reaches do not: a submit
 * that fails, a request still loading, the connection gone. A journey file
 * declares those states so strict visual QA renders and inspects them too,
 * on every scheme and viewport, with a screenshot and a Playwright trace each:
 *
 *   {
 *     "name": "start",
 *     "url": "start.html",            // relative to the journey file, or a URL
 *     "timeoutMs": 5000,              // per step (optional)
 *     "states": [
 *       { "name": "submit-error",
 *         "routes": [{ "url": "**\/api/start", "status": 500, "json": { "error": "boom" } }],
 *         "steps": [
 *           { "fill": "#email", "value": "ada@example.com" },
 *           { "click": "button[type=submit]" },
 *           { "waitFor": "[role=alert]" },
 *           { "expect": "#email", "focused": true }
 *         ] },
 *       { "name": "loading", "routes": [{ "url": "**\/api/**", "hold": true }], "steps": [ ... ] },
 *       { "name": "offline", "offline": true, "steps": [ ... ] }
 *     ]
 *   }
 *
 * Routes answer matching requests (status, json | body, contentType, headers,
 * delayMs), hold them open (loading), or abort them. `offline` goes offline
 * after the page has loaded, before the steps. Steps: fill (+value), click,
 * press (+on), focus, hover, check, select (+value), waitFor (+state), wait
 * (ms), offline (true/false), expect (+visible, hidden, text, focused). A step
 * that cannot run is a `journey-step-failed` finding; an expectation that does
 * not hold is a `journey-expectation` finding; either fails a strict run. The
 * page is then inspected exactly as a resting page is.
 */
const STEP_ACTIONS = ['fill', 'click', 'press', 'focus', 'hover', 'check', 'select', 'waitFor', 'wait', 'offline', 'expect'];

function stepAction(step) {
  const actions = STEP_ACTIONS.filter((action) => Object.hasOwn(step, action));
  if (actions.length !== 1) throw new Error(`Journey step ${JSON.stringify(step)} must name exactly one action: ${STEP_ACTIONS.join(', ')}.`);
  return actions[0];
}

function describeStep(step) {
  const action = stepAction(step);
  const target = step[action];
  if (action === 'wait') return `wait ${target}ms`;
  if (action === 'offline') return `offline ${target}`;
  const extras = ['value', 'on', 'state', 'text', 'visible', 'hidden', 'focused'].filter((key) => Object.hasOwn(step, key)).map((key) => `${key}=${JSON.stringify(step[key])}`);
  return [action, typeof target === 'string' ? target : JSON.stringify(target), ...extras].join(' ');
}

/**
 * Read and check a journey: a path to a JSON file, or the object itself.
 * Relative URLs resolve against the journey file's directory (or the cwd).
 */
export async function loadJourney(journey) {
  const fromFile = typeof journey === 'string';
  const parsed = fromFile ? JSON.parse(await readFile(journey, 'utf8')) : journey;
  if (!parsed || typeof parsed !== 'object' || !Array.isArray(parsed.states) || !parsed.states.length) throw new Error('A journey needs a non-empty states array.');
  const base = fromFile ? dirname(resolve(journey)) : process.cwd();
  const url = parsed.url === undefined ? undefined : /^(?:file|https?):\/\//i.test(parsed.url) ? parsed.url : pathToFileURL(resolve(base, parsed.url)).href;
  const names = new Set();
  const states = parsed.states.map((state, index) => {
    if (!state || typeof state !== 'object' || typeof state.name !== 'string' || !state.name.trim()) throw new Error(`Journey state ${index + 1} needs a name.`);
    if (names.has(state.name)) throw new Error(`Journey state name ${state.name} is used twice.`);
    names.add(state.name);
    const steps = state.steps ?? [];
    if (!Array.isArray(steps)) throw new Error(`Journey state ${state.name}: steps must be an array.`);
    for (const step of steps) stepAction(step);
    const routes = state.routes ?? [];
    if (!Array.isArray(routes) || routes.some((route) => !route || typeof route.url !== 'string')) throw new Error(`Journey state ${state.name}: every route needs a url pattern.`);
    return { name: state.name, offline: state.offline === true, routes, steps };
  });
  return { name: parsed.name || 'journey', url, timeoutMs: Number(parsed.timeoutMs) > 0 ? Number(parsed.timeoutMs) : 5000, states };
}

async function installRoute(page, route, holds) {
  await page.route(route.url, async (request) => {
    try {
      if (route.hold) { await new Promise((release) => holds.push(release)); await request.abort(); return; }
      if (route.abort) { await request.abort(typeof route.abort === 'string' ? route.abort : 'failed'); return; }
      if (route.delayMs) await new Promise((wake) => setTimeout(wake, route.delayMs));
      const json = Object.hasOwn(route, 'json');
      await request.fulfill({
        status: route.status ?? 200,
        contentType: route.contentType ?? (json ? 'application/json' : 'text/plain'),
        // A file: page's fetch is cross-origin; the declared answer must reach it.
        headers: { 'access-control-allow-origin': '*', ...(route.headers ?? {}) },
        body: json ? JSON.stringify(route.json) : String(route.body ?? ''),
      });
    } catch {
      // The context closed while the request was held: nothing left to answer.
    }
  });
}

async function runStep(page, context, step, timeout) {
  const action = stepAction(step);
  const target = step[action];
  const locator = typeof target === 'string' ? page.locator(target).first() : undefined;
  switch (action) {
    case 'fill': await locator.fill(String(step.value ?? ''), { timeout }); return undefined;
    case 'click': await locator.click({ timeout }); return undefined;
    case 'press': if (step.on) await page.locator(step.on).first().press(target, { timeout }); else await page.keyboard.press(target); return undefined;
    case 'focus': await locator.focus({ timeout }); return undefined;
    case 'hover': await locator.hover({ timeout }); return undefined;
    case 'check': await locator.check({ timeout }); return undefined;
    case 'select': await locator.selectOption(String(step.value ?? ''), { timeout }); return undefined;
    case 'waitFor': await locator.waitFor({ state: step.state ?? 'visible', timeout }); return undefined;
    case 'wait': await page.waitForTimeout(Number(target) || 0); return undefined;
    case 'offline': await context.setOffline(target !== false); return undefined;
    case 'expect': {
      const deadline = Date.now() + timeout;
      let last = '';
      for (;;) {
        last = await page.evaluate(({ selector, expected }) => {
          const element = document.querySelector(selector);
          const shown = Boolean(element && element.getClientRects().length && getComputedStyle(element).visibility !== 'hidden');
          if (expected.hidden === true) return shown ? 'is visible' : '';
          if (!element) return 'is missing';
          if (expected.visible !== false && !shown) return 'is not visible';
          if (expected.text !== undefined && !(element.innerText || element.textContent || '').includes(expected.text)) return `does not contain ${JSON.stringify(expected.text)} (has ${JSON.stringify((element.innerText || '').trim().slice(0, 80))})`;
          if (expected.focused === true && document.activeElement !== element) {
            const active = document.activeElement;
            return `is not focused (focus is on ${active === document.body || !active ? 'the page body' : active.tagName.toLowerCase() + (active.id ? '#' + active.id : '')})`;
          }
          return '';
        }, { selector: target, expected: step });
        if (!last || Date.now() >= deadline) break;
        await page.waitForTimeout(50);
      }
      return last ? `${target} ${last}` : undefined;
    }
    default: throw new Error(`Unknown journey step ${action}.`);
  }
}

/**
 * Settle the page before it is measured. A fixed wait captures colours
 * mid-transition, so contrast findings vary between runs. Every finite CSS
 * transition and animation (and any Web Animation) is finished at its end
 * state; an infinite one is paused at its start so every run reads the same
 * frame. Repeats until no finite animation is left, then waits two frames so
 * the settled styles are painted.
 */
async function settlePage(page) {
  await page.evaluate(async () => {
    const frame = () => new Promise((resolve) => requestAnimationFrame(() => resolve()));
    for (let round = 0; round < 10; round += 1) {
      await frame();
      let running = 0;
      for (const animation of document.getAnimations()) {
        if (animation.playState === 'finished') continue;
        try {
          animation.finish();
          running += 1;
        } catch {
          if (animation.playState !== 'paused') animation.pause();
          animation.currentTime = 0;
        }
      }
      if (!running) break;
    }
    await frame();
    await frame();
  });
}

/**
 * Render and inspect one or more HTML URLs.
 * With `journey` (a journey file path or object, see loadJourney), its
 * declared states are rendered and inspected after the resting pages.
 * Authenticated callers pass storageState (seeded before tracing), optional
 * extraHTTPHeaders, and secrets for any opaque credential values.
 * @param {{urls?: string[], artifactDir?: string, schemes?: string[], viewports?: Array<{name?: string,width:number,height:number}|string>, allowlistPath?: string, strict?: boolean, journey?: string | object}} options
 */
export async function runVisualQa(options) {
  try { return await inspectVisualQa(options); }
  catch (error) { throw new Error(redactQaText(error, options?.secrets)); }
}

async function inspectVisualQa(options) {
  const journey = options?.journey ? await loadJourney(options.journey) : undefined;
  const inputUrls = [...(options?.urls ?? [])];
  // The journey's page is checked at rest too, like any URL given.
  if (journey?.url && !inputUrls.includes(journey.url)) inputUrls.push(journey.url);
  if (!inputUrls.length) throw new Error('At least one file:// or http(s) URL is required.');
  if (journey && !journey.url && inputUrls.length !== 1) throw new Error('A journey without a url needs exactly one URL to run on.');
  const artifactDir = resolve(options.artifactDir || join(process.cwd(), 'docs/factory/data/visual-qa'));
  const urls = inputUrls.map((url) => /^(?:file|https?):\/\//i.test(url) ? url : pathToFileURL(resolve(url)).href);
  const schemes = options.schemes || DEFAULT_SCHEMES;
  const viewports = (options.viewports || DEFAULT_VIEWPORTS).map(normalizeViewport);
  if (!schemes.length || !viewports.length) throw new Error('No captures requested: at least one color scheme and viewport are required.');
  const allowlist = await loadAllowlist(options.allowlistPath);
  await mkdir(artifactDir, { recursive: true });
  const { playwright, version: playwrightVersion, source: playwrightSource } = await resolvePlaywright();
  console.log(redactQaText(`Playwright ${playwrightVersion} (${playwrightSource})`, options.secrets));
  const browser = await playwright.chromium.launch({ headless: true, executablePath: systemChromium() });
  const findings = [];
  const infoFindings = [];
  const suppressed = [];
  const screenshots = [];
  const pageDeclarations = [];
  const journeyRuns = [];
  const seen = new Set();
  let warnedUnownedTrace = false;
  try {
    for (const [urlIndex, url] of urls.entries()) {
      const source = inputUrls[urlIndex];
      for (const scheme of schemes) {
        for (const viewport of viewports) {
          const context = await browser.newContext({ storageState: options.storageState, extraHTTPHeaders: options.extraHTTPHeaders, colorScheme: scheme, viewport: { width: viewport.width, height: viewport.height } });
          const page = await context.newPage();
          try {
            await page.goto(url, { waitUntil: 'load' });
            await settlePage(page);
            const recordFinding = (finding, informational = false) => {
              const enriched = { ...finding, url: source, scheme, viewport };
              const key = [informational ? 'info' : 'finding', 'rest', enriched.type, enriched.selector || enriched.elementPath, enriched.otherElementPath || '', scheme, viewport.name].join('|');
              if (seen.has(key)) return;
              seen.add(key);
              const exception = enriched.allowlistedBy?.[0] ?? allowlist.find((entry) =>
                (entry.type === '*' || entry.type === enriched.type)
                && (entry.selector === '*' || (entry.selector === enriched.selector && !enriched.allowlistedBy)));
              if (exception) suppressed.push({ ...enriched, reason: exception.reason });
              else if (informational) infoFindings.push(enriched);
              else findings.push(enriched);
            };
            const inspection = await page.evaluate(PAGE_INSPECTION, { colorScheme: scheme, contrastLimit: CONTRAST_LIMIT, largeTextLimit: LARGE_TEXT_LIMIT, boxTolerance: BOX_TOLERANCE, allowlistEntries: allowlist });
            for (const invalid of inspection.invalidAllowlistSelectors) recordFinding(invalid, true);
            for (const finding of inspection.findings) recordFinding(finding);

            // A reviewed application declaration exempts only the no-JS
            // comparison. All visual and print checks still apply. Undeclared
            // pages, including reports, retain the default no-JS requirement.
            const requirement = await page.evaluate(() => {
              const declarations = document.querySelectorAll('head meta[name="visual-qa:requires-javascript"]');
              if (!declarations.length) return null;
              return { count: declarations.length, reason: declarations[0].getAttribute('content')?.trim() ?? '' };
            });
            const requiresJavaScript = requirement?.count === 1 && Boolean(requirement.reason);
            if (requirement && !requiresJavaScript) {
              recordFinding({ type: 'invalid-javascript-requirement', selector: 'meta[name="visual-qa:requires-javascript"]',
                elementPath: 'head > meta', reason: 'declare-one-javascript-requirement-with-a-nonempty-reason' });
            }
            if (requiresJavaScript && !pageDeclarations.some((page) => page.url === source)) {
              pageDeclarations.push({ url: source, requiresJavaScript: true, reason: requirement.reason });
            }

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

            if (!requiresJavaScript) {
              const noScriptContext = await browser.newContext({ storageState: options.storageState, extraHTTPHeaders: options.extraHTTPHeaders, colorScheme: scheme, viewport: { width: viewport.width, height: viewport.height }, javaScriptEnabled: false });
              const noScriptPage = await noScriptContext.newPage();
              try {
                await noScriptPage.goto(url, { waitUntil: 'load' });
                const noScriptText = await noScriptPage.locator('body').innerText().catch(() => '');
                if (screenText.trim().length > 20 && noScriptText.trim().length < Math.max(1, Math.floor(screenText.trim().length * 0.8))) {
                  recordFinding({ type: 'javascript-disabled-loss', selector: 'body', elementPath: 'body', textSample: noScriptText.trim().slice(0, 96), reason: 'content-requires-javascript', screenCharacters: screenText.trim().length, javascriptDisabledCharacters: noScriptText.trim().length });
                }
              } finally {
                await closeQaContext(noScriptContext);
              }
            }

            const filename = `${slug(source)}-${scheme}-${viewport.name}.png`;
            screenshots.push({ path: filename, url: source, scheme, viewport });
          } finally {
            await closeQaContext(context);
          }
        }
      }
    }
    if (journey) {
      const source = journey.url ?? inputUrls[0];
      const url = /^(?:file|https?):\/\//i.test(source) ? source : pathToFileURL(resolve(source)).href;
      for (const state of journey.states) {
        for (const scheme of schemes) {
          for (const viewport of viewports) {
            const base = `${slug(source)}-${slug(state.name)}-${scheme}-${viewport.name}`;
            const run = { journey: journey.name, state: state.name, url: source, scheme, viewport, steps: [], screenshot: `${base}.png`, trace: `${base}.trace.zip` };
            journeyRuns.push(run);
            const recordFinding = (finding, informational = false) => {
              const enriched = { ...finding, url: source, state: state.name, scheme, viewport };
              const key = [informational ? 'info' : 'finding', `state:${state.name}`, enriched.type, enriched.selector || enriched.elementPath, enriched.otherElementPath || '', scheme, viewport.name].join('|');
              if (seen.has(key)) return;
              seen.add(key);
              const exception = enriched.allowlistedBy?.[0] ?? allowlist.find((entry) =>
                (entry.type === '*' || entry.type === enriched.type)
                && (entry.selector === '*' || (entry.selector === enriched.selector && !enriched.allowlistedBy)));
              if (exception) suppressed.push({ ...enriched, reason: exception.reason });
              else if (informational) infoFindings.push(enriched);
              else findings.push(enriched);
            };
            const context = await browser.newContext({ storageState: options.storageState, extraHTTPHeaders: options.extraHTTPHeaders, colorScheme: scheme, viewport: { width: viewport.width, height: viewport.height } });
            const holds = [];
            // The Playwright runner may already own this context trace.
            let tracing = false;
            try {
              await context.tracing.start({ screenshots: true, snapshots: true, title: `${journey.name} · ${state.name} · ${scheme} · ${viewport.name}` });
              tracing = true;
            } catch {
              run.trace = undefined;
              if (!warnedUnownedTrace) {
                console.warn(redactQaText('Warning: runner-owned trace is outside visual-qa and is not scrubbed.'));
                warnedUnownedTrace = true;
              }
            }
            const page = await context.newPage();
            try {
              for (const route of state.routes) await installRoute(page, route, holds);
              await page.goto(url, { waitUntil: 'load' });
              if (state.offline) await context.setOffline(true);
              let failed = false;
              for (const [index, step] of state.steps.entries()) {
                const entry = { index: index + 1, step: describeStep(step), status: 'skipped' };
                run.steps.push(entry);
                if (failed) continue;
                const started = Date.now();
                try {
                  const unmet = await runStep(page, context, step, journey.timeoutMs);
                  entry.status = unmet ? 'unmet' : 'ok';
                  if (unmet) {
                    entry.error = unmet;
                    recordFinding({ type: 'journey-expectation', selector: typeof step.expect === 'string' ? step.expect : 'journey', elementPath: typeof step.expect === 'string' ? step.expect : 'journey', reason: unmet, step: entry.index });
                  }
                } catch (error) {
                  failed = true;
                  entry.status = 'failed';
                  entry.error = redactQaText(error, options.secrets).split('\n')[0];
                  recordFinding({ type: 'journey-step-failed', selector: `step ${entry.index}`, elementPath: `step ${entry.index}`, reason: `${entry.step}: ${entry.error}`, step: entry.index });
                }
                entry.ms = Date.now() - started;
              }
              await settlePage(page);
              const inspection = await page.evaluate(PAGE_INSPECTION, { colorScheme: scheme, contrastLimit: CONTRAST_LIMIT, largeTextLimit: LARGE_TEXT_LIMIT, boxTolerance: BOX_TOLERANCE, allowlistEntries: allowlist });
              for (const invalid of inspection.invalidAllowlistSelectors) recordFinding(invalid, true);
              for (const finding of inspection.findings) recordFinding(finding);
              const screenshotBuffer = await page.screenshot({ path: join(artifactDir, run.screenshot), fullPage: true });
              for (const info of inspection.infos) {
                const sampledBackground = sampleScreenshotBackground(screenshotBuffer, info.box);
                const sampledRatio = sampledBackground && info.foreground ? rgbContrast(info.foreground, sampledBackground) : undefined;
                recordFinding({ ...info, sampledBackground, sampledRatio }, true);
              }
              screenshots.push({ path: run.screenshot, url: source, state: state.name, scheme, viewport });
            } catch (error) {
              recordFinding({ type: 'journey-step-failed', selector: 'journey', elementPath: 'journey', reason: `${state.name}: ${redactQaText(error, options.secrets).split('\n')[0]}` });
            } finally {
              for (const release of holds) release();
              if (tracing) await saveQaTrace(context, join(artifactDir, run.trace), { secrets: options.secrets }).catch(() => {
                run.trace = undefined;
                recordFinding({ type: 'journey-trace-failed', selector: 'journey', elementPath: 'journey', reason: 'Could not publish a scrubbed trace.' });
              });
              await closeQaContext(context);
            }
          }
        }
      }
    }
  } finally {
    await browser.close();
  }
  if (!screenshots.length) throw new Error('No captures produced; visual QA cannot pass without screenshots.');
  const result = redactQaValue({
    status: findings.length ? 'FAIL' : 'PASS',
    exitCode: findings.length && options.strict ? 1 : 0,
    // The close gate counts a claimed pass only from a strict run.
    strict: Boolean(options.strict),
    generatedAt: new Date().toISOString(),
    playwrightVersion,
    schemes,
    viewports,
    urls: inputUrls,
    pageDeclarations,
    ...(journey ? { journey: { name: journey.name, url: journey.url, states: journey.states.map((state) => state.name) }, journeyRuns } : {}),
    findings,
    infoFindings,
    suppressed,
    screenshots,
    counts: findings.reduce((counts, finding) => ({ ...counts, [finding.type]: (counts[finding.type] || 0) + 1 }), {}),
    infoCounts: infoFindings.reduce((counts, finding) => ({ ...counts, [finding.type]: (counts[finding.type] || 0) + 1 }), {}),
  }, options.secrets);
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
    else if (arg === '--journey') options.journey = argv[++index];
    else if (arg === '--scheme') options.schemes = [argv[++index]];
    else if (arg === '--viewport') options.viewports = [argv[++index]];
    else if (arg === '--scrub-trace') options.scrubTrace = [argv[++index], argv[++index]];
    else if (arg === '--help' || arg === '-h') options.help = true;
    else options.urls.push(arg);
  }
  return options;
}

if (process.argv[1] && existsSync(process.argv[1]) && realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url))) {
  const options = parseArgs(process.argv.slice(2));
  if (options.scrubTrace) {
    // Extra literal secrets come from the environment, never argv (process lists).
    const secrets = (process.env.QA_TRACE_SECRETS ?? '').split('\n').filter(Boolean);
    const [input, output] = options.scrubTrace;
    try {
      if (!input || !output) throw new Error('--scrub-trace needs an input and an output path');
      await scrubQaTraceFile(input, output, { secrets });
      console.log(`SCRUBBED ${output}`);
    } catch (error) {
      console.error(redactQaText(error, secrets));
      process.exitCode = 2;
    }
  } else if (options.help || (!options.urls.length && !options.journey)) {
    if (!options.help) console.error('No captures requested: at least one URL or journey is required.');
    console.log('Usage: npm exec --yes --package=playwright -- node scripts/visual-qa.mjs [--strict] [--artifact-dir DIR] [--allowlist FILE] [--journey FILE] [--scheme light|dark] [--viewport WIDTHxHEIGHT] [URL...]\n       node scripts/visual-qa.mjs --scrub-trace RAW.zip trace.zip   (extra literal secrets: QA_TRACE_SECRETS, newline-separated)');
    process.exitCode = options.help ? 0 : 2;
  } else {
    try {
      const result = await runVisualQa(options);
      if (result.status === 'PASS') console.log('PASS');
      else for (const finding of result.findings) {
        const colors = finding.foreground && finding.background ? ` foreground=${finding.foreground.join(',')} background=${finding.background.join(',')} ratio=${finding.ratio ?? 'n/a'}` : '';
        console.log(redactQaText(`FAIL ${finding.type}${finding.state ? ` [${finding.state}]` : ''} ${finding.elementPath} text=${JSON.stringify(finding.textSample || (finding.type.startsWith('journey-') ? finding.reason : '') || '')}${colors}`));
      }
      process.exitCode = result.exitCode;
    } catch (error) {
      console.error(redactQaText(error));
      process.exitCode = 2;
    }
  }
}
