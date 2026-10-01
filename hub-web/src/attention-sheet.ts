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

const LABEL_OPEN = "Attention for this session";
const LABEL_RAIL = "Conversation context";
const FOCUSABLE = 'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), summary, [tabindex]';

/** Give or take the sheet's modal semantics; `modal` false restores the plain rail. */
export function applySheetSemantics(shell: HTMLElement | null, modal: boolean): void {
  if (!shell) return;
  shell.classList.toggle("attention-sheet-open", modal);
  const rail = shell.querySelector<HTMLElement>(":scope > .conversation-context");
  for (const child of shell.children) {
    if (child === rail) continue;
    if (modal) child.setAttribute("inert", ""); else child.removeAttribute("inert");
  }
  if (!rail) return;
  if (modal) {
    rail.setAttribute("role", "dialog");
    rail.setAttribute("aria-modal", "true");
    rail.setAttribute("aria-label", LABEL_OPEN);
  } else {
    rail.removeAttribute("role");
    rail.removeAttribute("aria-modal");
    rail.setAttribute("aria-label", LABEL_RAIL);
  }
}

/** Rendered, so focusable: the sheet hides its other sections with CSS. */
const rendered = (node: HTMLElement): boolean => node.getClientRects().length > 0;

/** The sheet's focusable controls, in order, skipping hidden ones. */
export function sheetFocusables(sheet: HTMLElement, visible: (node: HTMLElement) => boolean = rendered): HTMLElement[] {
  return [...sheet.querySelectorAll<HTMLElement>(FOCUSABLE)].filter((node) => node.tabIndex >= 0 && !node.closest("[hidden], [inert]") && visible(node));
}

/**
 * Handle one keydown while the sheet is modal. Returns true when the key was
 * the sheet's to handle (the caller then prevents the default).
 */
export function sheetKeydown(event: Pick<KeyboardEvent, "key" | "shiftKey">, sheet: HTMLElement, active: Element | null, close: () => void, visible?: (node: HTMLElement) => boolean): boolean {
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
