import type { PendingPairingClearResult } from "./pending-pairing";

/**
 * Who owns the "Could not finish cancelling" step.
 *
 * Cancel invalidates the pairing operation, so anything that settles late for
 * that operation — a rejected rollback, a slow catalog write — arrives with
 * `isCurrent() === false`. Dropping it there is what left the dialog on
 * "verifying" for good (cas-7d55 F2). The cancellation itself is the identity
 * those late results belong to: it stays current until the operator starts
 * something newer (a fresh code, a new link, a relay approval, a successful
 * pair), and only while it is current may a late result touch the dialog.
 */
export interface PairingCancellation {
  /** Monotonic; a newer cancellation or a replacement flow makes an older one stale. */
  readonly id: number;
  /** The operation generation Cancel invalidated, if an exchange was in flight. */
  readonly operationGeneration: number | undefined;
}

export interface CleanupRetryTicket {
  readonly cancellation: PairingCancellation;
}

export class PairingCancellationTracker {
  private serial = 0;
  private current: PairingCancellation | undefined;
  private retry: CleanupRetryTicket | undefined;

  /** Cancel pressed. Returns the identity late results must present. */
  begin(operationGeneration: number | undefined): PairingCancellation {
    this.serial += 1;
    this.current = { id: this.serial, operationGeneration };
    this.retry = undefined;
    return this.current;
  }

  /** A replacement flow started: no earlier cancellation may touch the dialog again. */
  supersede(): void {
    this.current = undefined;
    this.retry = undefined;
  }

  /** Whether `cancellation` still owns the dialog. */
  isCurrent(cancellation: PairingCancellation | undefined): boolean {
    return cancellation !== undefined && this.current?.id === cancellation.id;
  }

  /** A late result from the operation Cancel invalidated, and nothing newer since. */
  ownsOperation(operationGeneration: number): boolean {
    return this.current !== undefined && this.current.operationGeneration === operationGeneration;
  }

  get active(): PairingCancellation | undefined {
    return this.current;
  }

  get retrying(): boolean {
    return this.retry !== undefined;
  }

  /** One retry at a time, and only for the current cancellation. */
  beginRetry(): CleanupRetryTicket | undefined {
    if (!this.current || this.retry) return undefined;
    this.retry = { cancellation: this.current };
    return this.retry;
  }

  /** Releases the busy guard; the ticket's result is applied only if still current. */
  finishRetry(ticket: CleanupRetryTicket): boolean {
    if (this.retry === ticket) this.retry = undefined;
    return this.isCurrent(ticket.cancellation);
  }
}

export interface CleanupRecovery {
  /** `catalog.recoverPending()` resolved with this many staged rows still blocked. */
  readonly pendingCleanup?: number;
  /** `catalog.recoverPending()` rejected. */
  readonly failed?: boolean;
}

export interface CleanupRetryOutcome {
  readonly done: boolean;
  readonly status: string;
}

/** What one Retry cleanup attempt proved, in words the dialog can show. */
export function cleanupRetryOutcome(cleared: PendingPairingClearResult, recovery: CleanupRecovery): CleanupRetryOutcome {
  if (recovery.failed) {
    return { done: false, status: "Browser storage could not be checked. Keep this page open and retry once storage access is restored." };
  }
  if (!cleared.failClosed) {
    return { done: false, status: "Browser storage still refuses to record the cancellation. Keep this page open and retry once storage access is restored." };
  }
  if ((recovery.pendingCleanup ?? 0) > 0) {
    return { done: false, status: "The cancelled invitation is blocked, but a cancelled credential is still waiting for cleanup. Keep this page open and retry." };
  }
  return {
    done: true,
    status: cleared.persistentRemovalFailed
      ? "Pairing cancelled. Browser storage removal was denied, so the request was durably blocked instead."
      : "Pairing cancelled.",
  };
}

/** The sentence for a rollback that rejected after Cancel. */
export const LATE_ROLLBACK_FAILURE_MESSAGE = "Pairing was cancelled, but the cancelled credential could not be cleaned up yet. It stays blocked and invisible; keep this page open and retry the cleanup.";
