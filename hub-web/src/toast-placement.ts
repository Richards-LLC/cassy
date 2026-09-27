/** A box as getBoundingClientRect reports it. */
export interface Box { left: number; right: number; top: number; bottom: number; width: number; height: number }

/**
 * Where a toast should sit so it never covers the reconnect banner (cas-00cc):
 * the pixel top that puts it just below the banner when, at its stylesheet
 * position, it would land on it; otherwise undefined (keep the stylesheet's).
 */
export function toastTopClearOfBanner(toastTop: number, toast: Box, banner: Box | undefined, gap = 8): number | undefined {
  if (!banner || banner.width === 0 || banner.bottom <= 0 || !Number.isFinite(toastTop)) return undefined;
  const overlapsX = toast.width === 0 || (toast.left < banner.right && toast.right > banner.left);
  const overlapsY = toastTop < banner.bottom && toastTop + Math.max(toast.height, 1) > banner.top;
  return overlapsX && overlapsY ? Math.ceil(banner.bottom + gap) : undefined;
}

/**
 * Where a toast sits while a conversation is open beside other columns
 * (3.30.0 journey F8): at the top right of the viewport it landed on the
 * context rail's first heading. Inside the thread column, just below the
 * conversation header, it covers no heading, as the phone layout already
 * places it. Returns the fixed-position top and right in pixels, or
 * undefined when there is no side-by-side thread column to place it in (the
 * stylesheet's position stands).
 */
export function toastPlacementInThread(header: Box | undefined, column: Box | undefined, viewportWidth: number, gap = 8): { top: number; right: number } | undefined {
  if (!header || !column || header.width === 0 || column.width === 0) return undefined;
  // A thread column that fills the viewport is the phone layout: the
  // stylesheet already puts the toast below the header there.
  if (column.left <= 0 && column.right >= viewportWidth) return undefined;
  return { top: Math.ceil(header.bottom + gap), right: Math.max(gap, Math.floor(viewportWidth - column.right + gap)) };
}

/**
 * cas-71af (dfb2 QA F01): on the phone's conversation list, below the brand
 * row the toast sat over the "Conversations" title and Pair a machine for its
 * whole life. It sits just above the list's bottom action instead, where a
 * phone keeps its passing notes, clear of every heading. Returns the top in
 * pixels, or undefined when there is no such action on screen or no room.
 */
export function toastTopAboveAction(toast: Box, action: Box | undefined, clearOf: Box | undefined, gap = 8): number | undefined {
  if (!action || action.width === 0 || action.top <= 0) return undefined;
  const top = Math.floor(action.top - gap - Math.max(toast.height, 1));
  if (clearOf && top < clearOf.bottom + gap) return undefined;
  return top;
}
