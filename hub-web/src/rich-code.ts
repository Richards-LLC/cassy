import "./rich-content.css";

/** Progressive enhancement: on failure the original, selectable code survives. */
export async function enhanceCode(pre: HTMLPreElement, language: string): Promise<void> {
  try {
    if (language === "mermaid") {
      const { diagram } = await import("./rich-diagram");
      const figure = await diagram(pre.ownerDocument, pre.textContent ?? "");
      pre.replaceWith(figure);
    } else {
      const { unifiedDiff } = await import("./rich-diff");
      pre.replaceWith(unifiedDiff(pre.ownerDocument, pre.textContent ?? ""));
    }
  } catch {
    pre.dataset.richState = "fallback";
    pre.setAttribute("aria-label", `${language === "mermaid" ? "Diagram" : "Diff"} source — preview unavailable`);
  }
}
