import { describe, expect, it, vi } from "vitest";
import { replaceMachineConnection } from "./connection-lifecycle";
import type { ConnectionState } from "./connection";
import type { StoredMachine } from "./types";

function storedMachine(credentialId: string): StoredMachine {
  return {
    id: "stable-hub-id",
    label: "Studio Mac",
    baseUrl: "https://workstation.tail.example",
    deviceId: `device-${credentialId}`,
    credentialId,
    credential: `opaque-${credentialId}`,
    expiresAt: "2027-01-01T00:00:00Z",
    scopes: ["machine-read"],
    publicKey: { kty: "EC" },
    privateKey: {} as CryptoKey,
  };
}

describe("Commander live connection lifecycle", () => {
  it("stops and replaces an existing same-hub supervisor with the newly paired credential", () => {
    const prior = storedMachine("revoked");
    const replacement = storedMachine("replacement");
    const events: string[] = [];
    const oldSupervisor = {
      machine: prior,
      start: vi.fn(),
      stop: vi.fn(() => { events.push("old:stop"); }),
    };
    const connections = new Map([[prior.id, oldSupervisor]]);
    const connectionStates = new Map<string, ConnectionState>([[prior.id, {
      phase: "failed", stage: "auth", since: 0, attempt: 0,
      missedHeartbeats: 0, degraded: false, authFailure: "revoked",
    }]]);
    const createConnection = vi.fn((machine: StoredMachine) => ({
      machine,
      start: vi.fn(() => { events.push(`new:start:${machine.credentialId}`); }),
      stop: vi.fn(),
    }));

    const installed = replaceMachineConnection(replacement, connections, connectionStates, createConnection);

    expect(oldSupervisor.stop).toHaveBeenCalledOnce();
    expect(connections.get(replacement.id)).toBe(installed);
    expect(connections.get(replacement.id)).not.toBe(oldSupervisor);
    expect(connectionStates.has(replacement.id)).toBe(false);
    expect(installed.machine.credentialId).toBe("replacement");
    expect(installed.machine.credential).toBe("opaque-replacement");
    expect(installed.start).toHaveBeenCalledOnce();
    expect(events).toEqual(["old:stop", "new:start:replacement"]);
  });
});
