import cassyMark from "../public/favicon.svg?raw";

export function escapeHtml(value: string): string {
  return value.replace(/[&<>"']/g, (character) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[character]!);
}

export function projectName(path: string | undefined): string {
  return path?.trim().split(/[\\/]+/).filter(Boolean).at(-1) || "Project unavailable";
}

/** Canonical Cassy ribbons; same geometry as the favicon, theme inherited. */
export function cloudBrand(): string {
  return `<span class="cloud-brand">${cassyMark.replace('<svg ', '<svg aria-hidden="true" focusable="false" ').replace(/<title>.*?<\/title>|<style>[\s\S]*?<\/style>/g, '')}<span>Cassy Cloud</span></span>`;
}

export function projectBadge(path: string | undefined): string {
  return `<span class="project-badge">${escapeHtml(projectName(path))}</span>`;
}
