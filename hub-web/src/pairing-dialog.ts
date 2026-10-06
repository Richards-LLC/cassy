export interface PairingDialogState {
  createInFlight: boolean;
  exchangeInFlight: boolean;
  hasPendingPairing: boolean;
}

export function pairingDialogCancellationActive(state: PairingDialogState): boolean {
  return state.createInFlight || state.exchangeInFlight || state.hasPendingPairing;
}

/** Route the native HTMLDialog Escape path through the pairing cancellation policy. */
export function bindPairingDialogCancel(
  dialog: HTMLDialogElement,
  state: () => PairingDialogState,
  cancel: () => void,
): void {
  dialog.addEventListener("cancel", (event) => {
    if (!pairingDialogCancellationActive(state())) return;
    event.preventDefault();
    cancel();
  });
}

/** A failed attempt owns its visible advice, rather than reopening a field. */
export function focusPairingFeedback(dialog: HTMLDialogElement | null): void {
  const feedback = dialog?.querySelector<HTMLElement>(".pair-status");
  if (!dialog?.open || !feedback || feedback.hidden || !feedback.textContent?.trim()) return;
  feedback.setAttribute("role", "alert");
  feedback.focus({ preventScroll: true });
  // Centering clears the sticky action row on short screens; focus alone can
  // put the last line under Pair. No animation delays the failure feedback.
  feedback.scrollIntoView({ block: "center", inline: "nearest", behavior: "instant" });
}
