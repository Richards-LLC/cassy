import type { ConversationHistory } from "./conversation-history";

/**
 * What the grid holds instead of the supervisor's pane: the conversation's
 * opening or reconnecting card (`connecting`), the attached session's "not
 * started yet" slot (`no-panes`), or nothing, because the pane is up (`none`).
 */
export type GridPlaceholder = "connecting" | "no-panes" | "none";

/**
 * cas-fc2c: the thread is normally mounted once the session's supervisor pane
 * is known, so before the session attaches nothing of it shows. After a reload during
 * an outage, that hid the messages this browser kept: a message still waiting
 * to go, one not confirmed, one not sent. While the conversation is still
 * opening or reconnecting and the thread has something to show, the thread
 * goes up on its own, above the connecting card. The panes take it over when
 * they arrive.
 */
export function threadBeforePanes(input: {
  placeholder: GridPlaceholder;
  history: ConversationHistory | undefined;
}): boolean {
  return input.placeholder === "connecting"
    && (input.history?.visibleEvents().length ?? 0) > 0;
}

/** Read the grid's placeholder from the DOM it is painted into. */
export function gridPlaceholder(grid: HTMLElement): GridPlaceholder {
  if (grid.querySelector(":scope > .pane-host")) return "none";
  const empty = grid.querySelector<HTMLElement>(":scope > .empty");
  if (!empty) return "none";
  return empty.classList.contains("empty-pane-slot") ? "no-panes" : "connecting";
}
