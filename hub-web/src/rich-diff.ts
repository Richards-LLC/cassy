const HUNK = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/;

/** A hand-written change has no hunk to bound it: a --- / +++ pair is its file header, and any other marker is a change. */
function hunklessChange(lines: readonly string[], index: number): "header" | "add" | "remove" | "context" {
  const line = lines[index]!;
  if ((line.startsWith("--- ") && lines[index + 1]?.startsWith("+++ ")) || (line.startsWith("+++ ") && lines[index - 1]?.startsWith("--- "))) return "header";
  return line.startsWith("+") ? "add" : line.startsWith("-") ? "remove" : "context";
}

/** A small, semantic diff viewer; no syntax highlighter or HTML parser. */
export function unifiedDiff(document: Document, source: string): HTMLElement | undefined {
  const lines = source.split("\n");
  const hunked = lines.some((line) => HUNK.test(line));
  // Without a hunk or a changed line there is nothing to color: stay plain code.
  if (!hunked && !lines.some((_, index) => ["add", "remove"].includes(hunklessChange(lines, index)))) return undefined;
  const root = document.createElement("div"); root.className = "rich-diff";
  const pre = document.createElement("pre"); pre.setAttribute("aria-label", hunked ? "Unified diff" : "Diff");
  const populate = () => {
    if (pre.childNodes.length) return;
    let oldLine = 0, newLine = 0, oldRemaining = 0, newRemaining = 0;
    for (const [index, line] of lines.entries()) {
      const text = document.createElement("code"); text.textContent = `${line}\n`;
      if (!hunked) {
        // No positions were written, so no line-number gutter is drawn.
        const row = document.createElement("span"); row.className = "diff-line"; row.dataset.change = hunklessChange(lines, index);
        row.append(text); pre.append(row);
        continue;
      }
      const hunk = HUNK.exec(line);
      const inHunk = oldRemaining > 0 || newRemaining > 0;
      const header = !inHunk || line.startsWith("\\");
      if (hunk) {
        oldLine = Number(hunk[1]); newLine = Number(hunk[3]);
        oldRemaining = Number(hunk[2] ?? 1); newRemaining = Number(hunk[4] ?? 1);
      }
      const change = hunk ? "hunk" : header ? "header" : line.startsWith("+") ? "add" : line.startsWith("-") ? "remove" : "context";
      const row = document.createElement("span"); row.className = "diff-line"; row.dataset.change = change;
      const numbers = document.createElement("span"); numbers.className = "diff-numbers"; numbers.setAttribute("aria-hidden", "true");
      if (!hunk && !header) {
        numbers.textContent = `${change === "add" ? "" : oldLine++} ${change === "remove" ? "" : newLine++}`;
        if (change !== "add") oldRemaining = Math.max(0, oldRemaining - 1);
        if (change !== "remove") newRemaining = Math.max(0, newRemaining - 1);
      }
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
