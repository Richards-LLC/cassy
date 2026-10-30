// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { plainTextMarkdown, renderMarkdown } from "./markdown-renderer";

vi.mock("pele/lazy", async (original) => ({ ...await original<typeof import("pele/lazy")>() }));

describe("supervisor markdown subset", () => {
  it("keeps complete emoji sequences colored without changing text, code or safe literal markup", () => {
    const prose = '<script> & ☺️ 👍🏽 👩🏽‍💻 🇯🇵 1️⃣ 🏴\u{E0067}\u{E0062}\u{E0065}\u{E006E}\u{E0067}\u{E007F}';
    const root = document.createElement("div");
    root.append(...renderMarkdown(document, `${prose}\n\n\`👍🏽\``));
    expect(root.querySelector("p")?.textContent).toBe(prose);
    expect([...root.querySelectorAll(".emoji-text")].map(node => node.textContent)).toEqual(["☺️", "👍🏽", "👩🏽‍💻", "🇯🇵", "1️⃣", "🏴\u{E0067}\u{E0062}\u{E0065}\u{E006E}\u{E0067}\u{E007F}"]);
    expect(root.querySelector("script")).toBeNull();
    expect(root.querySelector("code")?.textContent).toBe("👍🏽");
    expect(root.querySelector("code .emoji-text")).toBeNull();
  });
  it.each([
    {
      language: "bash",
      source: "awk '{\n  print $1\n}' file.txt\njq '.items[]\n  | .name' data.json",
      strings: ["'{\n  print $1\n}'", "'.items[]\n  | .name'"],
      keywords: [],
    },
    {
      language: "rust",
      source: String.raw`fn borrow<'a>(value: &'a str) -> &'a str {
  let chars = ['a', '\n', '\'', '\x41', '\u{1F980}', '🦀'];
  let forever: &'static str = "ready";
  return value;
}`,
      strings: ["'a'", String.raw`'\n'`, String.raw`'\''`, String.raw`'\x41'`, String.raw`'\u{1F980}'`, "'🦀'", '"ready"'],
      keywords: ["fn", "let", "let", "return"],
    },
    {
      language: "jsx",
      source: "const label = <p>don't stop</p>;\r\nconst other = 'ready';\nreturn true;",
      strings: ["'ready'"],
      keywords: ["const", "const", "return"],
    },
    {
      language: "ts",
      source: "const broken = 'unfinished\\\r\nconst good = 'ready';\nreturn true;",
      strings: ["'unfinished\\", "'ready'"],
      keywords: ["const", "const", "return"],
    },
  ])("keeps $language apostrophes from swallowing following code", async ({ language, source, strings, keywords }) => {
    const root = document.createElement("div");
    root.append(...renderMarkdown(document, `\`\`\`${language}\n${source}\n\`\`\``));
    await vi.waitFor(() => expect(root.querySelector('[data-syntax-state="ready"]')).not.toBeNull());
    const code = root.querySelector("code")!;
    expect(code.textContent).toBe(source);
    expect([...code.querySelectorAll(".syntax-string")].map((node) => node.textContent)).toEqual(strings);
    expect([...code.querySelectorAll(".syntax-keyword")].map((node) => node.textContent)).toEqual(keywords);
  });

  it("keeps cached source when another colored block is too large to cache", async () => {
    const { highlightCode, SYNTAX_LIMITS } = await import("./rich-syntax");
    const paint = (source: string) => {
      const pre = document.createElement("pre");
      const code = document.createElement("code");
      code.textContent = source;
      pre.append(code);
      expect(highlightCode(pre, "ts")).toBeUndefined();
      expect(code.textContent).toBe(source);
    };
    const cached = "const retainedCacheEntry = true;";
    paint(cached);
    // Fits the source/span budgets, but its source plus runs exceed cache bytes.
    paint("let ".repeat(SYNTAX_LIMITS.spans).padEnd(SYNTAX_LIMITS.sourceUnits, " "));
    const tokenize = vi.spyOn(String.prototype, "matchAll");
    try {
      paint(cached);
      expect(tokenize).not.toHaveBeenCalled();
    } finally { tokenize.mockRestore(); }
  });

  it("colors safe source lazily while preserving selection and oversized plain text", async () => {
    const source = 'const message = "<script>&雪"; // keep source\r\nreturn true;';
    const root = document.createElement("div");
    root.append(...renderMarkdown(document, `\`\`\`ts example.ts\n${source}\n\`\`\``));
    document.body.append(root);
    const code = root.querySelector("code")!;
    expect(code.textContent).toBe(source);
    expect(code.querySelector("span")).toBeNull();
    const selection = document.getSelection()!;
    selection.setBaseAndExtent(code.firstChild!, 35, code.firstChild!, 6);
    const selected = selection.toString();
    await vi.waitFor(() => expect(root.querySelector('[data-syntax-state="ready"]')).not.toBeNull());
    expect(code.querySelector('.syntax-keyword')?.textContent).toBe("const");
    expect(code.querySelector('.syntax-string')?.textContent).toBe('"<script>&雪"');
    expect(code.querySelector('.syntax-comment')?.textContent).toBe("// keep source");
    expect(code.textContent).toBe(source);
    expect(root.querySelector("script")).toBeNull();
    expect(selection.toString()).toBe(selected);
    const offsets = [selection.anchorNode!, selection.focusNode!].map((node, index) => {
      const range = document.createRange(); range.selectNodeContents(code);
      range.setEnd(node, index === 0 ? selection.anchorOffset : selection.focusOffset);
      return range.toString().length;
    });
    expect(offsets).toEqual([35, 6]);
    root.querySelector<HTMLButtonElement>('[data-action="wrap"]')!.click();
    expect(selection.toString()).toBe(selected);
    selection.removeAllRanges();
    root.remove();
    const large = "const a = 1;\n".repeat(4000);
    root.replaceChildren(...renderMarkdown(document, `\`\`\`js\n${large}\n\`\`\``));
    await vi.waitFor(() => expect(root.querySelector('[data-syntax-state="plain"]')).not.toBeNull());
    expect(root.querySelector("code")!.textContent).toBe(large);
    expect(root.textContent).toContain("Plain text · source too large to color");
  });

  it("copies literal fence source with honest feedback and wraps each block independently", async () => {
    const source = '\tconst literal = "<script>&amp;雪";  \r\n\r\n  done();';
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    const root = document.createElement("div");
    root.append(...renderMarkdown(document, `\`\`\`ts src/example.ts\r\n${source}\r\n\`\`\`\r\n\r\n~~~unknown\nuntouched\n~~~`));
    const frames = root.querySelectorAll<HTMLElement>(".code-frame");
    expect(frames).toHaveLength(2);
    expect(frames[0]!.querySelector(".code-frame-label")?.textContent).toBe("src/example.ts");
    expect(frames[1]!.querySelector(".code-frame-label")?.textContent).toBe("unknown");
    expect(root.querySelector("script")).toBeNull();
    const copy = frames[0]!.querySelector<HTMLButtonElement>('[data-action="copy"]')!;
    copy.click();
    await vi.waitFor(() => expect(frames[0]!.querySelector('[role="status"]')?.textContent).toBe("Copied"));
    expect(writeText).toHaveBeenCalledWith(source);
    writeText.mockRejectedValueOnce(new Error("Denied"));
    copy.click();
    await vi.waitFor(() => expect(frames[0]!.querySelector('[role="status"]')?.textContent).toContain("Copy failed"));
    expect(copy.disabled).toBe(false);
    const wrap = frames[0]!.querySelector<HTMLButtonElement>('[data-action="wrap"]')!;
    expect(wrap.getAttribute("aria-pressed")).toBe("false");
    wrap.click();
    expect(wrap.getAttribute("aria-pressed")).toBe("true");
    expect(frames[0]!.dataset.wrap).toBe("true");
    expect(frames[1]!.dataset.wrap).toBe("false");
    wrap.click();
    expect(wrap.getAttribute("aria-pressed")).toBe("false");
    Reflect.deleteProperty(navigator, "clipboard");
    frames[1]!.querySelector<HTMLButtonElement>('[data-action="copy"]')!.click();
    expect(frames[1]!.querySelector('[role="status"]')?.textContent).toContain("Copy failed");
  });

  it("renders reply structure without leaking markdown markers", () => {
    const root = document.createElement("div");
    root.append(...renderMarkdown(document, [
      "# Ship result",
      "",
      "**Ready** with *care*, `cargo check` and [the receipt](https://example.com/receipt).",
      "",
      "- **one**",
      "- two",
      "  - nested",
      "",
      "1. first",
      "2. second",
      "",
      "```sh",
      "cargo check -p cas",
      "```",
      "",
      "> [!NOTE]\r\n> ```sh\r\n> \tkeep();  \r\n> \r\n> end();\r\n> ```",
    ].join("\n")));

    expect(root.querySelector("h1.markdown-heading strong")?.textContent).toBe("Ship result");
    expect(root.querySelector("p:not(.markdown-heading) strong")?.textContent).toBe("Ready");
    expect(root.querySelector("p:not(.markdown-heading) em")?.textContent).toBe("care");
    expect(root.querySelector(".markdown-inline-code")?.textContent).toBe("cargo check");
    expect(root.querySelector("a")?.textContent).toBe("the receipt");
    expect(root.querySelector("a")?.target).toBe("_blank");
    expect(root.querySelector("a")?.rel).toBe("noopener");
    expect(root.querySelectorAll(".markdown-list")).toHaveLength(3);
    expect(root.querySelectorAll(".markdown-list-nested")).toHaveLength(1);
    expect(root.querySelector("ol")?.textContent).toContain("first");
    expect(root.querySelector(".markdown-code code")?.textContent).toBe("cargo check -p cas");
    expect(root.querySelector(".code-frame-label")?.textContent).toBe("sh");
    expect(root.querySelector(".markdown-callout code")?.textContent).toBe("\tkeep();  \r\n\r\nend();");
    expect(root.textContent).not.toContain("**");
    expect(root.textContent).not.toContain("```sh");
  });

  it("keeps tags, entities, and unsafe links literal", () => {
    const root = document.createElement("div");
    root.append(...renderMarkdown(document, "<script>alert(1)</script> &lt;b&gt; [run](javascript:alert(1)) [safe](https://example.com)"));

    expect(root.querySelector("script")).toBeNull();
    expect(root.textContent).toContain("<script>alert(1)</script>");
    expect(root.textContent).toContain("&lt;b&gt;");
    expect(root.textContent).toContain("[run](javascript:alert(1))");
    expect(root.querySelectorAll("a")).toHaveLength(1);
    expect(root.querySelector("a")?.textContent).toBe("safe");
  });

  it("strips supported markers from conversation previews", () => {
    expect(plainTextMarkdown("# **Ready**\n\n- one\n- `cargo check`\n\n[receipt](https://example.com)"))
      .toBe("Ready one cargo check receipt");
    expect(plainTextMarkdown("[unsafe](javascript:alert(1))")).toBe("[unsafe](javascript:alert(1))");
  });

  it("turns an older status enumeration into a bold lead and an ordered list", () => {
    const root = document.createElement("div");
    root.append(...renderMarkdown(document, "Status 13:28Z. WAITING ON YOU: (1) merge the branch — it is ready (2) close the task after the receipt"));

    expect(root.querySelector(".markdown-heading strong")?.textContent).toBe("Status 13:28Z. WAITING ON YOU:");
    expect([...root.querySelectorAll("ol > li")].map((item) => item.textContent)).toEqual([
      "merge the branch — it is ready",
      "close the task after the receipt",
    ]);
  });

  it("renders the verbatim status row with a malformed minute as a bold lead and three items", () => {
    const source = "Status 13:2xZ. WAITING ON YOU: (1) mockup shape — final 5 posts / 10 issues at ~/.cas/artifacts/cas-7771/slack-by-flow/index.html, Blocked paths first, every item proven by real login / measurement / real click; (2) \"post\"; (3) \"run it\" — the real 80-cell one-command run (~$53, <2 h), and whether Luna or Opus explores. LUNA vs OPUS (20 same cells, same verify+vet): Opus 66 raw → 35 vetted, 27 false positives, $12.42; Luna 19 raw → 14 vetted, 6 false positives, cost receipt missing from Codex CLI; both recovered 4 of 9 known defects — a floor, the comparison's verifier ran 1 attempt on an older build and dropped 2 items that reproduce 3/3; being re-verified. flow-opus.png / flow-luna.png in ~/.cas/artifacts/cas-db69/. MERGED TODAY (tests green, honest typecheck): action-replay verifier + real form login; code-vet step; cross-route dedupe; one cell per route + 120 s + 429 cap; capture fix; one-command qa-run with scorecard. Retrospective: docs/qa/2026-09-21-user-eye-pilot-retrospective.md. IN FLIGHT: recall re-check, dev-pages cleanup PR. Nothing posted.";
    const root = document.createElement("div");
    root.append(...renderMarkdown(document, source));

    expect(root.querySelector(".markdown-heading strong")?.textContent).toBe("Status 13:2xZ. WAITING ON YOU:");
    expect([...root.querySelectorAll("ol > li")].map((item) => item.textContent)).toEqual([
      expect.stringContaining("mockup shape"),
      '"post";',
      expect.stringContaining('"run it"'),
    ]);
    expect(root.querySelectorAll("ol > li")).toHaveLength(3);
  });

  it("recognizes a short sentence followed by an all-caps lead label", () => {
    const root = document.createElement("div");
    root.append(...renderMarkdown(document, "The release gate is green. WAITING ON YOU: (1) review the receipt (2) close the task"));

    expect(root.querySelector(".markdown-heading strong")?.textContent).toBe("The release gate is green. WAITING ON YOU:");
    expect(root.querySelectorAll("ol > li")).toHaveLength(2);
  });

  it("keeps the plain prefix when an enumeration has no recognizable lead", () => {
    const root = document.createElement("div");
    root.append(...renderMarkdown(document, "A note with (1) a parenthetical and (2) another colon: stays structured."));

    expect(root.querySelector(".markdown-heading")).toBeNull();
    expect(root.querySelector("p")?.textContent).toBe("A note with");
    expect([...root.querySelectorAll("ol > li")].map((item) => item.textContent)).toEqual(["a parenthetical and", "another colon: stays structured."]);
  });

  it("keeps Markdown bodies untouched", () => {
    const markdown = document.createElement("div");
    markdown.append(...renderMarkdown(document, "**Already formatted**\n\n(1) leave this literal (2) too"));
    expect(markdown.querySelector("strong")?.textContent).toBe("Already formatted");
    expect(markdown.querySelector("ol")).toBeNull();
    expect(markdown.textContent).toContain("(1) leave this literal (2) too");
  });
});

// The real Markdown entry point must keep readable source until the lazy renderer succeeds.
it("renders Mermaid through sanitized SVG and keeps invalid diagrams as source", async () => {
  const root = document.createElement("div"); document.body.append(root);
  // jsdom has no canvas font metrics; the browser journey proves real layout.
  const canvas = vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
  root.append(...renderMarkdown(document, "```mermaid\nflowchart LR\nA[Ready] --> B[Ship]\n```\n\n```mermaid\nthis is invalid\n```"));
  await vi.waitFor(() => expect(root.querySelector("svg" )).not.toBeNull());
  expect(root.querySelector("svg")?.textContent).toContain("Ready");
  await vi.waitFor(() => expect(root.querySelector('[data-rich-state="fallback"]')?.textContent).toContain("this is invalid"));
  canvas.mockRestore(); root.remove();
});

it("never inserts a renderer's scripts, event handlers or foreignObject", async () => {
  const pele = await import("pele/lazy");
  const render = vi.spyOn(pele, "renderAsync").mockResolvedValue({ svg: '<svg xmlns="http://www.w3.org/2000/svg" style="font-size:12px;fill:var(--_fg)" onload="window.compromised=1"><script>window.compromised=1</script><foreignObject><div xmlns="http://www.w3.org/1999/xhtml"><img src="x" onerror="window.compromised=1" /></div></foreignObject><text onclick="window.compromised=1">Safe label</text></svg>', width: 200, height: 100, links: [], type: "flowchart", mermaidType: "flowchart-v2" });
  const root = document.createElement("div"); document.body.append(root);
  try {
    root.append(...renderMarkdown(document, "```mermaid\nflowchart LR\nA --> B\n```"));
    await vi.waitFor(() => expect(root.querySelector("svg")?.textContent).toBe("Safe label"));
    expect(root.querySelector("script, foreignObject, [onload], [onclick], [onerror], [style]")).toBeNull();
    expect(root.querySelector("svg")?.getAttribute("font-size")).toBe("12px");
    root.querySelector("svg")!.dispatchEvent(new Event("load"));
    root.querySelector("text")!.dispatchEvent(new MouseEvent("click"));
    expect((window as unknown as { compromised?: number }).compromised).toBeUndefined();
  } finally { render.mockRestore(); root.remove(); }
});

it("renders unified diff lines literally and collapses long changes", async () => {
  const root = document.createElement("div"); document.body.append(root);
  const patch = ['--- a/app.ts', '+++ b/app.ts', '@@ -1,26 +1,26 @@', '-<script>bad()</script>', '+safe()', '--- SQL comment', '+++ counter', ...Array.from({ length: 24 }, (_, i) => ` context ${i}`)].join("\n");
  const snippet = "-old()\n+new()";
  root.append(...renderMarkdown(document, `\`\`\`diff\n${patch}\n\`\`\`\n\n\`\`\`diff\n${snippet}\n\`\`\``));
  await vi.waitFor(() => expect(root.querySelector("details")).not.toBeNull());
  expect(root.querySelector("details")?.open).toBe(false);
  expect(root.querySelector("summary")?.textContent).toContain("lines");
  root.querySelector("summary")!.click();
  expect(root.querySelector("details")?.open).toBe(true);
  await vi.waitFor(() => expect(root.querySelector('[data-change="remove"]')?.textContent).toContain("<script>bad()</script>"));
  expect(root.querySelector('[data-change="add"]')?.textContent).toContain("safe()");
  expect([...root.querySelectorAll('[data-change="remove"]')].some((row) => row.textContent?.includes("--- SQL comment"))).toBe(true);
  expect([...root.querySelectorAll('[data-change="add"]')].some((row) => row.textContent?.includes("+++ counter"))).toBe(true);
  await vi.dynamicImportSettled();
  const code = [...root.querySelectorAll("pre.markdown-code")].find((node) => node.textContent === snippet)!;
  expect(code).toBeDefined(); expect(code.hasAttribute("aria-label")).toBe(false);
  expect(root.querySelectorAll(".rich-diff")).toHaveLength(1);
  expect(root.querySelector("script")).toBeNull(); root.remove();
});
