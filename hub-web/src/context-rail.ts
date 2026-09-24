import { artifactHref, attachmentSize, attachmentTypeMark } from "./attachment-sheet";
import type { ConversationHistory } from "./conversation-history";
import type { ArtifactRef, OperatorReply } from "./types";

/**
 * Desktop context rail (P10 of the 2026-09-22 design review). The rail used
 * to repeat the thread header — project badge, supervisor, host, a second
 * Cassy Cloud lockup — in 240px. It now carries only what the thread header
 * and the flow do not already show at a glance:
 *
 *   - Waiting on you: open asks and blockers, each a jump to its turn. The
 *     ask pinned above the composer is left out: the pinned card is where it
 *     is answered, so the rail does not show it a third time (journey F18).
 *   - Tasks & progress: the session summary, agents and tasks from status
 *   - Attachments: every artifact the supervisor sent in this thread
 *   - Attention: open attention events for this thread
 *
 * A section with no data is absent, never placeholder copy, and with no
 * section present the rail folds to the 48px context track so the thread gets
 * the width. Everything comes from data hub-web already receives.
 */

export interface ContextRailInput {
  /** The open thread's history: asks, blockers and attachments come from here. */
  readonly history?: ConversationHistory;
  /** Status reported a session summary, agents or tasks for this session. */
  readonly progress: boolean;
  /** Open (grouped, unacknowledged) attention events scoped to this thread. */
  readonly attention: number;
}

export interface ThreadAttachment {
  readonly attachment: ArtifactRef;
  readonly reply: OperatorReply;
}

export type ContextSection = "waiting" | "progress" | "attachments" | "attention";

/** Every artifact sent in the thread, newest first, once per artifact id. */
export function threadAttachments(history: ConversationHistory | undefined): ThreadAttachment[] {
  if (!history) return [];
  const seen = new Set<string>();
  const out: ThreadAttachment[] = [];
  for (let index = history.events.length - 1; index >= 0; index -= 1) {
    const event = history.events[index]!;
    if (event.kind !== "reply") continue;
    for (const attachment of event.value.attachments ?? []) {
      if (seen.has(attachment.artifact_id)) continue;
      seen.add(attachment.artifact_id);
      out.push({ attachment, reply: event.value });
    }
  }
  return out;
}

/** Open asks and blockers the rail lists: everything waiting except the pinned ask. */
export function railWaiting(history: ConversationHistory | undefined): OperatorReply[] {
  if (!history) return [];
  const pinned = history.pinnedAsk()?.notification_id;
  return history.waiting().filter((reply) => reply.notification_id !== pinned);
}

/** Which sections have something unique to show. */
export function contextSections(input: ContextRailInput): ContextSection[] {
  const sections: ContextSection[] = [];
  if (railWaiting(input.history).length > 0) sections.push("waiting");
  if (input.progress) sections.push("progress");
  if (threadAttachments(input.history).length > 0) sections.push("attachments");
  if (input.attention > 0) sections.push("attention");
  return sections;
}

/**
 * First line of a turn, cut at a word near 90 characters: the entry is a
 * pointer to the turn, whose full text the jump lands on. The cut is made
 * here, in words, rather than by a CSS clamp that hides text mid-line.
 */
export const CONTEXT_ENTRY_LIMIT = 90;
export function entryText(message: string): string {
  const line = message.split(/\r?\n/).map((part) => part.trim()).find(Boolean) ?? "";
  if (line.length <= CONTEXT_ENTRY_LIMIT) return line;
  const cut = line.slice(0, CONTEXT_ENTRY_LIMIT);
  const space = cut.lastIndexOf(" ");
  return `${(space > CONTEXT_ENTRY_LIMIT / 2 ? cut.slice(0, space) : cut).trimEnd().replace(/[,;:—–-]$/, "").trimEnd()}…`;
}

/** Bring a thread turn into view and give it focus, so a jump from the rail lands somewhere a keyboard user can continue from. */
function jumpToTurn(document: Document, notificationId: number): void {
  const turn = document.querySelector<HTMLElement>(`.thread [data-key="reply:${notificationId}"]`);
  if (!turn) return;
  if (!turn.hasAttribute("tabindex")) turn.tabIndex = -1;
  turn.scrollIntoView?.({ block: "center" });
  turn.focus({ preventScroll: true });
}

function renderWaiting(document: Document, list: HTMLElement, waiting: readonly OperatorReply[]): void {
  list.replaceChildren(...[...waiting].reverse().map((reply) => {
    const item = document.createElement("li");
    const jump = document.createElement("button"); jump.type = "button"; jump.className = "context-jump";
    jump.dataset.kind = reply.kind ?? "ask";
    jump.dataset.replyTo = String(reply.notification_id);
    const kind = document.createElement("span"); kind.className = "context-kind"; kind.textContent = reply.kind === "blocker" ? "Blocker" : "Question";
    const text = document.createElement("span"); text.className = "context-text"; text.textContent = entryText(reply.message);
    jump.append(kind, text);
    jump.onclick = () => jumpToTurn(document, reply.notification_id);
    item.append(jump);
    return item;
  }));
}

function renderAttachments(document: Document, list: HTMLElement, attachments: readonly ThreadAttachment[]): void {
  list.replaceChildren(...attachments.map(({ attachment }) => {
    const item = document.createElement("li");
    const link = document.createElement("a"); link.className = "context-attachment";
    link.href = artifactHref(attachment);
    link.dataset.artifactId = attachment.artifact_id;
    const mark = attachmentTypeMark(attachment.mime, attachment.name);
    const size = attachmentSize(attachment.size_bytes);
    const plate = document.createElement("span"); plate.className = "context-plate"; plate.textContent = mark; plate.setAttribute("aria-hidden", "true");
    const name = document.createElement("span"); name.className = "context-text"; name.textContent = attachment.name;
    const meta = document.createElement("span"); meta.className = "context-meta"; meta.textContent = size;
    link.setAttribute("aria-label", `${attachment.name}, ${mark}${size ? `, ${size}` : ""}. Open`);
    link.append(plate, name, meta);
    item.append(link);
    return item;
  }));
}

/**
 * Apply the rail to a conversation shell already in the document. Returns
 * whether the rail is open. Safe to call on every region update: lists are
 * only rebuilt when their content changed.
 */
export function syncContextRail(root: ParentNode, input: ContextRailInput): boolean {
  const rail = root.querySelector<HTMLElement>(".conversation-context");
  if (!rail) return false;
  const document = rail.ownerDocument;
  const sections = contextSections(input);
  const waiting = railWaiting(input.history);
  const attachments = threadAttachments(input.history);
  const waitingList = rail.querySelector<HTMLElement>(".context-waiting");
  const waitingSignature = JSON.stringify(waiting.map((reply) => [reply.notification_id, reply.kind, reply.message]));
  if (waitingList && waitingList.dataset.signature !== waitingSignature) {
    waitingList.dataset.signature = waitingSignature;
    renderWaiting(document, waitingList, waiting);
  }
  const attachmentList = rail.querySelector<HTMLElement>(".context-attachments");
  const attachmentSignature = JSON.stringify(attachments.map(({ attachment }) => [attachment.artifact_id, attachment.name, attachment.mime, attachment.size_bytes]));
  if (attachmentList && attachmentList.dataset.signature !== attachmentSignature) {
    attachmentList.dataset.signature = attachmentSignature;
    renderAttachments(document, attachmentList, attachments);
  }
  for (const section of rail.querySelectorAll<HTMLElement>("[data-section]")) {
    section.hidden = !sections.includes(section.dataset.section as ContextSection);
  }
  const open = sections.length > 0;
  rail.dataset.open = String(open);
  // A folded rail is an empty 48px track: nothing for assistive tech to land on.
  if (open) rail.removeAttribute("aria-hidden"); else rail.setAttribute("aria-hidden", "true");
  rail.closest(".conversation-shell")?.classList.toggle("context-open", open);
  return open;
}
