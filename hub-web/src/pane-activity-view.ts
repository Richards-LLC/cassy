import type { PaneActivityLabel } from "./pane-activity";

// A hidden caption retains its label without retaining invisible reading text.
// Weak keys do not keep cards alive after a shell replacement.
const labels = new WeakMap<HTMLElement, PaneActivityLabel>();

export function updatePaneActivityCaption(element: HTMLElement, label: PaneActivityLabel): void {
  const previous = labels.get(element);
  labels.set(element, label);
  element.title = label.title;
  // Output can arrive many times per second while the displayed label stays
  // "now". Placement/resize/font refresh owns geometry-only changes.
  if (previous?.text !== label.text || !element.isConnected) fitPaneActivityCaption(element);
}

/** Show the complete caption or remove its reading/AX content, never a stub. */
export function fitPaneActivityCaption(element: HTMLElement): void {
  const label = labels.get(element);
  if (!label) return;
  // Restore before measuring: the reserved flex slot does not depend on the
  // text, so a wider header or shorter live-output label can recover it.
  element.textContent = label.text;
  let fits = false;
  if (element.isConnected && element.getClientRects().length > 0) {
    const box = element.getBoundingClientRect();
    const range = element.ownerDocument.createRange();
    range.selectNodeContents(element);
    const words = range.getBoundingClientRect();
    // Match the header's existing whole-text geometry contract (cas-2072).
    const tolerance = 0.02;
    fits = box.width > 0 && box.height > 0
      && words.left >= box.left - tolerance && words.right <= box.right + tolerance
      && words.top >= box.top - tolerance && words.bottom <= box.bottom + tolerance;
  }
  if (fits) element.removeAttribute("aria-hidden");
  else {
    element.textContent = "";
    element.setAttribute("aria-hidden", "true");
  }
}

export function fitPaneActivityCaptions(root: ParentNode): void {
  for (const element of root.querySelectorAll<HTMLElement>(".pane-last-activity")) fitPaneActivityCaption(element);
}
