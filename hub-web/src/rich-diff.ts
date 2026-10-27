/** A small, semantic unified-diff viewer; no syntax highlighter or HTML parser. */
export function unifiedDiff(document: Document, source: string): HTMLElement {
  const lines = source.split("\n");
  if (!lines.some((line) => /^@@ -\d+(?:,\d+)? \+\d+(?:,\d+)? @@/.test(line))) throw new Error("Not a unified diff");
  const root = document.createElement("div"); root.className = "rich-diff";
  const pre = document.createElement("pre"); pre.setAttribute("aria-label", "Unified diff");
  const populate = () => {
    if (pre.childNodes.length) return;
    let oldLine = 0, newLine = 0;
    for (const line of lines) {
      const hunk = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(line);
      const header = /^(?:diff |index |--- |\+\+\+ |\\)/.test(line);
      if (hunk) { oldLine = Number(hunk[1]); newLine = Number(hunk[2]); }
      const change = hunk ? "hunk" : header ? "header" : line.startsWith("+") ? "add" : line.startsWith("-") ? "remove" : "context";
      const row = document.createElement("span"); row.className = "diff-line"; row.dataset.change = change;
      const numbers = document.createElement("span"); numbers.className = "diff-numbers"; numbers.setAttribute("aria-hidden", "true");
      if (!hunk && !header) {
        numbers.textContent = `${change === "add" ? "" : oldLine++} ${change === "remove" ? "" : newLine++}`;
      }
      const text = document.createElement("code"); text.textContent = `${line}\n`;
      row.append(numbers, text); pre.append(row);
    }
  };
  // The summary tells the reader the exact expansion cost before opening it.
  if (lines.length > 24) {
    const details = document.createElement("details");
    const summary = document.createElement("summary"); summary.textContent = `Show diff · ${lines.length} lines`;
    details.addEventListener("toggle", () => { if (details.open) populate(); });
    details.append(summary, pre); root.append(details);
  } else { populate(); root.append(pre); }
  return root;
}
