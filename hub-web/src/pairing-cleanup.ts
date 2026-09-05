import type { PairingDraft } from "./pairing-draft";
import type { PairingOperation, PairingOperationCoordinator } from "./pairing-operation";
import type { PendingPairing, PendingPairingClearResult } from "./pending-pairing";

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

/** Preserve the exchange failure while making reload safety explicit. */
export function pairingStorageClearFailureMessage(message: string, cleared: PendingPairingClearResult): string {
  return cleared.failClosed
    ? message
    : `${message} Browser storage could not durably block this pairing request; keep this page open and retry after storage access is restored.`;
}

export interface CancellationOutcome {
  /** Durable cleanup did not happen; the dialog stays open with a retry (F2). */
  readonly cleanupFailed: boolean;
  /** The exchange is still unwinding; the dialog stays open until it reports. */
  readonly verifying: boolean;
  readonly status: string;
}

/**
 * Cancel is discard: the invitation is gone from this page either way. What
 * differs is whether the page can prove a reload will not resurrect it, and
 * that answer has to be visible in the dialog the operator is looking at, not
 * in a status sentence behind a dialog that just closed.
 */
export function cancellationOutcome(cleared: PendingPairingClearResult, exchangeInFlight: boolean): CancellationOutcome {
  if (!cleared.failClosed) {
    return {
      cleanupFailed: true,
      verifying: false,
      status: "Browser storage refused to record the cancellation, so a reload could still see the discarded invitation. Keep this page open and retry the cleanup once storage access is restored.",
    };
  }
  if (exchangeInFlight) {
    return { cleanupFailed: false, verifying: true, status: "Cancelling pairing and verifying local cleanup…" };
  }
  return {
    cleanupFailed: false,
    verifying: false,
    status: cleared.persistentRemovalFailed
      ? "Pairing cancelled. Browser storage removal was denied, so the request was durably blocked instead."
      : "Pairing cancelled.",
  };
}

/** Nonsecret feedback for a link or stored invitation that cannot be used (F6). */
export const INVALID_PAIRING_LINK_MESSAGE = "This pairing link is invalid or incomplete, so nothing was paired. Create a pairing code here, or open a fresh link printed by cas hub pair on the machine.";
export const EXPIRED_PAIRING_INVITATION_MESSAGE = "This pairing invitation expired before it was used. Create a pairing code here, or open a fresh link printed by cas hub pair on the machine.";

/**
 * What the "cleanup incomplete" step has to say, and only what is true of
 * this cleanup. The step is reached by Cancel and by a failed exchange, and
 * either the session store, the catalog rollback, or both can be outstanding;
 * the earlier copy always blamed the session store.
 */
export interface CleanupStepContext {
  readonly cause: "cancel" | "failure";
  /** The pending-pairing store could not be fail-closed. */
  readonly storeOpen: boolean;
  /** A staged/cancelled credential is still waiting for its catalog rollback. */
  readonly rollbackPending: boolean;
}

export interface CleanupStepCopy {
  readonly title: string;
  readonly discarded: string;
  readonly outstanding: string;
  readonly next: string;
}

export function cleanupStepCopy(context: CleanupStepContext): CleanupStepCopy {
  const outstanding = [
    ...(context.storeOpen ? ["Browser storage refused to record the cancellation, so a reload could still see the discarded invitation."] : []),
    ...(context.rollbackPending ? ["The credential this browser started saving is blocked and invisible, but removing it from browser storage has not completed."] : []),
  ];
  return {
    title: context.cause === "cancel" ? "Could not finish cancelling" : "Pairing failed and cleanup is incomplete",
    discarded: context.cause === "cancel"
      ? "Pairing was cancelled on this page and the discarded invitation cannot be resumed here."
      : "Pairing did not complete and its invitation was discarded; it cannot be resumed here.",
    outstanding: outstanding.length ? outstanding.join(" ") : "The cleanup this page owes has not completed.",
    next: "Keep this page open and retry once browser storage is available. This blocks this browser only; copies of the link elsewhere remain subject to the machine's own expiry.",
  };
}
