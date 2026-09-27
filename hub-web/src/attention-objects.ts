import { BLOCKER_HINT, entryText } from "./context-rail";
import { registerTurnRenderer, renderBody, type TurnRenderContext } from "./conversation-view";
import { blockerEvidence } from "./thread-model";
import type { AskRetirement } from "./conversation-history";
import type { OperatorReply } from "./types";

/**
 * Pebble 3 (cas-43f9): ask and blocker as treatment A from the round-3 study —
 * one fused stepped object. The body steps down into a deeper tray under a
 * single 28px outline with the spoken notch at the tray corner; the shadow is
 * cast from `.obj` itself, so it follows the rounded silhouette. An ask's tray
 * holds the quick-reply chips; a blocker's tray is an inset evidence window.
 * Source: docs/design/hub-messaging/round-3/pebble.css (.obj, .t-a, .blk,
 * .chip, .window) and pairs.html.
 */

/** Chips when the payload carries no options (OperatorReplyPayload has none yet). */
export const DEFAULT_ASK_OPTIONS: readonly string[] = ["Yes, go ahead", "Hold"];

const TICK = '<svg class="tick" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M2.6 8.6l3.3 3.3L13.4 4.4"/></svg>';

export function askOptions(reply: OperatorReply): string[] {
  const options = (reply.options ?? []).map((option) => option.trim()).filter(Boolean);
  return options.length > 0 ? options : [...DEFAULT_ASK_OPTIONS];
}

/**
 * What a question that stopped waiting without an answer says in the thread
 * (cas-16eed). A dismissed one keeps its choices, so it can still be answered
 * from here; one whose session ended, or that the supervisor moved past,
 * offers none.
 */
export const RETIRED_LINES: Readonly<Record<AskRetirement, string>> = {
  dismissed: "Dismissed. You can still answer here.",
  "session-ended": "No longer waiting: the session that asked has ended.",
};

/** The collapsed in-flow copy's pointer to the pinned tray. */
export const WAITING_LINE = "Waiting on you — answer below";

/** True while `reply` still waits on the operator (an unanswered ask, an unacknowledged blocker). */
export function isWaiting(reply: OperatorReply, context: Pick<TurnRenderContext, "history">): boolean {
  return context.history?.waiting().some((item) => item.notification_id === reply.notification_id) === true;
}

/** True when `reply` is the ask the view pins above the composer (the most recent unanswered one). */
export function isPinnedAsk(reply: OperatorReply, context: Pick<TurnRenderContext, "history">): boolean {
  return context.history?.pinnedAsk()?.notification_id === reply.notification_id;
}

function tick(document: Document): Element {
  const template = document.createElement("template"); template.innerHTML = TICK;
  return template.content.firstElementChild!;
}

export function renderAskObject(reply: OperatorReply, context: TurnRenderContext): HTMLElement {
  const { document } = context;
  const object = document.createElement("div");
  object.className = "obj t-a";
  object.setAttribute("role", "group");
  object.setAttribute("aria-label", `Question from ${context.supervisor}`);
  object.dataset.notificationId = String(reply.notification_id);
  const body = document.createElement("div"); body.className = "obj-body"; body.append(...context.body());
  const foot = document.createElement("div"); foot.className = "obj-foot";
  const answer = context.history?.answered(reply.notification_id);
  // P1 (cas-b1ee): while this ask is the one pinned above the composer, its
  // copy in the flow collapses to a supervisor pebble with a waiting line; the
  // chips live only in the pinned tray, so the operator never sees two live
  // copies of the same question. Older unanswered asks keep their chips.
  if (!answer && !context.pinned && isPinnedAsk(reply, context)) {
    object.className = "obj t-a ask-collapsed";
    object.dataset.answered = "false";
    object.dataset.collapsed = "true";
    // A compact reference, not a second copy (journey F18): its first line,
    // cut at a word, points at the pinned card that carries the full question.
    const excerpt = document.createElement("p"); excerpt.className = "ask-excerpt"; excerpt.textContent = entryText(reply.message);
    const wait = document.createElement("p"); wait.className = "ask-waiting"; wait.textContent = WAITING_LINE;
    body.replaceChildren(excerpt, wait);
    object.append(body);
    return object;
  }
  const retired = answer || context.pinned ? undefined : context.history?.retirement(reply.notification_id);
  if (retired) {
    // cas-16eed: no longer waiting, so it quiets like an answered question
    // and says why instead of showing an answer.
    object.dataset.answered = "false";
    object.dataset.retired = retired;
    const note = document.createElement("p"); note.className = "ask-retired"; note.textContent = RETIRED_LINES[retired];
    foot.append(note);
  }
  if (answer) {
    // The chosen reply stays in the tray as sent; the other choices leave.
    // Handled, it quiets to the supervisor's colour (journey F12): only what
    // still waits keeps the attention treatment.
    object.dataset.answered = "true";
    const sent = document.createElement("span"); sent.className = "chip sent"; sent.dataset.state = answer.state;
    sent.append(tick(document), document.createTextNode(answer.text));
    sent.setAttribute("aria-label", `You replied: ${answer.text}`);
    foot.append(sent);
  } else if (!retired || retired === "dismissed") {
    object.dataset.answered = "false";
    for (const option of askOptions(reply)) {
      const chip = document.createElement("button"); chip.type = "button"; chip.className = "chip"; chip.textContent = option;
      chip.disabled = !context.respond;
      chip.onclick = () => context.respond?.(reply, option);
      foot.append(chip);
    }
  }
  object.append(body, foot);
  return object;
}

export function renderBlockerObject(reply: OperatorReply, context: TurnRenderContext): HTMLElement {
  const { document } = context;
  const object = document.createElement("div");
  object.className = "obj t-a blk";
  object.setAttribute("role", "group");
  object.setAttribute("aria-label", `Blocker from ${context.supervisor}`);
  object.dataset.notificationId = String(reply.notification_id);
  const { text, evidence } = blockerEvidence(reply.message);
  const body = document.createElement("div"); body.className = "obj-body";
  body.append(...renderBody(document, { ...reply, message: text }, context));
  const waiting = isWaiting(reply, context);
  object.dataset.waiting = String(waiting);
  if (waiting) {
    const hint = document.createElement("p"); hint.className = "blk-hint"; hint.textContent = BLOCKER_HINT;
    body.append(hint);
  } else if (context.history?.events.some((event) => event.kind === "reply" && event.value.notification_id === reply.notification_id)) {
    // cas-71af (aac8 QA F01): a blocker the operator replied to is handled.
    // It quiets to the supervisor's colour, as an answered ask does, and says
    // so with a tick, instead of staying a red alarm with no sign of it.
    object.dataset.acknowledged = "true";
    object.setAttribute("aria-label", `Blocker from ${context.supervisor}, acknowledged`);
    const handled = document.createElement("p"); handled.className = "blk-handled";
    handled.append(tick(document), document.createTextNode("Acknowledged — you replied"));
    body.append(handled);
  }
  // cas-16eed: a blocker from a session that has ended no longer waits, and says so.
  const retired = waiting ? undefined : context.history?.retirement(reply.notification_id);
  if (retired) {
    object.dataset.retired = retired;
    const note = document.createElement("p"); note.className = "blk-retired"; note.textContent = RETIRED_LINES[retired];
    body.append(note);
  }
  object.append(body);
  if (evidence) {
    const foot = document.createElement("div"); foot.className = "obj-foot";
    const window = document.createElement("code"); window.className = "window"; window.textContent = evidence;
    foot.append(window);
    object.append(foot);
  }
  return object;
}

/** Register both objects on the Pebble 2 seam. Returns the unregister. */
export function installAttentionObjects(): () => void {
  const ask = registerTurnRenderer("ask", renderAskObject);
  const blocker = registerTurnRenderer("blocker", renderBlockerObject);
  return () => { ask(); blocker(); };
}
