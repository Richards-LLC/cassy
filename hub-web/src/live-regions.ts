import { fitComposerPlaceholder } from "./conversation-shell";

/**
 * Everything a hub heartbeat changes, written into nodes that already exist.
 *
 * These are the values that used to arrive by way of `app.innerHTML`, taking
 * the composer, the dialogs and every focus state with them. Nothing here
 * creates or replaces a node in the shell: a five-second status frame must be
 * unable to close a phone keyboard.
 */

export interface LiveRegionView {
  /** Full sentence, or undefined when the hub is live. */
  readonly staleNotice?: string;
  readonly sendReason?: string;
  /**
   * Why this device can never send here (a read-only pairing, cas-b7c0). The
   * composer is read-only and the sentence stands under it before anything is
   * typed, instead of appearing only after a tap on Send.
   */
  readonly readOnlyReason?: string;
  readonly messageStatus?: { readonly text: string; readonly error: boolean };
  readonly delivery?: string;
  /**
   * The pairing dialog's status sentence and busy state, written into the
   * dialog that is already open. The form's nodes — and the caret in them —
   * survive an exchange failure, so Pair is usable again the moment the error
   * lands instead of after the next blur (F1).
   */
  readonly pairing?: {
    readonly status?: string;
    readonly exchangeInFlight: boolean;
    readonly createInFlight: boolean;
    /** A Retry cleanup is running; the button waits for it. */
    readonly cleanupRetryInFlight?: boolean;
  };
}

export const READ_ONLY_PLACEHOLDER = "Read only on this browser";

/** A reason that is absent must clear the attributes, not leave a stale one. */
function setDisabledReason(element: HTMLElement | null, reason: string | undefined): void {
  if (!element) return;
  if (reason === undefined) {
    element.removeAttribute("aria-disabled");
    element.removeAttribute("data-disabled-reason");
    return;
  }
  element.setAttribute("aria-disabled", "true");
  element.setAttribute("data-disabled-reason", reason);
}

function setNotice(element: HTMLElement | null, text: string | undefined): void {
  if (!element) return;
  element.textContent = text ?? "";
  element.hidden = text === undefined;
}

export function applyLiveRegions(root: ParentNode, view: LiveRegionView): void {
  setNotice(root.querySelector<HTMLElement>(".status-stale"), view.staleNotice);
  const send = root.querySelector<HTMLElement>("#message-send");
  setDisabledReason(send, view.sendReason);
  if (view.sendReason !== undefined) send?.setAttribute("aria-describedby", "message-status");
  else send?.removeAttribute("aria-describedby");

  const field = root.querySelector<HTMLTextAreaElement>("#message-text");
  if (field) {
    const readOnly = view.readOnlyReason !== undefined;
    // cas-a56c (QA F01): a read-only field does not invite a message; writable
    // again, it takes back its fitted wording.
    const wasReadOnly = field.readOnly;
    field.readOnly = readOnly;
    if (readOnly && field.placeholder !== READ_ONLY_PLACEHOLDER) field.placeholder = READ_ONLY_PLACEHOLDER;
    if (!readOnly && wasReadOnly) fitComposerPlaceholder(field);
    const composer = field.closest<HTMLElement>(".message");
    if (composer) {
      if (view.readOnlyReason === undefined) delete composer.dataset.readOnly;
      else composer.dataset.readOnly = "true";
    }
  }

  const status = root.querySelector<HTMLElement>("#message-status");
  if (status) {
    // The current refusal must stay visible and describe Send, even when a
    // previous send left a status. A pairing fact has no error tone.
    const reason = view.sendReason ?? view.readOnlyReason;
    const shown = reason === undefined ? view.messageStatus : { text: reason, error: false };
    status.textContent = shown?.text ?? "";
    status.className = `message-status${shown?.error ? " error" : ""}`;
    status.hidden = shown === undefined;
  }

  setNotice(root.querySelector<HTMLElement>("#message-delivery"), view.delivery);

  const pairing = view.pairing;
  if (pairing) {
    const feedback = root.querySelector<HTMLElement>("#pair-dialog .pair-status");
    // A failure is assertive; progress on its next attempt is polite. Reset
    // before changing the text, including updates that retain the same node.
    if (pairing.exchangeInFlight || pairing.createInFlight || pairing.cleanupRetryInFlight || pairing.status === undefined) {
      feedback?.setAttribute("role", "status");
    }
    setNotice(feedback, pairing.status);
    const form = root.querySelector<HTMLFormElement>("#pair-form");
    if (form) form.setAttribute("aria-busy", String(pairing.exchangeInFlight));
    const submit = root.querySelector<HTMLButtonElement>('#pair-form button[type="submit"]');
    if (submit) {
      submit.disabled = pairing.exchangeInFlight;
      submit.textContent = pairing.exchangeInFlight ? "Pairing…" : "Pair";
    }
    const create = root.querySelector<HTMLButtonElement>("#pair-create");
    if (create) {
      create.disabled = pairing.createInFlight;
      create.textContent = pairing.createInFlight ? "Creating…" : "Create pairing code";
    }
    // Close becomes Cancel while a code is being minted: the handler reads the
    // flag at click time, so only the label has to move.
    const close = root.querySelector<HTMLButtonElement>("#pair-close");
    if (close && close.dataset.role !== "cleanup") close.textContent = pairing.createInFlight ? "Cancel" : "Close";
    const retry = root.querySelector<HTMLButtonElement>("#pair-cleanup-retry");
    if (retry) {
      retry.disabled = pairing.cleanupRetryInFlight === true;
      retry.textContent = pairing.cleanupRetryInFlight ? "Retrying…" : "Retry cleanup";
    }
  }
}
