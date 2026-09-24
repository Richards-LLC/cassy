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
