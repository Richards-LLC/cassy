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

export function projectBadge(path: string | undefined): string {
  return `<span class="project-badge">${escapeHtml(projectName(path))}</span>`;
}
