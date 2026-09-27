/**
 * Everything a hub heartbeat changes, written into nodes that already exist.
 *
 * These are the values that used to arrive by way of `app.innerHTML`, taking
 * the composer, the dialogs and every focus state with them. Nothing here
 * creates or replaces a node in the shell: a five-second status frame must be
 * unable to close a phone keyboard.
 */

export interface LiveRegionView {
  readonly connection?: {
    readonly state: string;
    readonly title: string;
    readonly latencyText: string;
  };
  /** hidden: the session is not live, so there is no control to claim (cas-edcd, cas-4a93). */
  readonly mode?: { readonly badge: string; readonly compact: string; readonly hidden?: boolean };
  readonly controlAction?: { readonly label: string; readonly disabledReason?: string };
  readonly interruptReason?: string;
  /** The visible line under the header naming why its controls are unavailable. */
  readonly controlsNotice?: string;
  /** Full sentence, or undefined when the hub is live. */
  readonly staleNotice?: string;
  readonly controlReason?: string;
  readonly sendReason?: string;
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

/**
 * The on-screen reason for the header's unavailable controls (journey F9):
 * each distinct reason once, so an outage that disables both Release control
 * and Interrupt reads as one sentence. Undefined when every control is usable.
 */
export function sessionControlsNotice(...reasons: readonly (string | undefined)[]): string | undefined {
  const distinct = [...new Set(reasons.filter((reason): reason is string => Boolean(reason?.trim())))];
  return distinct.length ? distinct.join(" ") : undefined;
}

export function applyLiveRegions(root: ParentNode, view: LiveRegionView): void {
  const summary = root.querySelector<HTMLElement>(".connection-summary");
  if (summary && view.connection) {
    summary.className = `connection-summary ${view.connection.state}`;
    summary.title = view.connection.title;
    const latency = summary.querySelector<HTMLElement>("[data-machine-latency]");
    if (latency) latency.textContent = view.connection.latencyText;
  }

  const mode = root.querySelector<HTMLElement>(".mode-badge");
  if (mode && view.mode) {
    mode.className = `mode-badge ${view.mode.badge.toLowerCase()}`;
    mode.dataset.compactLabel = view.mode.compact;
    mode.textContent = view.mode.badge;
    mode.hidden = view.mode.hidden === true;
  }

  const lease = root.querySelector<HTMLButtonElement>("#lease");
  if (lease && view.controlAction) {
    lease.textContent = view.controlAction.label;
    lease.setAttribute("aria-label", view.controlAction.label);
    setDisabledReason(lease, view.controlAction.disabledReason);
    const wrapper = lease.closest<HTMLElement>(".control-action");
    if (wrapper) wrapper.title = view.controlAction.disabledReason ?? view.controlAction.label;
    const described = root.querySelector<HTMLElement>("#control-disabled-reason");
    setNotice(described, view.controlAction.disabledReason);
  }

  const interrupt = root.querySelector<HTMLButtonElement>("#interrupt");
  if (interrupt) {
    interrupt.title = view.interruptReason ?? "Interrupt selected pane";
    setDisabledReason(interrupt, view.interruptReason);
    // cas-71af: the reason under the header is its description, so a click
    // (or a screen reader) finds it there instead of in a repeating toast.
    if (view.interruptReason) interrupt.setAttribute("aria-describedby", "session-controls-reason");
    else interrupt.removeAttribute("aria-describedby");
  }
  setNotice(root.querySelector<HTMLElement>("#session-controls-reason"), view.controlsNotice);

  setNotice(root.querySelector<HTMLElement>(".status-stale"), view.staleNotice);
  setNotice(root.querySelector<HTMLElement>(".control-disabled-reason"), view.controlReason);
  setDisabledReason(root.querySelector<HTMLElement>("#message-send"), view.sendReason);

  const status = root.querySelector<HTMLElement>("#message-status");
  if (status) {
    status.textContent = view.messageStatus?.text ?? "";
    status.className = `message-status${view.messageStatus?.error ? " error" : ""}`;
    status.hidden = view.messageStatus === undefined;
  }

  setNotice(root.querySelector<HTMLElement>("#message-delivery"), view.delivery);

  const pairing = view.pairing;
  if (pairing) {
    setNotice(root.querySelector<HTMLElement>("#pair-dialog .pair-status"), pairing.status);
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
