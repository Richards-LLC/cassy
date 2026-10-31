import { sentMessageLabel } from "./sent-attachments";
import { BLOCKER_HINT } from "./context-rail";
import { registerTurnRenderer, renderBody, type SecretAutoDelete, type TurnRenderContext } from "./conversation-view";
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

const TICK = '<svg class="tick" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M2.6 8.6l3.3 3.3L13.4 4.4"/></svg>';

export function askOptions(reply: OperatorReply): string[] {
  const options = (reply.options ?? []).map((option) => option.trim()).filter(Boolean);
  return options;
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
  handled: "No longer waiting: handled in earlier history.",
};

/** True while `reply` still waits on the operator (an unanswered ask, an unacknowledged blocker). */
export function isWaiting(reply: OperatorReply, context: Pick<TurnRenderContext, "history">): boolean {
  return context.history?.waiting().some((item) => item.notification_id === reply.notification_id) === true;
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
    sent.append(tick(document), document.createTextNode(sentMessageLabel(answer.text)));
    sent.setAttribute("aria-label", `You replied: ${sentMessageLabel(answer.text)}`);
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
  object.append(body);
  if (foot.childNodes.length > 0) object.append(foot);
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
  } else if (context.history?.answered(reply.notification_id)) {
    // cas-71af (aac8 QA F01): a blocker the operator replied to is handled.
    // It quiets to the supervisor's colour, as an answered ask does, and says
    // so with a tick, instead of staying a red alarm with no sign of it.
    // cas-e829: only a reply bound to THIS blocker (in_reply_to) says so.
    // The check used to find the blocker itself in the history, which is
    // always true, so every blocker that stopped waiting claimed a reply.
    object.dataset.acknowledged = "true";
    object.setAttribute("aria-label", `Blocker from ${context.supervisor}, acknowledged`);
    const handled = document.createElement("p"); handled.className = "blk-handled";
    handled.append(tick(document), document.createTextNode("Acknowledged — you replied"));
    body.append(handled);
  } else if (!context.history?.retirement(reply.notification_id) && context.history?.writtenSince(reply.notification_id)) {
    // A later message from the operator, not bound to this blocker: it no
    // longer waits, and the line says only what is true.
    // It quiets like a handled blocker (it no longer waits), without a tick.
    object.dataset.acknowledged = "true";
    object.setAttribute("aria-label", `Blocker from ${context.supervisor}, you've written since`);
    const later = document.createElement("p"); later.className = "blk-handled blk-later";
    later.textContent = "You've written since this";
    body.append(later);
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

/** The card's deletion choices; "" leaves it to the project setting. */
const SECRET_AUTO_DELETE_CHOICES: readonly (readonly [SecretAutoDelete | "", string])[] = [
  ["", "Project setting (keep unless set)"],
  ["keep", "Keep the file"],
  ["after_use", "Delete once the agent has used it"],
  ["1h", "Delete after 1 hour"],
  ["24h", "Delete after 24 hours"],
];

/** "once used" or "after 2 hours" for the hub's auto_delete label; undefined keeps. */
function secretDeletionPhrase(label: string | null | undefined): string | undefined {
  if (!label) return undefined;
  if (label === "after_use") return "once used";
  const match = /^(\d+)([mhd])$/.exec(label);
  if (!match) return undefined;
  const count = Number(match[1]);
  const unit = { m: "minute", h: "hour", d: "day" }[match[2] as "m" | "h" | "d"];
  return `after ${count} ${unit}${count === 1 ? "" : "s"}`;
}

/**
 * What this page's answer to a secret request said, by request id, until the
 * hub's reply reaches the history (cas-1996). Never the value.
 */
const secretOutcomes = new Map<number, string>();

/**
 * cas-1996: an agent asks for a credential. The card reuses the ask's fused
 * tray (no new shape): the request names the secret and says what it is for
 * and who can read it; the tray holds a masked field, Save securely and
 * Decline. The value goes to the hub's secrets route through
 * `context.answerSecret`, never a send, so it is in no message, journal or
 * history. The hub replies to the supervisor with the saved file's path.
 */
export function renderSecretObject(reply: OperatorReply, context: TurnRenderContext): HTMLElement {
  const { document } = context;
  const name = reply.summary.trim();
  const object = document.createElement("div");
  object.className = "obj t-a secret";
  object.setAttribute("role", "group");
  object.setAttribute("aria-label", `Secret ${name} requested by ${context.supervisor}`);
  object.dataset.notificationId = String(reply.notification_id);
  const body = document.createElement("div"); body.className = "obj-body";
  const title = document.createElement("p"); title.className = "secret-title";
  const label = document.createElement("span"); label.textContent = "Asking for a secret: ";
  const code = document.createElement("code"); code.className = "secret-name"; code.textContent = name;
  title.append(label, code);
  const scope = document.createElement("p"); scope.className = "secret-scope";
  scope.textContent = `Saved only on ${context.machine ?? "the session's machine"}, as a private file (mode 600) that ${context.supervisor} can read. It never appears in chat or history.`;
  body.append(title, ...context.body(), scope);
  const foot = document.createElement("div"); foot.className = "obj-foot";
  const answer = context.history?.answered(reply.notification_id);
  const outcome = answer ? sentMessageLabel(answer.text) : secretOutcomes.get(reply.notification_id);
  const retired = outcome ? undefined : context.history?.retirement(reply.notification_id);
  if (outcome) {
    object.dataset.answered = "true";
    const sent = document.createElement("span"); sent.className = "chip sent";
    if (answer) sent.dataset.state = answer.state;
    sent.append(tick(document), document.createTextNode(outcome));
    foot.append(sent);
  } else if (retired && retired !== "dismissed") {
    object.dataset.answered = "false";
    object.dataset.retired = retired;
    const note = document.createElement("p"); note.className = "ask-retired"; note.textContent = RETIRED_LINES[retired];
    foot.append(note);
  } else {
    object.dataset.answered = "false";
    // No <form> and no submit: a password field submitted in a form is what
    // browsers offer to save. The ignore attributes keep 1Password, LastPass,
    // Bitwarden and Dashlane from offering to store or fill it.
    const form = document.createElement("div"); form.className = "secret-form";
    const input = document.createElement("input");
    input.type = "password"; input.className = "secret-value";
    input.autocomplete = "off"; input.spellcheck = false;
    input.setAttribute("autocapitalize", "off"); input.setAttribute("autocorrect", "off");
    for (const [name, value] of [["data-1p-ignore", ""], ["data-lpignore", "true"], ["data-bwignore", "true"], ["data-form-type", "other"]]) input.setAttribute(name!, value!);
    input.setAttribute("aria-label", `Value for ${name}`);
    input.placeholder = "Paste the value";
    // cas-2f55: deleting the saved file is opt-in; the first choice follows
    // the project's secrets.auto_delete, which keeps the file by default.
    const expiry = document.createElement("select"); expiry.className = "secret-auto-delete";
    expiry.setAttribute("aria-label", `After saving ${name}`);
    for (const [value, label] of SECRET_AUTO_DELETE_CHOICES) {
      const option = document.createElement("option"); option.value = value; option.textContent = label;
      expiry.append(option);
    }
    const save = document.createElement("button"); save.type = "button"; save.className = "chip secret-save"; save.textContent = "Save securely";
    const decline = document.createElement("button"); decline.type = "button"; decline.className = "chip secret-decline"; decline.textContent = "Decline";
    const status = document.createElement("p"); status.className = "secret-status"; status.setAttribute("role", "status");
    const controls = [input, expiry, save, decline];
    if (!context.answerSecret) for (const control of controls) control.disabled = true;
    const send = async (answerWith: Parameters<NonNullable<TurnRenderContext["answerSecret"]>>[1]): Promise<void> => {
      if (!context.answerSecret) return;
      for (const control of controls) control.disabled = true;
      status.textContent = "decline" in answerWith ? "Declining…" : "Saving on the machine…";
      try {
        const saved = await context.answerSecret(reply, answerWith);
        input.value = "";
        const deletes = "decline" in answerWith ? undefined : secretDeletionPhrase(saved.auto_delete);
        const text = "decline" in answerWith ? "Declined" : `Saved to ${saved.path ?? "the machine"}${deletes ? ` · deleted ${deletes}` : ""}`;
        secretOutcomes.set(reply.notification_id, text);
        object.dataset.answered = "true";
        const sent = document.createElement("span"); sent.className = "chip sent";
        sent.append(tick(document), document.createTextNode(text));
        foot.replaceChildren(sent);
      } catch (error) {
        status.textContent = error instanceof Error && error.message ? error.message : "Could not save the secret. Try again.";
        for (const control of controls) control.disabled = false;
      }
    };
    const submit = (): void => {
      if (!input.value.trim()) { status.textContent = "Paste the value first."; input.focus(); return; }
      const choice = expiry.value as SecretAutoDelete | "";
      void send(choice ? { value: input.value, auto_delete: choice } : { value: input.value });
    };
    save.onclick = submit;
    input.onkeydown = (event) => { if (event.key === "Enter") { event.preventDefault(); submit(); } };
    decline.onclick = () => { void send({ decline: true }); };
    form.append(input, expiry, save, decline);
    foot.append(form, status);
  }
  object.append(body, foot);
  return object;
}

/** Register the objects on the Pebble 2 seam. Returns the unregister. */
export function installAttentionObjects(): () => void {
  const ask = registerTurnRenderer("ask", renderAskObject);
  const blocker = registerTurnRenderer("blocker", renderBlockerObject);
  const secret = registerTurnRenderer("secret", renderSecretObject);
  return () => { ask(); blocker(); secret(); };
}
