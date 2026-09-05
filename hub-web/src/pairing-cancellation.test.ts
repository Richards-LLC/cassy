import { describe, expect, it, vi } from "vitest";
import { LATE_ROLLBACK_FAILURE_MESSAGE, PairingCancellationTracker, cleanupRetryOutcome } from "./pairing-cancellation";
import { cleanupStepCopy } from "./pairing-cleanup";
import { exchangePendingPairing, PairingCleanupError } from "./pairing-exchange";
import { PairingOperationCoordinator } from "./pairing-operation";

const credential = { device_id: "device", credential_id: "credential", credential: "opaque", expires_at: "2030-01-01T00:00:00Z", scopes: ["machine-read"] };
const response = (status: number, body: unknown) => new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
const invitation = { kind: "invitation" as const, token: "A".repeat(43), hubId: "machine-uuid", hubUrl: "https://workstation.tail.example", controllerOrigin: "https://commander.example", scopes: ["machine-read"] as const };

/**
 * The main.ts catch, reduced to the decision under test: a PairingCleanupError
 * from an operation Cancel invalidated is shown only while that cancellation
 * still owns the dialog.
 */
function lateCleanupDecision(tracker: PairingCancellationTracker, coordinator: PairingOperationCoordinator, operation: { generation: number; signal: AbortSignal }): "show" | "drop" {
  if (coordinator.isCurrent(operation as never)) return "show";
  return tracker.ownsOperation(operation.generation) ? "show" : "drop";
}

describe("F2: cancel while staged, then the rollback rejects", () => {
  it("still owns the late PairingCleanupError and shows the cleanup step", async () => {
    const coordinator = new PairingOperationCoordinator();
    const tracker = new PairingCancellationTracker();
    const operation = coordinator.begin();
    const cancel = () => { tracker.begin(operation.generation); coordinator.invalidate(); };
    const attempt = exchangePendingPairing({
      invitation, controllerOrigin: invitation.controllerOrigin, deviceLabel: "Phone", operatorLabel: "Operator",
      fetcher: async () => response(200, credential),
      createKey: async () => ({ privateKey: {} as CryptoKey, publicKey: { kty: "EC" } }),
      installationGeneration: operation.generation,
      stagePersisted: async () => undefined,
      // Cancel lands while the credential is being activated; the rollback then rejects.
      activatePersisted: async () => { cancel(); throw new DOMException("aborted", "AbortError"); },
      rollbackPersisted: async () => { throw new Error("durable cleanup rejected"); },
      signal: operation.signal,
      isCurrent: () => coordinator.isCurrent(operation),
    });
    await expect(attempt).rejects.toBeInstanceOf(PairingCleanupError);
    // This is exactly the state c35f3ad3 dropped: the operation is no longer
    // current, but the cancellation that ended it is.
    expect(coordinator.isCurrent(operation)).toBe(false);
    expect(lateCleanupDecision(tracker, coordinator, operation)).toBe("show");
    expect(LATE_ROLLBACK_FAILURE_MESSAGE).toContain("retry the cleanup");
    expect(LATE_ROLLBACK_FAILURE_MESSAGE).not.toContain("resume");
  });

  it("drops the late failure once a replacement flow owns the dialog", () => {
    const coordinator = new PairingOperationCoordinator();
    const tracker = new PairingCancellationTracker();
    const cancelled = coordinator.begin();
    tracker.begin(cancelled.generation);
    coordinator.invalidate();
    // A fresh code or a new link: nothing from the old cancellation may land.
    tracker.supersede();
    coordinator.replace();
    expect(lateCleanupDecision(tracker, coordinator, cancelled)).toBe("drop");
  });

  it("drops a late failure from an older cancellation when a newer cancellation exists", () => {
    const tracker = new PairingCancellationTracker();
    tracker.begin(1);
    tracker.begin(2);
    expect(tracker.ownsOperation(1)).toBe(false);
    expect(tracker.ownsOperation(2)).toBe(true);
  });
});

describe("F2: Retry cleanup is serialized, guarded and reports rejection", () => {
  it("runs one retry at a time", () => {
    const tracker = new PairingCancellationTracker();
    tracker.begin(undefined);
    const first = tracker.beginRetry();
    expect(first).toBeDefined();
    expect(tracker.beginRetry()).toBeUndefined();
    expect(tracker.retrying).toBe(true);
    expect(tracker.finishRetry(first!)).toBe(true);
    expect(tracker.retrying).toBe(false);
    expect(tracker.beginRetry()).toBeDefined();
  });

  it("refuses to retry when no cancellation owns the dialog", () => {
    const tracker = new PairingCancellationTracker();
    expect(tracker.beginRetry()).toBeUndefined();
    tracker.begin(undefined);
    tracker.supersede();
    expect(tracker.beginRetry()).toBeUndefined();
  });

  it("does not apply a retry that settles after a new invitation arrived", async () => {
    const tracker = new PairingCancellationTracker();
    tracker.begin(undefined);
    const ticket = tracker.beginRetry()!;
    const recover = vi.fn(async () => { await new Promise((resolve) => setTimeout(resolve, 5)); return { pendingCleanup: 0 }; });
    const pending = recover();
    // The operator opened a fresh link while the retry was in flight.
    tracker.supersede();
    await pending;
    expect(tracker.finishRetry(ticket)).toBe(false);
    // …and the newer flow may now retry on its own terms.
    tracker.begin(undefined);
    expect(tracker.beginRetry()).toBeDefined();
  });

  it("turns a rejected recovery into a visible, retryable status", () => {
    const rejected = cleanupRetryOutcome({ persistentRemovalFailed: false, failClosed: true }, { failed: true });
    expect(rejected.done).toBe(false);
    expect(rejected.status).toContain("could not be checked");
    const stillStaged = cleanupRetryOutcome({ persistentRemovalFailed: false, failClosed: true }, { pendingCleanup: 1 });
    expect(stillStaged.done).toBe(false);
    expect(stillStaged.status).toContain("still waiting for cleanup");
    const storeStillOpen = cleanupRetryOutcome({ persistentRemovalFailed: true, failClosed: false }, { pendingCleanup: 0 });
    expect(storeStillOpen.done).toBe(false);
    expect(cleanupRetryOutcome({ persistentRemovalFailed: false, failClosed: true }, { pendingCleanup: 0 })).toEqual({ done: true, status: "Pairing cancelled." });
    expect(cleanupRetryOutcome({ persistentRemovalFailed: true, failClosed: true }, { pendingCleanup: 0 }).status).toContain("durably blocked");
  });
});

describe("F2: a failed exchange whose rollback rejected owns a retry too (review 25564)", () => {
  it("reaches the cleanup step with the operation still current, and begin() gives Retry an owner", async () => {
    const coordinator = new PairingOperationCoordinator();
    const tracker = new PairingCancellationTracker();
    const operation = coordinator.begin();
    const attempt = exchangePendingPairing({
      invitation, controllerOrigin: invitation.controllerOrigin, deviceLabel: "Phone", operatorLabel: "Operator",
      fetcher: async () => response(200, credential),
      createKey: async () => ({ privateKey: {} as CryptoKey, publicKey: { kty: "EC" } }),
      installationGeneration: operation.generation,
      stagePersisted: async () => undefined,
      activatePersisted: async () => { throw new DOMException("Fixture storage denied", "QuotaExceededError"); },
      rollbackPersisted: async () => { throw new Error("durable cleanup rejected"); },
      signal: operation.signal,
      isCurrent: () => coordinator.isCurrent(operation),
    });
    await expect(attempt).rejects.toBeInstanceOf(PairingCleanupError);
    // Nobody cancelled: the operation is current, so the old code rendered the
    // step without an owner and beginRetry() returned undefined.
    expect(coordinator.isCurrent(operation)).toBe(true);
    expect(tracker.beginRetry()).toBeUndefined();
    tracker.begin(operation.generation);
    const ticket = tracker.beginRetry();
    expect(ticket).toBeDefined();
    // Storage restored: the retry completes and closes the step.
    expect(tracker.finishRetry(ticket!)).toBe(true);
    expect(cleanupRetryOutcome({ persistentRemovalFailed: false, failClosed: true }, { pendingCleanup: 0 }).done).toBe(true);
  });

  it("says only what is outstanding, for either way into the step", () => {
    const cancelStore = cleanupStepCopy({ cause: "cancel", storeOpen: true, rollbackPending: false });
    expect(cancelStore.title).toBe("Could not finish cancelling");
    expect(cancelStore.outstanding).toContain("reload could still see");
    expect(cancelStore.outstanding).not.toContain("credential this browser started saving");

    const cancelRollback = cleanupStepCopy({ cause: "cancel", storeOpen: false, rollbackPending: true });
    expect(cancelRollback.outstanding).not.toContain("reload could still see");
    expect(cancelRollback.outstanding).toContain("blocked and invisible");

    const failure = cleanupStepCopy({ cause: "failure", storeOpen: false, rollbackPending: true });
    expect(failure.title).toBe("Pairing failed and cleanup is incomplete");
    expect(failure.discarded).toContain("did not complete");
    expect(failure.discarded).not.toContain("cancelled on this page");

    const both = cleanupStepCopy({ cause: "cancel", storeOpen: true, rollbackPending: true });
    expect(both.outstanding).toContain("reload could still see");
    expect(both.outstanding).toContain("blocked and invisible");
    for (const copy of [cancelStore, cancelRollback, failure, both]) {
      expect(copy.next).toContain("blocks this browser only");
      expect(`${copy.discarded} ${copy.outstanding}`).not.toContain("resume ");
    }
  });
});
