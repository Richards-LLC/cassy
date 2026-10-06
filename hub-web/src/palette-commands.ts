import { escapeHtml, projectTitle } from "./cloud-brand";
import type { HubSession, SessionCardSummary } from "./types";

/**
 * One "Jump to" row in the command palette. The list titles conversations by
 * project (journey F7), so the palette row leads with the project too and the
 * generated codename is secondary: it opens the row's description, ahead of
 * the machine and the session summary (3.30.0 journey F2). The description
 * sits right of the title on a desktop and drops beneath it on a phone
 * (styles.css `.palette-command`), as docs/qa/journeys.md HUB-J3 says. With no project
 * named, the session name leads, as the list falls back to the codename. The
 * filter indexes the project, the codename and the summary, so typing any of
 * them finds the session (cas-cfcb).
 */
export function sessionJumpCommandMarkup(
  machine: { readonly id: string; readonly label: string },
  session: Pick<HubSession, "name" | "project_dir" | "supervisor">,
  summary?: Pick<SessionCardSummary, "title" | "description" | "phase">,
  state: { readonly current?: boolean; readonly needsYou?: boolean } = {},
): string {
  const project = projectTitle(session.project_dir);
  const codename = session.supervisor || session.name;
  const secondary = [project ? codename : undefined, machine.label, summary?.title, summary?.phase].filter(Boolean).join(" · ");
  const searchText = [project, session.supervisor, summary?.title, summary?.description, summary?.phase].filter(Boolean).join(" ");
  // cas-786a (journey F30): the conversation already open is the palette's
  // current item, so a jump that would go nowhere is never mistaken for one
  // that moves. It reads "Open now" ahead of its description (styles.css).
  const flags = `${state.current ? ' data-palette-current="true" aria-current="true"' : ""}${state.needsYou ? ' data-palette-needs-you="true"' : ""}`;
  return `<button type="button" class="palette-command" data-palette-machine="${escapeHtml(machine.id)}" data-palette-session="${escapeHtml(session.name)}"${flags} data-search-text="${escapeHtml(searchText)}"><span>Jump to ${escapeHtml(project ?? session.name)}</span><small${summary ? ` title="${escapeHtml(summary.description)}"` : ""}>${escapeHtml(secondary)}</small></button>`;
}

/**
 * The command Enter runs in the palette filter (cas-537f, cas-786a). With a
 * query it is the first row on screen, unless that row only jumps to the
 * conversation already open and another row is on screen. With no query it
 * is the first other conversation that needs the operator (unread or
 * waiting), else the first other conversation, else the first command.
 */
export function paletteEnterTarget<T extends { readonly dataset: DOMStringMap }>(shown: readonly T[], query: string): T | undefined {
  const jumpsElsewhere = (command: T) => command.dataset.paletteSession !== undefined && command.dataset.paletteCurrent !== "true";
  if (!query.trim()) {
    return shown.find((command) => jumpsElsewhere(command) && command.dataset.paletteNeedsYou === "true")
      ?? shown.find(jumpsElsewhere)
      ?? shown[0];
  }
  return shown.find((command) => command.dataset.paletteCurrent !== "true") ?? shown[0];
}
