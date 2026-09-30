import { webcrypto } from "node:crypto";
import { afterEach, describe, expect, it, vi } from "vitest";
import { replaceMachineConnection } from "./connection-lifecycle";
import { HubConnectionSupervisor, type ConnectionState, type HubCallbacks } from "./connection";
import { HEARTBEAT_INTERVAL_MS, headerConnectionChip } from "./connection-state";
import { createDeviceKey } from "./dpop";
import type { StoredMachine } from "./types";

const supervisors: HubConnectionSupervisor[] = [];
afterEach(() => {
  for (const supervisor of supervisors.splice(0)) supervisor.stop();
  vi.restoreAllMocks();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

async function storedMachine(credentialId: string): Promise<StoredMachine> {
  return {
    id: "stable-hub-id", label: "Studio Mac", baseUrl: "https://workstation.tail.example",
    deviceId: `device-${credentialId}`, credentialId, credential: `opaque-${credentialId}`,
    expiresAt: new Date(Date.now() + 60_000).toISOString(), scopes: ["machine-read"],
    ...await createDeviceKey(),
  };
}

// Only HTTP, the browser clock and the open SSE stream are controlled. Signing,
// authentication, transitions, heartbeat scheduling and teardown remain real.
function transport(multiplex = false) {
  vi.stubGlobal("crypto", webcrypto);
  vi.stubGlobal("window", globalThis);
  let clock = 0;
  vi.spyOn(performance, "now").mockImplementation(() => clock);
  const requests: { path: string; authorization: string | null }[] = [];
  const streams: { signal: AbortSignal; credential: string | null }[] = [];
  vi.stubGlobal("fetch", vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const path = new URL(String(input)).pathname;
    const authorization = new Headers(init?.headers).get("Authorization");
    requests.push({ path, authorization });
    if (path === "/v1/health") return new Response("{}");
    if (authorization?.includes("opaque-needs-pairing")) {
      return Response.json({ error: "credential_revoked" }, { status: 403 });
    }
    if (path === "/v1/machine") {
      clock += 24;
      return Response.json({ capabilities: multiplex ? ["machine_multiplex_v2"] : [] });
    }
    if (path === "/v1/sessions") {
      clock += 17;
      return Response.json({ sessions: [] });
    }
    if (path === "/v1/events") {
      const signal = init!.signal as AbortSignal;
      streams.push({ signal, credential: authorization });
      return new Response(new ReadableStream<Uint8Array>({
        start(controller) {
          signal.addEventListener("abort", () => controller.close(), { once: true });
        },
      }));
    }
    if (path === "/v1/auth/websocket-ticket") return Response.json({ ticket: "transport-ticket" });
    throw new Error(`unexpected transport request: ${path}`);
  }));
  return { requests, streams, elapse: (ms: number) => { clock += ms; } };
}

class TransportSocket {
  static readonly OPEN = 1;
  static readonly CONNECTING = 0;
  static instances: TransportSocket[] = [];
  readyState = TransportSocket.CONNECTING;
  binaryType = "";
  sent: string[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((message: MessageEvent) => void) | null = null;
  onclose: ((event: CloseEvent) => void) | null = null;
  onerror: (() => void) | null = null;
  constructor(readonly url: URL) { TransportSocket.instances.push(this); }
  open(): void { this.readyState = TransportSocket.OPEN; this.onopen?.(); }
  receive(message: unknown): void { this.onmessage?.({ data: JSON.stringify(message) } as MessageEvent); }
  close(code = 1000): void { this.readyState = 3; this.onclose?.({ code } as CloseEvent); }
  send(value: string): void { this.sent.push(value); }
}

function supervisor(machine: StoredMachine, onState: HubCallbacks["onState"] = () => {}): HubConnectionSupervisor {
  const connection = new HubConnectionSupervisor(machine, {
    onState, onSessions: () => {}, onMachineEvent: () => {}, onSessionState: () => {},
    onOutput: () => {}, onPaneKeyframe: () => {}, onSocketError: () => {},
  });
  supervisors.push(connection);
  return connection;
}

describe("Commander live connection lifecycle", () => {
  it("replaces a refused pairing with a real supervisor using the new credential", async () => {
    const hub = transport();
    const prior = await storedMachine("needs-pairing");
    const states = new Map<string, ConnectionState>();
    const old = supervisor(prior, (state) => states.set(prior.id, state));
    const connections = new Map([[prior.id, old]]);
    old.start();
    await vi.waitFor(() => expect(old.snapshot()).toMatchObject({ phase: "failed", stage: "auth" }));

    const replacement = await storedMachine("replacement");
    const installed = replaceMachineConnection(replacement, connections, states, (machine) => supervisor(machine));
    expect(states.has(prior.id)).toBe(false);
    expect(old.snapshot().phase).toBe("idle");
    expect(connections.get(prior.id)).toBe(installed);
    await vi.waitFor(() => expect(installed.snapshot()).toMatchObject({ phase: "live", stage: "live" }));
    expect(installed.snapshot().authFailure).toBeUndefined();
    expect(hub.streams).toHaveLength(1);
    expect(hub.streams[0]!.credential).toContain("opaque-replacement");
    expect(hub.requests.filter((request) => request.path === "/v1/events")).toEqual([
      { path: "/v1/events", authorization: hub.streams[0]!.credential },
    ]);
  });

  it("aborts the old live transport and stops its heartbeats when replacing it", async () => {
    const hub = transport();
    const prior = await storedMachine("old");
    const old = supervisor(prior);
    const connections = new Map([[prior.id, old]]);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    old.start();
    await vi.waitFor(() => expect(old.snapshot().phase).toBe("live"));
    const installed = replaceMachineConnection(await storedMachine("replacement"), connections, new Map(), (machine) => supervisor(machine));
    await vi.waitFor(() => expect(installed.snapshot().phase).toBe("live"));
    expect(hub.streams[0]!.signal.aborted).toBe(true);
    expect(hub.streams[1]!.signal.aborted).toBe(false);
    const boundary = hub.requests.length;
    await vi.advanceTimersByTimeAsync(HEARTBEAT_INTERVAL_MS);
    await vi.waitFor(() => expect(installed.snapshot().latencyMs).toBe(41));
    expect(hub.requests.slice(boundary).map((request) => request.authorization)).toEqual([
      hub.streams[1]!.credential, hub.streams[1]!.credential,
    ]);
    expect(old.snapshot().phase).toBe("idle");
  });

  it("shows Checking before the first heartbeat, then the measured round trip", async () => {
    const hub = transport();
    const connection = supervisor(await storedMachine("latency"));
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    expect(connection.snapshot().latencyMs).toBeUndefined();
    expect(headerConnectionChip(connection.snapshot(), "live", "Live")).toEqual({ state: "checking", text: "Checking…" });
    const boundary = hub.requests.length;
    await vi.advanceTimersByTimeAsync(HEARTBEAT_INTERVAL_MS);
    await vi.waitFor(() => expect(connection.snapshot().latencyMs).toBe(41));
    expect(hub.requests.slice(boundary).map((request) => request.path)).toEqual(["/v1/sessions", "/v1/machine"]);
    expect(headerConnectionChip(connection.snapshot(), "live", "Live")).toEqual({ state: "live", text: "41ms" });
  });

  it("keeps multiplexed latency absent until the matching health pong arrives", async () => {
    const hub = transport(true);
    TransportSocket.instances = [];
    vi.stubGlobal("WebSocket", TransportSocket);
    const connection = supervisor(await storedMachine("socket-latency"));
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    const attached = connection.attach("session-a");
    await vi.waitFor(() => expect(TransportSocket.instances).toHaveLength(1));
    const socket = TransportSocket.instances[0]!;
    socket.open();
    socket.receive({ proto: 2, capabilities: ["pty_binary", "machine_multiplex"] });
    await attached;
    socket.receive({ channel: "pty:session-a", message: { Welcome: {
      session_name: "session-a", state: { focused_pane: "supervisor", panes: [] },
      protocol_version: 3, capabilities: [],
    } } });
    expect(connection.snapshot().latencyMs).toBeUndefined();
    await vi.advanceTimersByTimeAsync(HEARTBEAT_INTERVAL_MS);
    await vi.waitFor(() => expect(socket.sent.map((frame) => JSON.parse(frame)).some((frame) => frame.channel === "health")).toBe(true));
    const ping = socket.sent.map((frame) => JSON.parse(frame)).find((frame) => frame.channel === "health").ping as number;
    expect(connection.snapshot().latencyMs).toBeUndefined();
    socket.receive({ channel: "health", pong: ping - 1 });
    expect(connection.snapshot().latencyMs).toBeUndefined();
    hub.elapse(24);
    socket.receive({ channel: "health", pong: ping });
    // 17ms for the catalog refresh plus 24ms for the socket's answer.
    expect(connection.snapshot().latencyMs).toBe(41);
    expect(headerConnectionChip(connection.snapshot(), "live", "Live")).toEqual({ state: "live", text: "41ms" });
  });
});
