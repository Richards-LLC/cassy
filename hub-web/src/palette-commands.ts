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
): string {
  const project = projectTitle(session.project_dir);
  const codename = session.supervisor || session.name;
  const secondary = [project ? codename : undefined, machine.label, summary?.title, summary?.phase].filter(Boolean).join(" · ");
  const searchText = [project, session.supervisor, summary?.title, summary?.description, summary?.phase].filter(Boolean).join(" ");
  return `<button type="button" class="palette-command" data-palette-machine="${escapeHtml(machine.id)}" data-palette-session="${escapeHtml(session.name)}" data-search-text="${escapeHtml(searchText)}"><span>Jump to ${escapeHtml(project ?? session.name)}</span><small${summary ? ` title="${escapeHtml(summary.description)}"` : ""}>${escapeHtml(secondary)}</small></button>`;
}

/**
 * The palette's control command in the words of a conversation-first user
 * (journey F16): what the device can do, with the terminal term ("Take
 * control", "Release control", "Force takeover") kept in the hint so it still
 * matches the header button and a filter for it still finds the row. An
 * unavailable command says why in the hint.
 */
export function controlCommandCopy(input: {
  readonly heldByMe: boolean;
  readonly forceTakeover: boolean;
  readonly controller?: string;
  readonly disabledReason?: string;
}): { readonly title: string; readonly hint: string } {
  const title = input.heldByMe ? "Let other devices type here" : "Type here from this device";
  if (input.disabledReason) return { title, hint: input.disabledReason };
  if (input.heldByMe) return { title, hint: "Release control of this conversation" };
  if (input.forceTakeover) return { title, hint: input.controller ? `Force takeover from ${input.controller}` : "Force takeover" };
  return { title, hint: "Take control of this conversation" };
}
