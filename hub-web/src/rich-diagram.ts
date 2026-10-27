import DOMPurify from "dompurify";
import { renderAsync } from "pele/lazy";

let nextDiagram = 0;

/** No HTML string is inserted into the live document: only sanitized DOM nodes. */
export async function diagram(document: Document, source: string): Promise<HTMLElement> {
  const result = await renderAsync(source, { links: false, images: false, idPrefix: `commander-diagram-${++nextDiagram}-`, limit: 20_000, maxEdges: 200, outputLimit: 1_000_000 });
  const fragment = DOMPurify.sanitize(result.svg, {
    USE_PROFILES: { svg: true, svgFilters: true },
    RETURN_DOM_FRAGMENT: true,
    FORBID_TAGS: ["script", "foreignObject", "style", "a", "image", "use", "animate", "animateTransform", "set"],
  });
  const svg = fragment.querySelector("svg");
  if (!svg) throw new Error("Diagram has no SVG");
  // DOMPurify handles executable attributes; also disallow external resources
  // and CSS URLs. The renderer's theme uses local CSS variables only.
  for (const node of fragment.querySelectorAll("*")) {
    for (const attr of [...node.attributes]) {
      if (/href$/i.test(attr.name) && !attr.value.startsWith("#") || /url\(\s*['"]?(?!#)/i.test(attr.value) || /@import/i.test(attr.value)) node.removeAttribute(attr.name);
    }
  }
  const figure = document.createElement("figure"); figure.className = "rich-diagram";
  svg.setAttribute("role", "img"); svg.setAttribute("aria-label", "Mermaid diagram");
  figure.append(document.importNode(svg, true));
  const sourceDetails = document.createElement("details");
  const summary = document.createElement("summary"); summary.textContent = "Diagram source";
  const code = document.createElement("pre"); code.className = "markdown-code"; code.textContent = source;
  sourceDetails.append(summary, code); figure.append(sourceDetails);
  return figure;
}
