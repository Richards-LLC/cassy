import { escapeHtml, projectTitle } from "./cloud-brand";
import type { HubSession, SessionCardSummary } from "./types";

/**
 * One "Jump to" row in the command palette. The list titles conversations by
 * project (journey F7), so the palette row leads with the project too and the
 * generated codename is secondary: it opens the row's second line, ahead of
 * the machine and the session summary (3.30.0 journey F2). With no project
 * named, the session name leads, as the list falls back to the codename. The
 * filter indexes the project, the codename and the summary, so typing any of
 * them finds the session (cas-cfcb).
 */
export function sessionJumpCommandMarkup(
  machine: { readonly id: string; readonly label: string },
  session: Pick<HubSession, "name" | "project_dir" | "supervisor">,
  summary?: Pick<SessionCardSummary, "title" | "description" | "phase">,
): string {
  const project = projectTitle(session.project_dir);
  const codename = session.supervisor || session.name;
  const secondary = [project ? codename : undefined, machine.label, summary?.title, summary?.phase].filter(Boolean).join(" · ");
  const searchText = [project, session.supervisor, summary?.title, summary?.description, summary?.phase].filter(Boolean).join(" ");
  return `<button type="button" class="palette-command" data-palette-machine="${escapeHtml(machine.id)}" data-palette-session="${escapeHtml(session.name)}" data-search-text="${escapeHtml(searchText)}"><span>Jump to ${escapeHtml(project ?? session.name)}</span><small${summary ? ` title="${escapeHtml(summary.description)}"` : ""}>${escapeHtml(secondary)}</small></button>`;
}
