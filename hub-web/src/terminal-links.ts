export interface TerminalLinkMatch {
  kind: "url" | "path";
  text: string;
  start: number;
  end: number;
}

export interface TerminalBufferLineLike {
  readonly isWrapped?: boolean;
  translateToString(trimRight?: boolean): string;
}

export interface WrappedTerminalLinkLineSegment {
  bufferLineNumber: number;
  text: string;
  startIndex: number;
  endIndex: number;
}

export interface WrappedTerminalLinkLine {
  text: string;
  segments: ReadonlyArray<WrappedTerminalLinkLineSegment>;
}

const URL_PATTERN = /https?:\/\/[^\s"'`<>]+/g;
const PATH_PATTERN = /(?:~\/|\.{1,2}\/|\/)[^\s"'`<>]+/g;

export function extractTerminalLinks(line: string): TerminalLinkMatch[] {
  const matches: TerminalLinkMatch[] = [];
  for (const [kind, pattern] of [["url", URL_PATTERN], ["path", PATH_PATTERN]] as const) {
    pattern.lastIndex = 0;
    for (const match of line.matchAll(pattern)) {
      const start = match.index ?? 0;
      const text = match[0].replace(/[.,;!?]+$/, "");
      if (!matches.some((existing) => start < existing.end && existing.start < start + text.length)) {
        matches.push({ kind, text, start, end: start + text.length });
      }
    }
  }
  return matches.toSorted((a, b) => a.start - b.start);
}

export function collectWrappedTerminalLinkLine(
  bufferLineNumber: number,
  getLine: (index: number) => TerminalBufferLineLike | null | undefined,
): WrappedTerminalLinkLine | null {
  const anchor = getLine(bufferLineNumber - 1);
  if (!anchor) return null;
  let start = bufferLineNumber;
  while (start > 1 && getLine(start - 1)?.isWrapped) start -= 1;
  const segments: WrappedTerminalLinkLineSegment[] = [];
  let offset = 0;
  for (let line = start; ; line += 1) {
    const current = getLine(line - 1);
    if (!current) break;
    const continues = getLine(line)?.isWrapped === true;
    const text = current.translateToString(!continues);
    segments.push({ bufferLineNumber: line, text, startIndex: offset, endIndex: offset + text.length });
    offset += text.length;
    if (!continues) break;
  }
  return { text: segments.map((segment) => segment.text).join(""), segments };
}
