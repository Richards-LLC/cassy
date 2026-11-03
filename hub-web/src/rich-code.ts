import "./rich-content.css";
import { setCodeFramePreview } from "./code-frame";

/** Progressive enhancement: on failure the original, selectable code survives. */
export async function enhanceCode(pre: HTMLPreElement, language: string): Promise<void> {
  try {
    if (language === "mermaid") {
      const { diagram } = await import("./rich-diagram");
      const figure = await diagram(pre.ownerDocument, pre.textContent ?? "");
      setCodeFramePreview(pre, figure);
    } else {
      const { unifiedDiff } = await import("./rich-diff");
      const diff = unifiedDiff(pre.ownerDocument, pre.textContent ?? "");
      if (diff) setCodeFramePreview(pre, diff);
    }
  } catch {
    pre.dataset.richState = "fallback";
    pre.setAttribute("aria-label", `${language === "mermaid" ? "Diagram" : "Diff"} source — preview unavailable`);
  }
}
