/**
 * The phone Attention sheet's modal behaviour (cas-a5c6). On a phone the
 * conversation's context rail is laid over the thread as a sheet that shows
 * only Attention (cas-5c22). While it is modal:
 *
 * - it carries dialog semantics, and the rest of the shell is inert, so
 *   neither Tab nor a screen reader can reach what it covers;
 * - Tab and Shift+Tab cycle within it, and focus that lands outside it (a
 *   click, a script) is brought back;
 * - Escape closes it from anywhere on the page.
 *
 * When it is not modal (closed, or the viewport has become a desktop where the
 * rail is a side panel again), none of that applies: no role, no aria-modal,
 * nothing inert.
 */

import { attentionControlKey } from "./attention-view";

const LABEL_OPEN = "Attention for this session";
const LABEL_RAIL = "Conversation context";
const FOCUSABLE = 'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), summary, [tabindex]';

/** Give or take the sheet's modal semantics; `modal` false restores the plain rail. */
export function applySheetSemantics(shell: HTMLElement | null, modal: boolean, section: "attention" | "progress" = "attention"): void {
  if (!shell) return;
  shell.classList.toggle("attention-sheet-open", modal);
  shell.classList.toggle("fleet-sheet-open", modal && section === "progress");
  const rail = shell.querySelector<HTMLElement>(":scope > .conversation-context");
  for (const child of shell.children) {
    if (child === rail) continue;
    if (modal) child.setAttribute("inert", ""); else child.removeAttribute("inert");
  }
  if (!rail) return;
  if (modal) {
    rail.setAttribute("role", "dialog");
    rail.setAttribute("aria-modal", "true");
    rail.setAttribute("aria-label", section === "progress" ? "Tasks & progress" : LABEL_OPEN);
    // cas-d043 G01: whose tasks, as its description, so its name stays "Tasks & progress".
    const where = rail.querySelector<HTMLElement>(".context-sheet-where");
    if (section === "progress" && where?.id) rail.setAttribute("aria-describedby", where.id); else rail.removeAttribute("aria-describedby");
  } else {
    rail.removeAttribute("role");
    rail.removeAttribute("aria-modal");
    rail.removeAttribute("aria-describedby");
    rail.setAttribute("aria-label", LABEL_RAIL);
  }
}

/** Rendered, so focusable: the sheet hides its other sections with CSS. */
const rendered = (node: HTMLElement): boolean => node.getClientRects().length > 0;

/**
 * Inside a closed <details> only its own <summary> can take focus; the rest
 * is not rendered, and focus() on it does nothing (cas-a5c6 QA F01: a Copy
 * button inside a collapsed Details was counted as the sheet's last stop, so
 * Shift+Tab from Close and Tab past Details went nowhere).
 */
function insideClosedDetails(node: HTMLElement): boolean {
  for (let details = node.closest("details"); details; details = details.parentElement?.closest("details") ?? null) {
    if (details.open) continue;
    const ownSummary = node.tagName === "SUMMARY" && node.parentElement === details;
    if (!ownSummary) return true;
  }
  return false;
}

/** The sheet's focusable controls, in order, skipping hidden, disabled and collapsed ones. */
export function sheetFocusables(sheet: HTMLElement, visible: (node: HTMLElement) => boolean = rendered): HTMLElement[] {
  return [...sheet.querySelectorAll<HTMLElement>(FOCUSABLE)].filter((node) =>
    node.tabIndex >= 0
    && !node.closest("[hidden], [inert], [aria-hidden='true']")
    && !(node as HTMLButtonElement).disabled
    && node.getAttribute("aria-disabled") !== "true"
    && !insideClosedDetails(node)
    && visible(node));
}

/**
 * A layer over the sheet (cas-a5c6 QA F02): an open modal <dialog> or another
 * aria-modal surface outside it, such as the Ctrl+K command palette. That
 * layer owns Escape, Tab and focus until it closes.
 */
export function layerAboveSheet(sheet: HTMLElement): boolean {
  const document = sheet.ownerDocument;
  return [...document.querySelectorAll<HTMLElement>("dialog[open], [aria-modal='true']")].some((layer) => layer !== sheet && (layer.tagName === "DIALOG" || (!sheet.contains(layer) && !layer.contains(sheet))));
}

/**
 * Handle one keydown while the sheet is modal. Returns true when the key was
 * the sheet's to handle (the caller then prevents the default).
 */
export function sheetKeydown(event: Pick<KeyboardEvent, "key" | "shiftKey">, sheet: HTMLElement, active: Element | null, close: () => void, visible?: (node: HTMLElement) => boolean): boolean {
  // The topmost layer wins: a palette opened over the sheet closes first.
  if (layerAboveSheet(sheet)) return false;
  if (event.key === "Escape") { close(); return true; }
  if (event.key !== "Tab") return false;
  const controls = sheetFocusables(sheet, visible);
  if (!controls.length) return true;
  // The sheet moves focus itself, in document order with wrap-around: left to
  // the browser, a Tab from its last stop walked out to the page (a control
  // the browser skips but a selector counts made "last" ambiguous).
  const index = controls.indexOf(active as HTMLElement);
  const next = index < 0
    ? (event.shiftKey ? controls.at(-1)! : controls[0]!)
    : controls[(index + (event.shiftKey ? -1 : 1) + controls.length) % controls.length]!;
  next.focus();
  return true;
}

/**
 * A sheet control's identity that survives a redraw (cas-a5c6 QA rounds 2
 * and 3): the Attention panel's own key (the notice or group it belongs to,
 * and its role) for panel controls, and the sheet's own controls by kind.
 * Never a position among look-alikes: when a new notice arrived above, an
 * index moved focus to the new notice's Dismiss.
 */
export function focusKey(_sheet: HTMLElement, node: HTMLElement): string {
  return attentionControlKey(node) ?? `sheet|${node.tagName}.${node.classList[0] ?? ""}`;
}

/** The control in the (redrawn) sheet with this key, if it is still a stop; otherwise nothing. */
export function findByFocusKey(sheet: HTMLElement, key: string, visible?: (node: HTMLElement) => boolean): HTMLElement | undefined {
  return sheetFocusables(sheet, visible).find((candidate) => focusKey(sheet, candidate) === key);
}
