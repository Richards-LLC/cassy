// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { plainTextMarkdown, renderMarkdown } from "./markdown-renderer";

vi.mock("pele/lazy", async (original) => ({ ...await original<typeof import("pele/lazy")>() }));

describe("supervisor markdown subset", () => {
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
    ].join("\n")));

    expect(root.querySelector(".markdown-heading strong")?.textContent).toBe("Ship result");
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
  root.append(...renderMarkdown(document, `\`\`\`diff\n${patch}\n\`\`\``));
  await vi.waitFor(() => expect(root.querySelector("details")).not.toBeNull());
  expect(root.querySelector("details")?.open).toBe(false);
  expect(root.querySelector("summary")?.textContent).toContain("lines");
  root.querySelector("summary")!.click();
  expect(root.querySelector("details")?.open).toBe(true);
  await vi.waitFor(() => expect(root.querySelector('[data-change="remove"]')?.textContent).toContain("<script>bad()</script>"));
  expect(root.querySelector('[data-change="add"]')?.textContent).toContain("safe()");
  expect([...root.querySelectorAll('[data-change="remove"]')].some((row) => row.textContent?.includes("--- SQL comment"))).toBe(true);
  expect([...root.querySelectorAll('[data-change="add"]')].some((row) => row.textContent?.includes("+++ counter"))).toBe(true);
  expect(root.querySelector("script")).toBeNull(); root.remove();
});
