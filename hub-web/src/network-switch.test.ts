import { afterEach, describe, expect, it, vi } from "vitest";
import { HubConnectionSupervisor, type HubCallbacks } from "./connection";
import { ConversationHistory } from "./conversation-history";
import { MACHINE_RETRY_CEILING_MS, SOCKET_PROBE_TIMEOUT_MS } from "./connection-state";
import type { StoredMachine } from "./types";

// cas-0978: surviving network switches. The journey (HUB-J12) proves the
// behaviour end to end; these pin the pieces it rests on.

type Internals = {
  machineSocket?: unknown;
  machineSocketReady: boolean;
  machineMultiplex: boolean;
  desiredSessions: Set<string>;
  sockets: Map<string, unknown>;
  lifecycle: { phase: string; stage: string; fatal?: boolean; authFailure?: string };
  retryTimer?: number;
  attempt: number;
  connectionLost: boolean;
  abandonSockets(reason: string): void;
  networkChanged(): void;
  connect(): Promise<void>;
  openMachineSocket(session: string): Promise<boolean>;
  request(...args: unknown[]): Promise<unknown>;
};

function fakeSocket() {
  return { readyState: 1, close: vi.fn(), send: vi.fn(), onopen: () => {}, onmessage: () => {}, onerror: () => {}, onclose: () => {} };
}

function supervisor(callbacks: Partial<HubCallbacks> = {}): { sup: HubConnectionSupervisor; internals: Internals } {
  const sup = new HubConnectionSupervisor({ baseUrl: "https://atlas.test" } as StoredMachine, { onState: vi.fn(), ...callbacks } as unknown as HubCallbacks);
  return { sup, internals: sup as unknown as Internals };
}

afterEach(() => { vi.unstubAllGlobals(); vi.useRealTimers(); });

describe("abandoning half-open sockets (cas-0978)", () => {
  it("detaches and drops every socket at once instead of waiting for a close that may never come", () => {
    vi.stubGlobal("window", globalThis);
    const onAttachState = vi.fn();
    const { internals } = supervisor({ onAttachState });
    const machine = fakeSocket();
    const legacy = fakeSocket();
    internals.machineSocket = machine;
    internals.machineSocketReady = true;
    internals.sockets.set("legacy-session", legacy);
    internals.desiredSessions.add("patient-pelican-9");

    internals.abandonSockets("Lost connection to the machine");

    expect(internals.machineSocket).toBeUndefined();
    expect(internals.machineSocketReady).toBe(false);
    expect(internals.sockets.size).toBe(0);
    for (const socket of [machine, legacy]) {
      expect(socket.close).toHaveBeenCalled();
      expect(socket.onclose).toBeNull();
      expect(socket.onmessage).toBeNull();
    }
    expect(onAttachState).toHaveBeenCalledWith("patient-pelican-9", expect.objectContaining({ phase: "failed", reason: "Lost connection to the machine" }));
  });
});

describe("network hints (cas-0978)", () => {
  it("retries at once, from a fresh schedule, when the network comes back during a backoff", () => {
    vi.stubGlobal("window", globalThis);
    const { internals } = supervisor();
    (internals as unknown as { desired: boolean }).desired = true;
    internals.lifecycle = { phase: "backoff", stage: "dialing" };
    internals.attempt = 5;
    internals.retryTimer = setTimeout(() => {}, 60_000) as unknown as number;
    const connect = vi.spyOn(internals, "connect").mockResolvedValue();

    internals.networkChanged();

    expect(connect).toHaveBeenCalledTimes(1);
    expect(internals.attempt).toBe(0);
    expect(internals.retryTimer).toBeUndefined();
  });

  it("does not retry a refused pairing or an unsupported browser", () => {
    vi.stubGlobal("window", globalThis);
    const { internals } = supervisor();
    (internals as unknown as { desired: boolean }).desired = true;
    const connect = vi.spyOn(internals, "connect").mockResolvedValue();
    internals.lifecycle = { phase: "failed", stage: "auth", authFailure: "revoked" };
    internals.networkChanged();
    internals.lifecycle = { phase: "failed", stage: "dialing", fatal: true };
    internals.networkChanged();
    expect(connect).not.toHaveBeenCalled();
  });

  it("keeps the machine protocol when the socket ticket is lost to the network", async () => {
    vi.stubGlobal("window", globalThis);
    const { internals } = supervisor();
    (internals as unknown as { desired: boolean }).desired = true;
    internals.machineMultiplex = true;
    vi.spyOn(internals, "request").mockRejectedValue(new TypeError("Failed to fetch"));

    await expect(internals.openMachineSocket("patient-pelican-9")).rejects.toBeInstanceOf(TypeError);
    expect(internals.machineMultiplex, "a network failure is not a protocol verdict").toBe(true);
  });

  it("waits at most 10 s between reconnects and gives a doubted socket 3 s to answer", () => {
    expect(MACHINE_RETRY_CEILING_MS).toBe(10_000);
    expect(SOCKET_PROBE_TIMEOUT_MS).toBe(3_000);
  });
});

describe("held sends (cas-0978)", () => {
  it("holds a send with no receipt clock, then starts it when released", () => {
    const history = new ConversationHistory();
    history.hold("send-1", "patient-pelican-9", "Are you there?", 1_000);
    const held = history.events.find((event) => event.kind === "send")!;
    expect(held.value).toMatchObject({ id: "send-1", state: "sending", held: true });
    expect("sentAt" in held.value ? held.value.sentAt : undefined).toBeUndefined();
    expect(history.nextReceiptCheck(5_000), "a held send cannot time out").toBeUndefined();

    expect(history.release("send-1", 9_000)).toBe(true);
    expect(held.value).toMatchObject({ state: "sending", sentAt: 9_000 });
    expect("held" in held.value && held.value.held).toBeFalsy();
    expect(history.nextReceiptCheck(9_000)).toBeGreaterThan(0);
    expect(history.release("send-1"), "released once").toBe(false);
  });

  it("turns a held send that never went out into Not sent", () => {
    const history = new ConversationHistory();
    history.hold("send-2", "patient-pelican-9", "Hello?", 1_000);
    expect(history.reject("send-2", "Not sent: lost connection to Atlas · Linux.")).toBe(true);
    expect(history.preview()).toBe("Not sent: Hello?");
  });
});
