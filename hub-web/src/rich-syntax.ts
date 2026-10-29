/** Deliberately lexical: no markup output, language guessing or external grammar. */
type Kind = "keyword" | "string" | "comment" | "literal";
interface Token { start: number; end: number; kind: Kind }
interface Cached { tokens: readonly Token[]; bytes: number }

export const SYNTAX_LIMITS = Object.freeze({ sourceUnits: 32 * 1024, spans: 2048, cacheBytes: 256 * 1024, cacheEntries: 32 });
const cache = new Map<string, Cached>();
let cacheBytes = 0;
const keywordSets = {
  script: "as async await break case catch class const continue debugger default delete do else enum export extends finally for from function if implements import in instanceof interface let new of private protected public readonly return static super switch this throw try type typeof var void while with yield",
  json: "",
  python: "and as assert async await break class continue def del elif else except finally for from global if import in is lambda nonlocal not or pass raise return try while with yield",
  rust: "as async await break const continue crate dyn else enum extern fn for if impl in let loop match mod move mut pub ref return self Self static struct super trait type unsafe use where while",
  shell: "if then else elif fi for while until do done case in esac function select local declare readonly export unset",
};
const keywords = Object.fromEntries(Object.entries(keywordSets).map(([mode, words]) => [mode, new Set(words.split(" "))])) as Record<keyof typeof keywordSets, Set<string>>;
const literals = new Set(["true", "false", "null", "undefined", "None", "True", "False", "NaN", "Infinity"]);

function family(language: string): "script" | "json" | "python" | "rust" | "shell" {
  if (/^(py|python)$/.test(language)) return "python";
  if (/^(rs|rust)$/.test(language)) return "rust";
  if (/^(sh|bash|shell)$/.test(language)) return "shell";
  return language === "json" ? "json" : "script";
}

/** Offset-only runs avoid keeping a second copy of every colored substring. */
function tokensFor(source: string, language: string): readonly Token[] | null {
  const mode = family(language.toLowerCase());
  const key = `${mode}\0${source}`;
  const hit = cache.get(key);
  if (hit) { cache.delete(key); cache.set(key, hit); return hit.tokens; }
  // Each alternative consumes a whole string/comment, including unterminated
  // ones. Anchored string bodies contain no nested quantifiers. No recursion.
  const strings = mode === "python"
    ? '"""[\\s\\S]*?(?:"""|$)|\'\'\'[\\s\\S]*?(?:\'\'\'|$)|"(?:[^"\\\\\\r\\n]|\\\\[\\s\\S])*(?:"|$)|\'(?:[^\'\\\\\\r\\n]|\\\\[\\s\\S])*(?:\'|$)'
    : '"(?:[^"\\\\]|\\\\[\\s\\S])*(?:"|$)|\'(?:[^\'\\\\]|\\\\[\\s\\S])*(?:\'|$)|`(?:[^`\\\\]|\\\\[\\s\\S])*(?:`|$)';
  const comments = mode === "json" ? "" : mode === "shell" || mode === "python" ? "#[^\\r\\n]*|" : "//[^\\r\\n]*|/\\*[\\s\\S]*?(?:\\*/|$)|";
  const pattern = new RegExp(`${comments}${strings}|\\b(?:0[xX][\\da-fA-F_]+|\\d[\\d_]*(?:\\.\\d[\\d_]*)?(?:[eE][+-]?\\d+)?)\\b|[A-Za-z_$][\\w$]*`, "g");
  const tokens: Token[] = [];
  for (const match of source.matchAll(pattern)) {
    const word = match[0];
    const kind: Kind | undefined = /^(?:#|\/\/|\/\*)/.test(word) ? "comment"
      : /^["'`]/.test(word) ? "string"
      : /^\d/.test(word) || literals.has(word) ? "literal"
      : mode !== "json" && keywords[mode].has(word) ? "keyword" : undefined;
    if (!kind) continue;
    if (tokens.length === SYNTAX_LIMITS.spans) return null;
    tokens.push({ start: match.index!, end: match.index! + word.length, kind });
  }
  // Conservative heap accounting: UTF-16 key + 96 bytes/run + Map/object
  // allowance, rather than presenting raw source bytes as actual heap use.
  const bytes = key.length * 2 + tokens.length * 96 + 128;
  while (cache.size && (cacheBytes + bytes > SYNTAX_LIMITS.cacheBytes || cache.size >= SYNTAX_LIMITS.cacheEntries)) {
    const oldest = cache.keys().next().value!;
    cacheBytes -= cache.get(oldest)!.bytes;
    cache.delete(oldest);
  }
  if (bytes <= SYNTAX_LIMITS.cacheBytes) { cache.set(key, { tokens, bytes }); cacheBytes += bytes; }
  return tokens;
}

interface Endpoint { node: Node; offset: number; sourceOffset?: number }
function endpoint(code: HTMLElement, node: Node | null, offset: number): Endpoint | undefined {
  if (!node) return undefined;
  if (!code.contains(node)) return { node, offset };
  const range = code.ownerDocument.createRange();
  range.selectNodeContents(code);
  range.setEnd(node, offset);
  return { node, offset, sourceOffset: range.toString().length };
}
function resolveEndpoint(code: HTMLElement, saved: Endpoint): Endpoint {
  if (saved.sourceOffset === undefined) return saved;
  const walker = code.ownerDocument.createTreeWalker(code, 4 /* SHOW_TEXT */);
  let remaining = saved.sourceOffset;
  let node: Node | null;
  while ((node = walker.nextNode())) {
    if (remaining <= node.textContent!.length) return { node, offset: remaining };
    remaining -= node.textContent!.length;
  }
  return { node: code, offset: code.childNodes.length };
}

/** Returns an honest fallback reason; throws before mutation if tokenization fails. */
export function highlightCode(pre: HTMLPreElement, language: string): string | undefined {
  const code = pre.querySelector("code");
  if (!code) return "colors unavailable";
  const source = code.textContent ?? "";
  if (source.length > SYNTAX_LIMITS.sourceUnits) return "source too large to color";
  const tokens = tokensFor(source, language);
  if (!tokens) return "too many tokens to color";
  const document = code.ownerDocument;
  const fragment = document.createDocumentFragment();
  let cursor = 0;
  for (const { start, end, kind } of tokens) {
    fragment.append(document.createTextNode(source.slice(cursor, start)));
    const span = document.createElement("span");
    span.className = `syntax-${kind}`;
    span.textContent = source.slice(start, end);
    fragment.append(span);
    cursor = end;
  }
  fragment.append(document.createTextNode(source.slice(cursor)));
  // Read the selection immediately before painting, after asynchronous loading.
  // Restore both endpoints, including direction and selections crossing a frame.
  const selection = document.getSelection();
  const anchor = endpoint(code, selection?.anchorNode ?? null, selection?.anchorOffset ?? 0);
  const focus = endpoint(code, selection?.focusNode ?? null, selection?.focusOffset ?? 0);
  const scroll = [pre.scrollLeft, pre.scrollTop];
  code.replaceChildren(fragment);
  if (selection && anchor && focus && (anchor.sourceOffset !== undefined || focus.sourceOffset !== undefined)) {
    const a = resolveEndpoint(code, anchor); const f = resolveEndpoint(code, focus);
    selection.setBaseAndExtent(a.node, a.offset, f.node, f.offset);
  }
  [pre.scrollLeft, pre.scrollTop] = scroll as [number, number];
  return undefined;
}
