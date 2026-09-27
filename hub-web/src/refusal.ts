/**
 * Plain words for a refused supervisor message (journey evaluation F6).
 *
 * The hub and the daemon refuse a send with protocol codes the operator cannot
 * act on: the hub answers every authorization failure with `forbidden`
 * (hub/server.rs `handle_client_message`: no `message:send` scope, or this
 * device does not hold the session lease), the hub answers
 * `upstream_unavailable` (retryable, cas-a0e2) when the session's daemon
 * upstream could not take the message, and the daemon prefixes its enqueue
 * failures with `semantic message enqueue failed:` (daemon runtime
 * delivery.rs), the commonest being an `in_reply_to` ask that is no longer
 * open. Each maps to a reason and the one step that gets the message through.
 */
export interface Refusal {
  /** Why the message did not go, in the operator's words. */
  readonly reason: string;
  /** What to do next. */
  readonly next: string;
  /**
   * The control that performs the next step, when the refused message can
   * carry it. The copy names only controls the operator can reach from the
   * message itself (cas-3433): the conversation header has no Take control.
   */
  readonly action?: "take-control" | "await-session";
}

const RULES: ReadonlyArray<readonly [RegExp, Refusal]> = [
  // cas-a355: a held send that ran out of time (connection-state-view.ts
  // `outageRefusal`). It never left this browser, so the next step is a Retry
  // once the session is back, never re-pairing. First, because the machine
  // label it names could contain any of the words below.
  [/^Not sent: lost connection to /, {
    reason: "The session didn't come back while it waited.",
    next: "Retry once the session is live again.",
    // cas-d15c (cas-a355 N2): once the session is live again the view says
    // Retry will go through instead.
    action: "await-session",
  }],
  [/in_reply_to|not a supervisor turn|belongs to factory session/i, {
    reason: "The question it answered is no longer open.",
    next: "Edit it and send it as a new message.",
  }],
  [/forbidden|authori[sz]ation|permission|not allowed|denied|lease|observ|control/i, {
    reason: "This device isn't the one in control of the session.",
    next: "Take control, then retry.",
    action: "take-control",
  }],
  [/authenticat|credential|expired|revoked|pair/i, {
    reason: "This device's pairing is no longer accepted.",
    next: "Re-pair this device, then retry.",
  }],
  // cas-a0e2: `upstream_unavailable` is the hub's retryable answer when the
  // session's daemon upstream is missing; the message never reached it.
  [/upstream_unavailable|reconnect|disconnect|closed|timed? ?out|unreachable|offline/i, {
    reason: "The connection to the machine dropped.",
    next: "Retry once the session is live again.",
    action: "await-session",
  }],
  [/enqueue failed|queue/i, {
    reason: "The supervisor's machine couldn't take the message.",
    next: "Retry in a moment.",
  }],
];

const UNKNOWN: Refusal = {
  reason: "The hub didn't accept it.",
  next: "Retry; if it keeps happening, re-pair this device.",
};

export function refusal(detail: string | undefined): Refusal {
  const text = detail ?? "";
  return RULES.find(([pattern]) => pattern.test(text))?.[1] ?? UNKNOWN;
}

/** The composer's line when the refused bubble already says why (cas-4d92). */
export const REFUSED_SEE_ABOVE = "Not sent — see the message above.";

/** One line for the composer status: reason, then next step. */
export function refusalSentence(detail: string | undefined): string {
  const { reason, next } = refusal(detail);
  return `Not sent. ${reason} ${next}`;
}
