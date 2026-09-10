export function escapeHtml(value: string): string {
  return value.replace(/[&<>"']/g, (character) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[character]!);
}

export function projectName(path: string | undefined): string {
  return path?.trim().split(/[\\/]+/).filter(Boolean).at(-1) || "Project unavailable";
}

/** One inline vector, no font or asset request. The C opens into a cloud. */
export function cloudBrand(): string {
  return `<span class="cloud-brand"><svg viewBox="0 0 32 32" aria-hidden="true" focusable="false"><path d="M24 8a12 12 0 1 0 0 16" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round"/><path d="M17 22a4 4 0 0 1-.4-8 5.5 5.5 0 0 1 10.6 0A4 4 0 0 1 27 22Z" fill="currentColor"/></svg><span>Cassy Cloud</span></span>`;
}

export function projectBadge(path: string | undefined): string {
  return `<span class="project-badge">${escapeHtml(projectName(path))}</span>`;
}
