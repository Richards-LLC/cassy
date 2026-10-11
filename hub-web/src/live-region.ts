/** cas-1380: live-region announcements. Red stub. */
export interface AnnounceOptions {
  readonly repeat?: boolean;
}

export function announceTo(region: HTMLElement, text: string, _options: AnnounceOptions = {}, _nextFrame: (run: () => void) => void = (run) => { requestAnimationFrame(() => run()); }): void {
  if (region.textContent !== text) region.textContent = text;
}
