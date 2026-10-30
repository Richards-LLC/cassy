import { appendEmojiText } from "./emoji-text";
/**
 * The deliberately small Markdown dialect accepted in supervisor replies.
 *
 * The renderer builds every node itself. Body text is never assigned to
 * innerHTML, which keeps tags, entities, and unsupported link schemes literal.
 */
import { createCodeFrame } from "./code-frame";

interface ListMarker {
  indent: number;
  ordered: boolean;
  content: string;
}

const FENCE = /^ {0,3}(`{3,}|~{3,})(.*)$/;
const HEADING = /^\s{0,3}#{1,6}(?:\s+|$)(.*)$/;
const CALLOUT = /^\s{0,3}>[ \t]?\[!(NOTE|WARNING|CAUTION)\][ \t]*$/i;
const QUOTE_LINE = /^\s{0,3}>[ \t]?/;
const LIST = /^(\s*)([-+*]|\d+[.)])\s+(.*)$/;
const LEGACY_NUMBER = /(?:\(\d+\)|\d+\))(?=[ \t]+)/g;
const ALL_CAPS_LABEL = "[A-Z][A-Z0-9]*(?:[ \\t]+[A-Z][A-Z0-9]*)*";
const LEAD = new RegExp(`^(?:Status\\s+\\d{1,2}:[\\dx]{2}Z\\.(?:[ \\t]+${ALL_CAPS_LABEL}:)?|[^.\\n]{1,120}\\.[ \\t]+${ALL_CAPS_LABEL}:)`);
const INLINE_MARKDOWN = /`[^`\n]+`|\*\*[^*\n]+\*\*|\*[^*\n]+\*|\[[^\]\n]+\]\([^\n)]+\)/;

function listMarker(line: string): ListMarker | undefined {
  const match = LIST.exec(line);
  if (!match) return undefined;
  return { indent: match[1]!.replace(/\t/g, "  ").length, ordered: /^\d/.test(match[2]!), content: match[3]! };
}

function isFence(line: string): boolean {
  const match = FENCE.exec(line);
  return match !== null && !(match[1]![0] === "`" && match[2]!.includes("`"));
}

function closesFence(line: string, opening: string): boolean {
  const match = FENCE.exec(line);
  return match !== null && match[1]![0] === opening[0] && match[1]!.length >= opening.length && !match[2]!.trim();
}

function isBlockStart(line: string): boolean {
  return isFence(line) || HEADING.test(line) || CALLOUT.test(line) || listMarker(line) !== undefined;
}

function leadingPlainText(source: string): string | undefined {
  const match = LEAD.exec(source);
  return match && match[0] ? match[0] : undefined;
}

function hasMarkdownMarkers(source: string): boolean {
  if (INLINE_MARKDOWN.test(source)) return true;
  return source.split("\n").some((line) => {
    if (isFence(line) || HEADING.test(line) || CALLOUT.test(line)) return true;
    const marker = listMarker(line);
    // A closing parenthesis is the legacy prose form handled below. A dot or
    // a bullet at line start remains the Markdown contract.
    return marker !== undefined && !/^\s*\d+\)\s+/.test(line);
  });
}

interface LegacyEnumeration {
  prefix: string;
  items: string[];
}

function legacyEnumeration(source: string): LegacyEnumeration | undefined {
  const markers: Array<{ index: number; end: number; number: number }> = [];
  for (const match of source.matchAll(LEGACY_NUMBER)) {
    const index = match.index ?? -1;
    if (index < 0 || (index > 0 && !/\s/.test(source[index - 1]!))) continue;
    markers.push({ index, end: index + match[0].length, number: Number.parseInt(match[0].replace(/[()]/g, ""), 10) });
  }
  if (markers.length < 2 || markers[0]!.number !== 1) return undefined;

  const prefix = source.slice(0, markers[0]!.index).trim();
  for (let index = 1; index < markers.length; index += 1) {
    if (markers[index]!.number !== markers[index - 1]!.number + 1) return undefined;
  }

  const items = markers.map((marker, index) => source.slice(marker.end, markers[index + 1]?.index ?? source.length).trim());
  if (items.some((item) => !item)) return undefined;
  return { prefix, items };
}

/**
 * Older supervisors sometimes put numbered prose in one line instead of
 * emitting Markdown. A recognized lead becomes a bold first paragraph; when
 * the lead is not recognizable, keep it as a plain paragraph before the list.
 */
function normalizePlainText(source: string): string {
  if (hasMarkdownMarkers(source)) return source;
  const enumeration = legacyEnumeration(source);
  if (enumeration) {
    const parts: string[] = [];
    if (enumeration.prefix) {
      parts.push(leadingPlainText(enumeration.prefix) === enumeration.prefix ? `# ${enumeration.prefix}` : enumeration.prefix);
    }
    parts.push(enumeration.items.map((item, index) => `${index + 1}. ${item}`).join("\n"));
    return parts.join("\n\n");
  }

  const lead = leadingPlainText(source);
  if (!lead) return source;
  return `# ${lead}${source.slice(lead.length)}`;
}

export function appendInline(document: Document, parent: HTMLElement, source: string): void {
  let index = 0;
  let plainStart = 0;
  const flush = (end: number): void => {
    if (end > plainStart) appendEmojiText(document, parent, source.slice(plainStart, end));
  };
  const nested = (element: HTMLElement, text: string): void => appendInline(document, element, text);

  while (index < source.length) {
    if (source[index] === "`") {
      const close = source.indexOf("`", index + 1);
      if (close > index + 1) {
        flush(index);
        const code = document.createElement("code");
        code.className = "markdown-inline-code";
        code.textContent = source.slice(index + 1, close);
        parent.append(code);
        index = close + 1;
        plainStart = index;
        continue;
      }
    }

    if (source[index] === "[") {
      const labelEnd = source.indexOf("](", index + 1);
      const urlEnd = labelEnd < 0 ? -1 : source.indexOf(")", labelEnd + 2);
      if (labelEnd > index + 1 && urlEnd > labelEnd + 2) {
        const label = source.slice(index + 1, labelEnd);
        const url = source.slice(labelEnd + 2, urlEnd);
        flush(index);
        if (/^https:\/\//i.test(url) && !/[\s<>]/.test(url)) {
          const link = document.createElement("a");
          link.className = "markdown-link";
          link.href = url;
          link.target = "_blank";
          link.rel = "noopener";
          nested(link, label);
          parent.append(link);
        } else {
          // Unsupported schemes remain one literal text node, including the
          // markdown markers, rather than becoming a partially parsed link.
          parent.append(document.createTextNode(source.slice(index, urlEnd + 1)));
        }
        index = urlEnd + 1;
        plainStart = index;
        continue;
      }
    }

    const strong = source.startsWith("**", index);
    if (strong) {
      const close = source.indexOf("**", index + 2);
      if (close > index + 2) {
        flush(index);
        const bold = document.createElement("strong");
        nested(bold, source.slice(index + 2, close));
        parent.append(bold);
        index = close + 2;
        plainStart = index;
        continue;
      }
    }

    if (source[index] === "*") {
      if (source[index + 1] !== "*") {
        const close = source.indexOf("*", index + 1);
        if (close > index + 1) {
          flush(index);
          const italic = document.createElement("em");
          nested(italic, source.slice(index + 1, close));
          parent.append(italic);
          index = close + 1;
          plainStart = index;
          continue;
        }
      }
    }
    index += 1;
  }
  flush(source.length);
}

function inlinePlainText(source: string): string {
  let output = "";
  let index = 0;
  while (index < source.length) {
    if (source[index] === "`") {
      const close = source.indexOf("`", index + 1);
      if (close > index + 1) { output += source.slice(index + 1, close); index = close + 1; continue; }
    }
    if (source[index] === "[") {
      const labelEnd = source.indexOf("](", index + 1);
      const urlEnd = labelEnd < 0 ? -1 : source.indexOf(")", labelEnd + 2);
      if (labelEnd > index + 1 && urlEnd > labelEnd + 2) {
        const label = source.slice(index + 1, labelEnd);
        const url = source.slice(labelEnd + 2, urlEnd);
        if (/^https:\/\//i.test(url) && !/[\s<>]/.test(url)) {
          output += inlinePlainText(label); index = urlEnd + 1; continue;
        }
        output += source.slice(index, urlEnd + 1); index = urlEnd + 1; continue;
      }
    }
    if (source.startsWith("**", index)) {
      const close = source.indexOf("**", index + 2);
      if (close > index + 2) { output += inlinePlainText(source.slice(index + 2, close)); index = close + 2; continue; }
    }
    if (source[index] === "*" && source[index + 1] !== "*") {
      const close = source.indexOf("*", index + 1);
      if (close > index + 1) { output += inlinePlainText(source.slice(index + 1, close)); index = close + 1; continue; }
    }
    output += source[index];
    index += 1;
  }
  return output;
}

function renderList(document: Document, lines: string[], start: number, baseIndent: number, ordered: boolean, depth: number): { node: HTMLElement; next: number } {
  const list = document.createElement(ordered ? "ol" : "ul");
  list.className = depth === 0 ? "markdown-list" : "markdown-list markdown-list-nested";
  let index = start;
  while (index < lines.length) {
    const marker = listMarker(lines[index]!);
    if (!marker || marker.indent !== baseIndent || marker.ordered !== ordered) break;
    const item = document.createElement("li");
    appendInline(document, item, marker.content);
    index += 1;
    while (index < lines.length) {
      const line = lines[index]!;
      if (!line.trim()) break;
      const nestedMarker = listMarker(line);
      if (nestedMarker && nestedMarker.indent > baseIndent) {
        if (depth === 0) {
          const nestedList = renderList(document, lines, index, nestedMarker.indent, nestedMarker.ordered, 1);
          item.append(nestedList.node);
          index = nestedList.next;
        } else {
          // A third level is outside the contract; preserve it as literal text.
          item.append(document.createTextNode(`\n${line.trim()}`));
          index += 1;
        }
        continue;
      }
      if (nestedMarker && nestedMarker.indent <= baseIndent) break;
      if (/^\s+/.test(line)) {
        item.append(document.createTextNode(`\n${line.trim()}`));
        index += 1;
        continue;
      }
      break;
    }
    list.append(item);
    if (index < lines.length && !lines[index]!.trim()) break;
  }
  return { node: list, next: index };
}

/** Render the supported reply subset as safe DOM nodes. */
export function renderMarkdown(document: Document, source: string): HTMLElement[] {
  return renderBlocks(document, source, true);
}

function renderBlocks(document: Document, source: string, allowCallouts: boolean): HTMLElement[] {
  // Keep original line separators for Copy; prose parsing still uses lines.
  const parts = normalizePlainText(source).split(/(\r\n|\r|\n)/);
  const lines = parts.filter((_, part) => part % 2 === 0);
  const nodes: HTMLElement[] = [];
  let index = 0;
  while (index < lines.length) {
    const line = lines[index]!;
    if (!line.trim()) { index += 1; continue; }

    if (isFence(line)) {
      const fence = FENCE.exec(line)!;
      const info = fence[2]!.trim();
      const [tag = "", ...metadata] = info.split(/\s+/);
      const filename = metadata.join(" ").replace(/^(?:title|filename)=["']?(.*?)["']?$/, "$1") || undefined;
      const language = tag.toLowerCase();
      index += 1;
      const start = index;
      while (index < lines.length && !closesFence(lines[index]!, fence[1]!)) index += 1;
      // The separator immediately before a closing fence is structural. All
      // bytes inside the source, including CRLF, tabs and blank lines, survive.
      const literal = index > start ? parts.slice(start * 2, (index - 1) * 2 + 1).join("") : "";
      if (index < lines.length) index += 1;
      const { frame, pre } = createCodeFrame(document, { source: literal, language: tag, filename });
      nodes.push(frame);
      if (["mermaid", "diff", "patch"].includes(language)) {
        void import("./rich-code").then(({ enhanceCode }) => enhanceCode(pre, language)).catch(() => { pre.dataset.richState = "fallback"; });
      }
      continue;
    }

    const heading = HEADING.exec(line);
    if (heading) {
      // Keep the scale modest while preserving the message's semantic outline.
      const level = Math.min(3, line.trimStart().match(/^#+/)![0].length);
      const lead = document.createElement(`h${level}`); lead.className = "markdown-heading";
      const bold = document.createElement("strong"); appendInline(document, bold, heading[1]!); lead.append(bold);
      nodes.push(lead); index += 1; continue;
    }

    const callout = allowCallouts ? CALLOUT.exec(line) : null;
    if (callout) {
      const kind = callout[1]!.toLowerCase();
      const label = kind[0]!.toUpperCase() + kind.slice(1);
      const aside = document.createElement("aside");
      aside.className = "markdown-callout"; aside.dataset.kind = kind;
      aside.setAttribute("aria-label", label);
      const title = document.createElement("p"); title.className = "markdown-callout-label";
      const strong = document.createElement("strong"); strong.textContent = label;
      title.append(strong); aside.append(title);
      index += 1;
      const body: string[] = [];
      while (index < lines.length && QUOTE_LINE.test(lines[index]!)) {
        body.push(lines[index]!.replace(QUOTE_LINE, "") + (parts[index * 2 + 1] ?? ""));
        index += 1;
      }
      // Reuse safe blocks; nested callout markers stay literal, with bounded depth.
      aside.append(...renderBlocks(document, body.join(""), false));
      nodes.push(aside); continue;
    }

    const marker = listMarker(line);
    if (marker) {
      const rendered = renderList(document, lines, index, marker.indent, marker.ordered, 0);
      nodes.push(rendered.node); index = rendered.next; continue;
    }

    const paragraphLines: string[] = [];
    while (index < lines.length && lines[index]!.trim() && !isBlockStart(lines[index]!)) paragraphLines.push(lines[index++]!);
    if (paragraphLines.length === 0) { paragraphLines.push(line); index += 1; }
    const paragraph = document.createElement("p"); appendInline(document, paragraph, paragraphLines.join("\n")); nodes.push(paragraph);
  }
  return nodes;
}

/** Strip supported formatting for compact conversation-list previews. */
export function plainTextMarkdown(source: string): string {
  const lines = normalizePlainText(source).replace(/\r\n?/g, "\n").split("\n");
  const output: string[] = [];
  let index = 0;
  while (index < lines.length) {
    const line = lines[index]!;
    if (isFence(line)) {
      const opening = FENCE.exec(line)![1]!;
      index += 1;
      while (index < lines.length && !closesFence(lines[index]!, opening)) output.push(lines[index++]!);
      if (index < lines.length) index += 1;
      continue;
    }
    const heading = HEADING.exec(line);
    if (heading) { output.push(inlinePlainText(heading[1]!)); index += 1; continue; }
    const marker = listMarker(line);
    if (marker) { output.push(inlinePlainText(marker.content)); index += 1; continue; }
    output.push(inlinePlainText(line)); index += 1;
  }
  return output.join(" ").replace(/\s+/g, " ").trim();
}
