import cassyMark from "../public/favicon.svg?raw";

export function escapeHtml(value: string): string {
  return value.replace(/[&<>"']/g, (character) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[character]!);
}

export function projectName(path: string | undefined): string {
  return projectTitle(path) || "Project unavailable";
}

/** The project's folder name, or undefined when the session names no project
 * (callers then title by the supervisor codename, not a status phrase). */
export function projectTitle(path: string | undefined): string | undefined {
  return path?.trim().split(/[\\/]+/).filter(Boolean).at(-1) || undefined;
}

/** The favicon itself as an image: one asset, one look in every scheme. An
 * <img> keeps its gradient ids private, so a hidden copy of the brand (the
 * thread header hides one) can never break another copy's fills. */
const CASSY_MARK_SRC = `data:image/svg+xml,${encodeURIComponent(cassyMark.trim())}`;

/** Canonical Cassy ribbons on the violet tile; same file as the favicon. */
export function cloudBrand(): string {
  return `<span class="cloud-brand"><img class="cloud-brand-mark" src="${CASSY_MARK_SRC}" alt="" width="32" height="32" decoding="async" draggable="false"><span>Cassy Cloud</span></span>`;
}

/**
 * The list header's lockup as the Cassy Cloud apps switch (cas-eaa3): the same
 * mark and wordmark, plus a chevron, opening the Commander ↔ Explorer popover
 * (#app-switcher). The lockup is the switch so the header gains no width or row.
 */
export function cloudBrandSwitcher(): string {
  return `<button id="app-switcher-toggle" class="cloud-brand cloud-brand-switch" type="button" popovertarget="app-switcher" aria-label="Cassy Cloud apps" title="Switch between Commander and Explorer"><img class="cloud-brand-mark" src="${CASSY_MARK_SRC}" alt="" width="32" height="32" decoding="async" draggable="false"><span>Cassy Cloud</span><svg class="cloud-brand-chevron" viewBox="0 0 10 10" aria-hidden="true" focusable="false"><path d="M2 3.5 5 6.5 8 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg></button>`;
}

export function projectBadge(path: string | undefined): string {
  return `<span class="project-badge">${escapeHtml(projectName(path))}</span>`;
}
