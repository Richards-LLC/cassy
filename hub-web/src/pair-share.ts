/**
 * Send a pairing code to whoever runs the machine (cas-a56c). The person holding
 * the phone is often not the one at the machine's terminal: the code has to
 * reach them. A phone's share sheet (Messages, WhatsApp, email) is the natural
 * way; where there is none, the text goes to the clipboard to paste anywhere.
 */
export interface ShareTarget {
  share?: (data: { text: string }) => Promise<void>;
  clipboard?: { writeText: (text: string) => Promise<void> };
}

export type ShareOutcome = "shared" | "copied" | "cancelled" | "failed";

export async function sendPairingCode(target: ShareTarget, text: string): Promise<ShareOutcome> {
  if (typeof target.share === "function") {
    try {
      await target.share({ text });
      return "shared";
    } catch (error) {
      // Closing the share sheet is a choice, not a failure.
      if ((error as { name?: string } | null)?.name === "AbortError") return "cancelled";
    }
  }
  try {
    if (!target.clipboard) return "failed";
    await target.clipboard.writeText(text);
    return "copied";
  } catch {
    return "failed";
  }
}

/** What is sent: the code, the command that approves it, and how long it lasts. */
export function pairingShareText(userCode: string): string {
  return `Please approve my Cassy Commander pairing. On your machine, run: cas hub authorize ${userCode} (the code works for 10 minutes).`;
}
