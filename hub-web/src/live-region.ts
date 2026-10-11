/**
 * cas-1380: live-region announcements. A region only speaks when its text
 * changes, so an identical result (the same refusal after Review grant
 * again) would be silent. With `repeat`, an unchanged text is cleared now
 * and set again on the next frame, so it is heard once more; a first or
 * changed text is set at once and heard exactly once.
 */
export interface AnnounceOptions {
  readonly repeat?: boolean;
}

export function announceTo(
  region: HTMLElement,
  text: string,
  options: AnnounceOptions = {},
  nextFrame: (run: () => void) => void = (run) => { requestAnimationFrame(() => run()); },
): void {
  if (region.textContent !== text) {
    region.textContent = text;
    return;
  }
  if (!options.repeat || !text) return;
  region.textContent = "";
  // A newer announcement in the meantime wins.
  nextFrame(() => { if (region.textContent === "") region.textContent = text; });
}
