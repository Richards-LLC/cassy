import { describe, expect, it, vi } from "vitest";
import { HubConnectionSupervisor, type HubCallbacks } from "./connection";
import type { StoredMachine } from "./types";

describe("Commander MessageQueued callback", () => {
  it("relays the daemon acknowledgment with its client reference", () => {
    const onMessageQueued = vi.fn();
    const callbacks = { onMessageQueued } as unknown as HubCallbacks;
    const supervisor = new HubConnectionSupervisor({} as StoredMachine, callbacks);
    const internals = supervisor as unknown as {
      handleDaemonObject(session: string, message: Record<string, unknown>): void;
    };

    internals.handleDaemonObject("factory-a", {
      MessageQueued: {
        client_ref: "send-42",
        notification_id: 812,
        target: "patient-pelican-9",
        stamped: true,
      },
    });

    expect(onMessageQueued).toHaveBeenCalledWith("factory-a", {
      client_ref: "send-42",
      notification_id: 812,
      target: "patient-pelican-9",
      stamped: true,
    });
  });
});
