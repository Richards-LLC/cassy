export interface PairingFailureCopy {
  /** A sentence naming the cause and the next action, never a wire token. */
  message: string;
  /** The invitation may still work, so keep it rather than sending the operator back to the machine. */
  keepInvitation: boolean;
}

/**
 * A hub error body is only shown to a human when it is already a sentence.
 * `unauthorized` and `pairing exchange refused` are wire tokens: they name no
 * cause a person can act on, and printing them is what left the phone stuck.
 */
function hubSentence(body: string): string | null {
  const text = readErrorText(body).trim();
  if (!text || !/\s/.test(text) || !/[.!?]$/.test(text)) return null;
  return text;
}

function readErrorText(body: string): string {
  try {
    const payload = JSON.parse(body) as { error_description?: unknown; error?: unknown };
    if (typeof payload.error_description === "string") return payload.error_description;
    if (typeof payload.error === "string") return payload.error;
    return "";
  } catch {
    return body;
  }
}

function retryAfterCopy(value: string | null | undefined): string {
  const text = value?.trim();
  if (!text || !/^\d+$/.test(text)) return "a minute";
  const seconds = Number(text);
  if (!Number.isSafeInteger(seconds) || seconds < 1 || seconds > 60) return "a minute";
  return seconds === 1 ? "1 second" : `${seconds} seconds`;
}

export function pairingExchangeFailure(input: { status: number; body: string; controllerOrigin: string; retryAfter?: string | null }): PairingFailureCopy {
  const detail = hubSentence(input.body);
  const remint = `Run cas hub pair --origin ${input.controllerOrigin} on the machine and open the link it prints.`;
  if (input.status === 429) {
    return {
      message: `The machine is limiting pairing attempts. Wait ${retryAfterCopy(input.retryAfter)} and tap Pair again. If this invitation was already used, you will need a fresh invitation.`,
      keepInvitation: true,
    };
  }
  if (input.status === 404) {
    return {
      message: "No Cassy hub answered at that address. Check it is the machine's hub address, not this page's, then tap Pair again; this invitation is still open.",
      keepInvitation: true,
    };
  }
  if (input.status >= 500) {
    return {
      message: `${detail ?? `The machine's hub answered with an error (${input.status}).`} Wait a few seconds and tap Pair again. If the machine already handled this invitation, you will need a fresh one.`,
      keepInvitation: true,
    };
  }
  if (input.status === 401 || input.status === 403) {
    return {
      message: detail
        ? `${detail} ${remint}`
        : `The machine refused this pairing link. A link pairs once, expires ten minutes after it is printed, and only works for the Cassy Commander address it was minted for. ${remint}`,
      keepInvitation: false,
    };
  }
  return {
    message: `${detail ?? `The machine rejected this pairing exchange (${input.status}).`} ${remint}`,
    keepInvitation: false,
  };
}

/** A fetch rejection cannot distinguish no delivery from a consumed POST whose response was lost. */
export function unreachableHubMessage(hubUrl: string): string {
  return `We could not confirm pairing with ${hubUrl}. Check that Tailscale (VPN) is connected here and that the machine is awake, then tap Pair again. If the machine already used this invitation, you will need a fresh invitation.`;
}
