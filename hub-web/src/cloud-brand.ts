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

/** Canonical Cassy ribbons; same geometry as the favicon, theme inherited. */
export function cloudBrand(): string {
  return `<span class="cloud-brand">${cassyMark.replace('<svg ', '<svg aria-hidden="true" focusable="false" ').replace(/<title>.*?<\/title>|<style>[\s\S]*?<\/style>/g, '')}<span>Cassy Cloud</span></span>`;
}

export function projectBadge(path: string | undefined): string {
  return `<span class="project-badge">${escapeHtml(projectName(path))}</span>`;
}
