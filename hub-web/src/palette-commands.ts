import { escapeHtml, projectName } from "./cloud-brand";
import type { HubSession, SessionCardSummary } from "./types";

/**
 * One "Jump to" row in the command palette. The list titles conversations by
 * project (journey F7), so the palette names the project too: it leads the
 * row's second line and is indexed with the supervisor codename and the
 * session summary, so typing a project name finds its session (cas-cfcb).
 */
export function sessionJumpCommandMarkup(
  machine: { readonly id: string; readonly label: string },
  session: Pick<HubSession, "name" | "project_dir" | "supervisor">,
  summary?: Pick<SessionCardSummary, "title" | "description" | "phase">,
): string {
  const project = session.project_dir?.trim() ? projectName(session.project_dir) : undefined;
  const secondary = [project, machine.label, summary?.title, summary?.phase].filter(Boolean).join(" · ");
  const searchText = [project, session.supervisor, summary?.title, summary?.description, summary?.phase].filter(Boolean).join(" ");
  return `<button type="button" class="palette-command" data-palette-machine="${escapeHtml(machine.id)}" data-palette-session="${escapeHtml(session.name)}" data-search-text="${escapeHtml(searchText)}"><span>Jump to ${escapeHtml(session.name)}</span><small${summary ? ` title="${escapeHtml(summary.description)}"` : ""}>${escapeHtml(secondary)}</small></button>`;
}
