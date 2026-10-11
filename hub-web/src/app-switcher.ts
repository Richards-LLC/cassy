// cas-eaa3: Commander ↔ Explorer, one Cassy Cloud. Commander links to the
// Explorer on the cloud origin, scoped to the open conversation's project when
// the hub knows its cloud identity. Explorer mirrors this control (spec:
// hub-web/DESIGN.md "App switcher"). Nothing but the path and the project id
// ever goes into the URL: no device credential, pairing fragment, session or
// machine name, and the link sends no referrer.
import { escapeHtml } from "./cloud-brand";

/** The reviewed cloud origin index.html names; also the operator inbox's. */
export const DEFAULT_EXPLORER_ORIGIN = "https://petra-stella-cloud.vercel.app";

/** Named browsing context, so repeated switches reuse one Explorer tab. */
export const EXPLORER_TARGET = "cassy-explorer";

/** An https origin with nothing else in it (no credentials, path, query or fragment), or null. */
export function explorerOrigin(value: string | null | undefined): string | null {
  if (!value) return null;
  try {
    const parsed = new URL(value);
    if (parsed.protocol !== "https:" || parsed.username || parsed.password || parsed.pathname !== "/" || parsed.search || parsed.hash) return null;
    return parsed.origin;
  } catch {
    return null;
  }
}

/** A cloud project identity as the hub reports it (`host/owner/repo` or a slug); anything else is dropped. */
export function cloudProjectId(value: string | null | undefined): string | null {
  const id = value?.trim();
  return id && id.length <= 200 && /^[A-Za-z0-9][A-Za-z0-9._\/-]*$/.test(id) ? id : null;
}

/**
 * Explorer for the current project: its task list filtered to that project
 * (`/explorer/tasks?project_id=…`, the filter Explorer's own project page
 * seeds), or Explorer's home when the project's cloud identity is unknown.
 */
export function explorerUrl(origin: string, projectId?: string | null): string {
  const id = cloudProjectId(projectId);
  const url = new URL(id ? "/explorer/tasks" : "/explorer", origin);
  if (id) url.searchParams.set("project_id", id);
  return url.href;
}

/** The two-app switcher; Commander is the current app. */
export function appSwitcherMarkup(href: string | null): string {
  const explorer = href
    ? `<a class="app-switcher-item" href="${escapeHtml(href)}" target="${EXPLORER_TARGET}" rel="noopener noreferrer" referrerpolicy="no-referrer">Explorer<span class="app-switcher-out" aria-hidden="true">↗</span><span class="sr-only"> (opens in a new tab)</span></a>`
    : '<span class="app-switcher-item" aria-disabled="true" title="Explorer is not configured for this deployment">Explorer</span>';
  return `<nav class="app-switcher" aria-label="Cassy Cloud apps"><span class="app-switcher-item" aria-current="page">Commander</span>${explorer}</nav>`;
}
