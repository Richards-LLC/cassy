import type { PairingDraft } from "./pairing-draft";
import type { PairingOperation, PairingOperationCoordinator } from "./pairing-operation";
import type { PendingPairing } from "./pending-pairing";

export interface PairingCleanupState {
  pendingPairing: PendingPairing | null;
  pairingDraft: PairingDraft;
  exchangeInFlight: boolean;
  status: string;
}

/**
 * Produce the visible cleanup failure state only for the operation that still
 * owns the pending pairing. A rejected rollback can settle after a replacement
 * flow has begun, and must not erase that newer request or its draft.
 */
export function pairingCleanupFailureUpdate(options: {
  coordinator: PairingOperationCoordinator;
  operation: PairingOperation;
  expectedPending: PendingPairing;
  current: PairingCleanupState;
  cleanupMessage: string;
  resetDraft: () => PairingDraft;
}): PairingCleanupState | null {
  if (!options.coordinator.isCurrent(options.operation) || options.current.pendingPairing !== options.expectedPending) return null;
  return {
    pendingPairing: null,
    pairingDraft: options.resetDraft(),
    exchangeInFlight: false,
    status: options.cleanupMessage,
  };
}
