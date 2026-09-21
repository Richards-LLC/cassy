// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { plainTextMarkdown, renderMarkdown } from "./markdown-renderer";

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
});
