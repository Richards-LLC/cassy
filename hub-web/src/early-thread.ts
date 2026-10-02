import type { ConversationHistory } from "./conversation-history";

/**
 * What the pane grid holds instead of panes: the conversation's opening or
 * reconnecting card (`connecting`), the attached session's "No panes yet"
 * slot (`no-panes`), or nothing, because the panes are up (`none`).
 */
export type GridPlaceholder = "connecting" | "no-panes" | "none";

/**
 * cas-fc2c: the thread is normally mounted over the session's supervisor pane,
 * so before the session attaches nothing of it shows. After a reload during
 * an outage, that hid the messages this browser kept: a message still waiting
 * to go, one not confirmed, one not sent. While the conversation is still
 * opening or reconnecting and the thread has something to show, the thread
 * goes up on its own, above the connecting card. The panes take it over when
 * they arrive.
 */
export function threadBeforePanes(input: {
  presentation: "conversation" | "terminal";
  placeholder: GridPlaceholder;
  history: ConversationHistory | undefined;
}): boolean {
  return input.presentation === "conversation"
    && input.placeholder === "connecting"
    && (input.history?.visibleEvents().length ?? 0) > 0;
}

/** Read the grid's placeholder from the DOM it is painted into. */
export function gridPlaceholder(grid: HTMLElement): GridPlaceholder {
  if (grid.querySelector(":scope > .primary-pane-slot")) return "none";
  const empty = grid.querySelector<HTMLElement>(":scope > .empty");
  if (!empty) return "none";
  return empty.classList.contains("empty-pane-slot") ? "no-panes" : "connecting";
}
