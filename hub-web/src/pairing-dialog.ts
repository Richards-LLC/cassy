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
