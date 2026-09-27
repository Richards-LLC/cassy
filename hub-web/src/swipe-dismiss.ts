/**
 * Swipe-to-dismiss for a thread card on a touch screen (cas-16eed): a failed
 * send or the pinned question slides off sideways under the finger.
 *
 * - Touch and pen only. A mouse drag stays text selection; desktop and
 *   keyboard users have the card's own Dismiss button.
 * - The gesture engages only once the finger has moved mostly sideways, so a
 *   vertical scroll through the thread is never captured. The card declares
 *   `touch-action: pan-y` in CSS so the browser keeps vertical panning and
 *   hands horizontal movement to us.
 * - Past the threshold (a third of the card, at least 72px) release
 *   dismisses; short of it the card settles back.
 * - Under reduced motion the card does not follow the finger and does not
 *   slide out: a swipe past the threshold dismisses at once.
 * - A swipe that engaged swallows the click that follows it, so a swipe that
 *   starts on a chip never also presses it.
 */

export const SWIPE_ENGAGE_PX = 12;
export const SWIPE_MIN_PX = 72;
export const SWIPE_FRACTION = 1 / 3;
const EXIT_MS = 160;

export interface SwipeDismissOptions {
  /** Called once the card should leave. */
  onDismiss: () => void;
  /** True when the reader asked for reduced motion; read at gesture time. */
  reducedMotion?: () => boolean;
}

/** The distance a release must travel to dismiss a card of this width. */
export function swipeThreshold(width: number): number {
  return Math.max(SWIPE_MIN_PX, width * SWIPE_FRACTION);
}

export function prefersReducedMotion(view: Window | null | undefined): boolean {
  return view?.matchMedia?.("(prefers-reduced-motion: reduce)").matches === true;
}

/** Bind the gesture to `element`. Returns the unbind. */
export function bindSwipeDismiss(element: HTMLElement, options: SwipeDismissOptions): () => void {
  let start: { id: number; x: number; y: number } | undefined;
  let engaged = false;
  let dx = 0;
  const reduced = () => options.reducedMotion?.() ?? prefersReducedMotion(element.ownerDocument.defaultView);
  const reset = (animate: boolean) => {
    element.style.transition = animate && !reduced() ? `transform ${EXIT_MS}ms ease-out, opacity ${EXIT_MS}ms ease-out` : "";
    element.style.transform = "";
    element.style.opacity = "";
    delete element.dataset.swiping;
  };
  const swallowClick = () => {
    const stop = (event: Event) => { event.preventDefault(); event.stopPropagation(); };
    element.addEventListener("click", stop, { capture: true, once: true });
    // Nothing clicked after all: stop swallowing once this gesture's click would have landed.
    setTimeout(() => element.removeEventListener("click", stop, { capture: true }), 400);
  };
  const down = (event: PointerEvent) => {
    if (event.pointerType !== "touch" && event.pointerType !== "pen") return;
    if (!event.isPrimary || start) return;
    start = { id: event.pointerId, x: event.clientX, y: event.clientY };
    engaged = false; dx = 0;
  };
  const move = (event: PointerEvent) => {
    if (!start || event.pointerId !== start.id) return;
    const x = event.clientX - start.x;
    const y = event.clientY - start.y;
    if (!engaged) {
      if (Math.abs(x) < SWIPE_ENGAGE_PX) return;
      // Mostly vertical: this is a scroll, not a swipe. Let it go.
      if (Math.abs(x) < Math.abs(y) * 1.5) { start = undefined; return; }
      engaged = true;
      element.dataset.swiping = "true";
      try { element.setPointerCapture(event.pointerId); } catch { /* synthetic or already released pointer */ }
    }
    dx = x;
    if (reduced()) return;
    const width = element.getBoundingClientRect().width || 1;
    element.style.transition = "";
    element.style.transform = `translateX(${dx}px)`;
    element.style.opacity = String(Math.max(0.25, 1 - Math.abs(dx) / width));
  };
  const up = (event: PointerEvent) => {
    if (!start || event.pointerId !== start.id) return;
    start = undefined;
    if (!engaged) return;
    engaged = false;
    swallowClick();
    const width = element.getBoundingClientRect().width || 0;
    if (Math.abs(dx) < swipeThreshold(width)) { reset(true); return; }
    if (reduced()) { reset(false); options.onDismiss(); return; }
    element.style.transition = `transform ${EXIT_MS}ms ease-in, opacity ${EXIT_MS}ms ease-in`;
    element.style.transform = `translateX(${Math.sign(dx) * (width + 24)}px)`;
    element.style.opacity = "0";
    setTimeout(() => { reset(false); options.onDismiss(); }, EXIT_MS);
  };
  const cancel = (event: PointerEvent) => {
    if (!start || event.pointerId !== start.id) return;
    start = undefined;
    if (engaged) { engaged = false; reset(true); }
  };
  element.addEventListener("pointerdown", down);
  element.addEventListener("pointermove", move);
  element.addEventListener("pointerup", up);
  element.addEventListener("pointercancel", cancel);
  return () => {
    element.removeEventListener("pointerdown", down);
    element.removeEventListener("pointermove", move);
    element.removeEventListener("pointerup", up);
    element.removeEventListener("pointercancel", cancel);
  };
}
