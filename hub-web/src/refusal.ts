/**
 * Plain words for a refused supervisor message (journey evaluation F6).
 *
 * The hub and the daemon refuse a send with protocol codes the operator cannot
 * act on: the hub answers every authorization failure with `forbidden`
 * (hub/server.rs `handle_client_message`: no `message:send` scope, or this
 * device does not hold the session lease), and the daemon prefixes its enqueue
 * failures with `semantic message enqueue failed:` (daemon runtime
 * delivery.rs), the commonest being an `in_reply_to` ask that is no longer
 * open. Each maps to a reason and the one step that gets the message through.
 */
export interface Refusal {
  /** Why the message did not go, in the operator's words. */
  readonly reason: string;
  /** What to do next. */
  readonly next: string;
}

const RULES: ReadonlyArray<readonly [RegExp, Refusal]> = [
  [/in_reply_to|not a supervisor turn|belongs to factory session/i, {
    reason: "The question it answered is no longer open.",
    next: "Edit it and send it as a new message.",
  }],
  [/forbidden|authori[sz]ation|permission|not allowed|denied|lease|observ|control/i, {
    reason: "This device isn't the one in control of the session.",
    next: "Take control from the header, then retry.",
  }],
  [/authenticat|credential|expired|revoked|pair/i, {
    reason: "This device's pairing is no longer accepted.",
    next: "Re-pair this device, then retry.",
  }],
  [/reconnect|disconnect|closed|timed? ?out|unreachable|offline/i, {
    reason: "The connection to the machine dropped.",
    next: "Retry once the session is live again.",
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

/** One line for the composer status: reason, then next step. */
export function refusalSentence(detail: string | undefined): string {
  const { reason, next } = refusal(detail);
  return `Not sent. ${reason} ${next}`;
}
