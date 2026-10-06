import { webcrypto } from "node:crypto";
import { afterEach, describe, expect, it, vi } from "vitest";
import { replaceMachineConnection } from "./connection-lifecycle";
import { HubConnectionSupervisor, type ConnectionState, type HubCallbacks } from "./connection";
import { HEARTBEAT_INTERVAL_MS, MACHINE_RETRY_CEILING_MS } from "./connection-state";
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
  let blocked = false;
  let refused = false;
  let stalledRefresh = false;
  let catalogRevision: number | undefined;
  let eventController: ReadableStreamDefaultController<Uint8Array> | undefined;
  vi.stubGlobal("fetch", vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const path = new URL(String(input)).pathname;
    const authorization = new Headers(init?.headers).get("Authorization");
    requests.push({ path, authorization });
    if (path === "/v1/health") return new Response("{}");
    if (blocked) throw new TypeError("Failed to fetch");
    if (refused || authorization?.includes("opaque-needs-pairing")) {
      // Only an explicit refusal can end a pairing.
      return Response.json({ reason: "revoked", retryable: false }, { status: 401 });
    }
    if (path === "/v1/machine") {
      clock += 24;
      return Response.json({ capabilities: multiplex ? ["machine_multiplex_v2"] : [] });
    }
    if (path === "/v1/sessions") {
      if (stalledRefresh && !(init?.signal)) return new Promise<Response>(() => {});
      if (stalledRefresh) return new Promise<Response>((_resolve, reject) => {
        init!.signal!.addEventListener("abort", () => reject(init!.signal!.reason), { once: true });
      });
      clock += 17;
      return Response.json({ sessions: catalogRevision === undefined ? [] : [{ name: `catalog-${catalogRevision}` }] });
    }
    if (path === "/v1/events") {
      const signal = init!.signal as AbortSignal;
      streams.push({ signal, credential: authorization });
      return new Response(new ReadableStream<Uint8Array>({
        start(controller) {
          eventController = controller;
          signal.addEventListener("abort", () => controller.error(signal.reason), { once: true });
        },
      }));
    }
    if (path === "/v1/auth/websocket-ticket") return Response.json({ ticket: "transport-ticket" });
    throw new Error(`unexpected transport request: ${path}`);
  }));
  return {
    requests, streams, block: (value: boolean) => { blocked = value; }, elapse: (ms: number) => { clock += ms; },
    stallRefresh: (value: boolean) => { stalledRefresh = value; },
    refusePairing: (value: boolean) => { refused = value; },
    catalogRevision: (value: number) => { catalogRevision = value; },
    event: (event: Record<string, unknown> = { kind: "session_added" }) => eventController!.enqueue(new TextEncoder().encode(`data: ${JSON.stringify(event)}\n\n`)),
    endEvents: () => eventController!.close(),
    replay: (events: Record<string, unknown>[]) => eventController!.enqueue(new TextEncoder().encode(events.map(event => `data: ${JSON.stringify(event)}\n\n`).join(""))),
  };
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
  receive(message: unknown): void { if (this.readyState === TransportSocket.OPEN) this.onmessage?.({ data: JSON.stringify(message) } as MessageEvent); }
  close(code = 1000): void { this.readyState = 3; this.onclose?.({ code } as CloseEvent); }
  send(value: string): void { this.sent.push(value); }
}

function supervisor(machine: StoredMachine, onState: HubCallbacks["onState"] = () => {}, onMachineEvent: HubCallbacks["onMachineEvent"] = () => {}, onSessions: HubCallbacks["onSessions"] = () => {}): HubConnectionSupervisor {
  const connection = new HubConnectionSupervisor(machine, {
    onState, onSessions, onMachineEvent, onSessionState: () => {},
    onOutput: () => {}, onPaneKeyframe: () => {}, onSocketError: () => {},
  });
  supervisors.push(connection);
  return connection;
}

describe("Commander live connection lifecycle", () => {
  it("delivers a stalled catalog's entire burst and joins manual refreshes to its flight (cas-b55b)", async () => {
    const hub = transport();
    const events: Record<string, unknown>[] = [];
    const connection = supervisor(await storedMachine("burst"), () => {}, event => events.push(event));
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    const before = hub.requests.filter(row => row.path === "/v1/sessions").length;
    hub.stallRefresh(true);
    for (let i = 1; i <= 100; i++) hub.event({ kind: "session_added", sequence: i });
    await vi.waitFor(() => expect(events).toHaveLength(100));
    const manual = Array.from({ length: 10 }, () => connection.refreshSessions().catch(() => {}));
    expect(hub.requests.filter(row => row.path === "/v1/sessions").length - before).toBe(1);
    connection.stop();
    await Promise.all(manual);
  });
  it.each([false, true])("coalesces 100 events in 1s with fresh final catalog (multiplex=%s, cas-b55b)", async (multiplex) => {
    const hub = transport(multiplex);
    TransportSocket.instances = [];
    vi.stubGlobal("WebSocket", TransportSocket);
    let latest: string | undefined;
    const events: Record<string, unknown>[] = [];
    const connection = supervisor(await storedMachine("rate-cap"), () => {}, event => events.push(event), sessions => { latest = sessions[0]?.name; });
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    let socket: TransportSocket | undefined;
    if (multiplex) {
      const attached = connection.attach("session-a");
      await vi.waitFor(() => expect(TransportSocket.instances).toHaveLength(1));
      socket = TransportSocket.instances[0]!;
      socket.open(); socket.receive({ proto: 2 });
      await attached;
    }
    const before = hub.requests.filter(row => row.path === "/v1/sessions").length;
    for (let i = 1; i <= 100; i++) {
      hub.catalogRevision(i);
      const event = { kind: "session_added", sequence: i };
      // Mix both transports: they must share one refresh lane, not one each.
      if (socket && i % 2 === 0) socket.receive({ channel: "events", event });
      else hub.event(event);
      await vi.advanceTimersByTimeAsync(10);
    }
    expect(hub.requests.filter(row => row.path === "/v1/sessions").length - before).toBeLessThanOrEqual(2);
    await vi.advanceTimersByTimeAsync(1_000);
    await vi.waitFor(() => expect(latest).toBe("catalog-100"));
    expect(events).toHaveLength(100);
    expect(hub.requests.filter(row => row.path === "/v1/sessions").length - before).toBeLessThanOrEqual(2);
    connection.stop();
    const stopped = hub.requests.length;
    await vi.advanceTimersByTimeAsync(10_000);
    expect(hub.requests).toHaveLength(stopped);
  });

  it("distinguishes a measured browser health503 from an opaque fetch failure (cas-2b3a5)", async () => {
    transport();
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response("Unavailable", { status: 503 })));
    const connection = supervisor(await storedMachine("health-503"));
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().cause).toMatchObject({ code: "health_http_unavailable", status: 503, layer: "http" }));
    expect(connection.snapshot().authFailure).toBeUndefined();
  });
  it("keeps delivering a flood while the catalog stalls, with one catalog flight (cas-2b3a5)", async () => {
    const hub = transport();
    const events: Record<string, unknown>[] = [];
    const connection = supervisor(await storedMachine("burst"), () => {}, event => events.push(event));
    connection.start(); await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    const before = hub.requests.filter(row => row.path === "/v1/sessions").length;
    hub.stallRefresh(true);
    for (let i = 1; i <= 100; i++) hub.event({ kind: "session_added", sequence: i, revision: 0 });
    await vi.waitFor(() => expect(events).toHaveLength(100));
    expect(hub.requests.filter(row => row.path === "/v1/sessions").length - before).toBe(1);
  });
  it("resyncs gaps, accepts replay revisions, and resets on a new epoch (cas-2b3a5)", async () => {
    const hub = transport();
    const events: Record<string, unknown>[] = [];
    const connection = supervisor(await storedMachine("event-gap"), () => {}, event => events.push(event));
    vi.spyOn(Math, "random").mockReturnValue(0.5);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    connection.start(); await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    hub.event({ kind: "stream_metadata", epoch: "one", oldest_sequence: 1, latest_sequence: 1 });
    hub.event({ kind: "session_added", sequence: 1, revision: 0 });
    hub.event({ kind: "replay_complete" });
    hub.event({ kind: "session_added", sequence: 1, revision: 1 });
    hub.event({ kind: "session_added", sequence: 3, revision: 0 });
    await vi.waitFor(() => expect(connection.snapshot().cause?.code).toBe("event_sequence_gap"));
    expect(events.map(event => [event.sequence, event.revision])).toEqual([[1, 0], [1, 1], [3, 0]]);
    await vi.advanceTimersByTimeAsync(1_000);
    await vi.waitFor(() => expect(hub.streams).toHaveLength(2));
    hub.event({ kind: "stream_metadata", epoch: "two", oldest_sequence: 1, latest_sequence: 1 });
    hub.event({ kind: "session_added", sequence: 1, revision: 0 });
    hub.event({ kind: "replay_complete" });
    await vi.waitFor(() => expect(events).toHaveLength(4));
    expect(connection.snapshot().cause?.code).toBe("event_epoch_changed");
  });
  it("clears permission guidance when the same tailnet pairing reconnects (cas-b85a)", async () => {
    const hub = transport();
    const machine = await storedMachine("local-network");
    machine.baseUrl = "https://soundwave-linux.tailf5a734.ts.net";
    const query = vi.fn().mockResolvedValue({ state: "denied" });
    vi.stubGlobal("navigator", { permissions: { query } });
    const connection = supervisor(machine);
    vi.spyOn(Math, "random").mockReturnValue(0.5);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    hub.block(true);
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().networkAccessHelp).toContain("allow Local network access"));
    expect(connection.snapshot().authFailure).toBeUndefined();
    query.mockResolvedValue({ state: "granted" });
    hub.block(false);
    await vi.advanceTimersByTimeAsync(1_000);
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    expect(connection.snapshot().networkAccessHelp).toBeUndefined();
    expect(hub.streams[0]!.credential).toContain("opaque-local-network");
  });
  it("clears permission remediation when the hub explicitly refuses the pairing (cas-b85a)", async () => {
    const hub = transport();
    const machine = await storedMachine("needs-pairing");
    machine.baseUrl = "https://soundwave-linux.tailf5a734.ts.net";
    vi.stubGlobal("navigator", { permissions: { query: vi.fn().mockResolvedValue({ state: "denied" }) } });
    const connection = supervisor(machine);
    vi.spyOn(Math, "random").mockReturnValue(0.5);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    hub.block(true);
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().networkAccessHelp).toContain("allow Local network access"));
    hub.block(false);
    await vi.advanceTimersByTimeAsync(1_000);
    await vi.waitFor(() => expect(connection.snapshot().authFailure).toBe("revoked"));
    expect(connection.snapshot().networkAccessHelp).toBeUndefined();
    const requests = hub.requests.length;
    await vi.advanceTimersByTimeAsync(10_000);
    expect(hub.requests).toHaveLength(requests);
  });
  it.each([undefined, "wifi"])("keeps network-quality estimates from bypassing four failed heartbeats (type=%s, cas-eefe)", async type => {
    const hub = transport();
    const hints = Object.assign(new EventTarget(), { type, rtt: 50, downlink: 10, effectiveType: "4g" });
    vi.stubGlobal("navigator", { connection: hints });
    vi.stubGlobal("document", Object.assign(new EventTarget(), { visibilityState: "visible" }));
    const connection = supervisor(await storedMachine("quality-estimate"));
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    vi.stubGlobal("window", Object.assign(new EventTarget(), { setTimeout, clearTimeout, setInterval, clearInterval }));
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    hub.block(true);
    await vi.advanceTimersByTimeAsync(HEARTBEAT_INTERVAL_MS);
    await vi.waitFor(() => expect(connection.snapshot().missedHeartbeats).toBe(1));
    const before = hub.requests.filter(request => request.path === "/v1/machine").length;
    hints.rtt = 250;
    hints.downlink = 1;
    hints.effectiveType = "3g";
    hints.dispatchEvent(new Event("change"));
    await vi.advanceTimersByTimeAsync(HEARTBEAT_INTERVAL_MS);
    await vi.waitFor(() => expect(connection.snapshot().missedHeartbeats).toBe(2));
    expect(connection.snapshot().phase).toBe("live");
    expect(connection.snapshot().degraded).toBe(true);
    expect(hub.requests.filter(request => request.path === "/v1/machine")).toHaveLength(before);
    await vi.advanceTimersByTimeAsync(HEARTBEAT_INTERVAL_MS);
    await vi.waitFor(() => expect(connection.snapshot().missedHeartbeats).toBe(3));
    expect(connection.snapshot().phase).toBe("live");
    await vi.advanceTimersByTimeAsync(HEARTBEAT_INTERVAL_MS);
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("backoff"));
  });

  it("still probes a measured network transport change immediately (cas-eefe)", async () => {
    const hub = transport();
    const hints = Object.assign(new EventTarget(), { type: "wifi" });
    vi.stubGlobal("navigator", { connection: hints });
    vi.stubGlobal("document", Object.assign(new EventTarget(), { visibilityState: "visible" }));
    const connection = supervisor(await storedMachine("transport-change"));
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    vi.stubGlobal("window", Object.assign(new EventTarget(), { setTimeout, clearTimeout, setInterval, clearInterval }));
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    const before = hub.requests.filter(request => request.path === "/v1/machine").length;
    hub.block(true);
    hints.type = "cellular";
    hints.dispatchEvent(new Event("change"));
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("backoff"));
    expect(hub.requests.filter(request => request.path === "/v1/machine")).toHaveLength(before + 1);
    expect(connection.snapshot().missedHeartbeats).toBe(4);
  });

  it("bounds an unanswered event catalog refresh to the probe deadline without tearing down the live stream (cas-b85a, cas-eefe)", async () => {
    const hub = transport();
    const connection = supervisor(await storedMachine("refresh-deadline"));
    vi.spyOn(Math, "random").mockReturnValue(0.5);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    vi.spyOn(AbortSignal, "timeout").mockImplementation(ms => {
      const controller = new AbortController();
      setTimeout(() => controller.abort(new DOMException("timed out", "TimeoutError")), ms);
      return controller.signal;
    });
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    const sessionReads = () => hub.requests.filter(request => request.path === "/v1/sessions").length;
    const before = sessionReads();
    hub.stallRefresh(true);
    hub.event();
    await vi.waitFor(() => expect(sessionReads()).toBe(before + 1));
    await vi.advanceTimersByTimeAsync(3_000);
    // The stalled read ended at its deadline: the next event starts a new one.
    hub.stallRefresh(false);
    hub.event();
    await vi.waitFor(() => expect(sessionReads()).toBe(before + 2));
    // Events kept flowing on the same stream; the heartbeat, not this read, judges the machine.
    expect(connection.snapshot().phase).toBe("live");
    expect(connection.snapshot().missedHeartbeats).toBe(0);
    expect(hub.streams).toHaveLength(1);
  });
  it("keeps a failed event catalog read from bypassing four failed heartbeats on a half-open machine (cas-eefe)", async () => {
    const hub = transport();
    const connection = supervisor(await storedMachine("half-open-catalog"));
    vi.spyOn(Math, "random").mockReturnValue(0.5);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    // Half-open: HTTP stops answering while the event stream stays open.
    hub.block(true);
    const sessionReads = () => hub.requests.filter(request => request.path === "/v1/sessions").length;
    for (let beat = 1; beat <= 3; beat++) {
      // An event lands with every beat, so its catalog read races (and
      // shares) the heartbeat's own read, as in the HUB-J12 trace.
      const reads = sessionReads();
      hub.event();
      await vi.advanceTimersByTimeAsync(HEARTBEAT_INTERVAL_MS);
      await vi.waitFor(() => expect(connection.snapshot().missedHeartbeats).toBe(beat));
      expect(sessionReads()).toBeGreaterThan(reads);
      expect(connection.snapshot().phase).toBe("live");
      if (beat >= 2) expect(connection.snapshot().degraded).toBe(true);
      expect(hub.streams).toHaveLength(1);
    }
    hub.event();
    await vi.advanceTimersByTimeAsync(HEARTBEAT_INTERVAL_MS);
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("backoff"));
  });
  it("still ends the stream when the event catalog read finds the pairing refused (cas-eefe)", async () => {
    const hub = transport();
    const states: ConnectionState[] = [];
    const connection = supervisor(await storedMachine("catalog-refusal"), state => states.push(state));
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    hub.refusePairing(false);
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    hub.refusePairing(true);
    hub.event();
    await vi.waitFor(() => expect(connection.snapshot().authFailure).toBe("revoked"));
    expect(connection.snapshot().phase).toBe("failed");
  });
  it("aborts a pending event catalog refresh when the network goes offline (cas-b85a)", async () => {
    const hub = transport();
    const events = new EventTarget();
    vi.stubGlobal("addEventListener", events.addEventListener.bind(events));
    vi.stubGlobal("removeEventListener", events.removeEventListener.bind(events));
    vi.stubGlobal("document", new EventTarget());
    const connection = supervisor(await storedMachine("stalled-refresh"));
    vi.spyOn(Math, "random").mockReturnValue(0.5);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    const before = hub.requests.length;
    hub.stallRefresh(true);
    hub.event();
    await vi.waitFor(() => expect(hub.requests.length).toBeGreaterThan(before));
    events.dispatchEvent(new Event("offline"));
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("backoff"));
    hub.stallRefresh(false);
    await vi.advanceTimersByTimeAsync(1_000);
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    expect(hub.streams).toHaveLength(2);
  });

  it("retries a local signing error before fetch without calling it a lost pairing (cas-b85a)", async () => {
    const hub = transport();
    const machine = await storedMachine("signing-error");
    const key = machine.privateKey;
    machine.privateKey = undefined as unknown as CryptoKey;
    const connection = supervisor(machine);
    vi.spyOn(Math, "random").mockReturnValue(0.5);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("backoff"));
    expect(connection.snapshot().authFailure).toBeUndefined();
    expect(hub.requests.every(request => request.path === "/v1/health")).toBe(true);
    machine.privateKey = key;
    await vi.advanceTimersByTimeAsync(1_000);
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    expect(hub.streams[0]!.credential).toContain("opaque-signing-error");
  });
  it("retries opaque authenticated failures without discarding an active pairing (cas-b85a)", async () => {
    const hub = transport();
    const connection = supervisor(await storedMachine("browser-blocked"));
    vi.spyOn(Math, "random").mockReturnValue(0.5);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    hub.block(true);
    for (let beat = 0; beat < 4; beat++) {
      const before = hub.requests.length;
      await vi.advanceTimersByTimeAsync(HEARTBEAT_INTERVAL_MS);
      await vi.waitFor(() => expect(hub.requests.length).toBeGreaterThan(before));
    }
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("backoff"));
    await vi.advanceTimersByTimeAsync(1_000);
    await vi.waitFor(() => expect(connection.snapshot()).toMatchObject({ phase: "backoff", stage: "auth" }));
    expect(connection.snapshot().authFailure).toBeUndefined();
    hub.block(false);
    await vi.advanceTimersByTimeAsync(MACHINE_RETRY_CEILING_MS);
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    expect(hub.streams).toHaveLength(2);
    expect(hub.streams[1]!.credential).toContain("opaque-browser-blocked");
  });
  it("replaces a refused pairing with a real supervisor using the new credential", async () => {
    const hub = transport();
    const prior = await storedMachine("needs-pairing");
    const states = new Map<string, ConnectionState>();
    const old = supervisor(prior, (state) => states.set(prior.id, state));
    const connections = new Map([[prior.id, old]]);
    old.start();
    await vi.waitFor(() => expect(old.snapshot()).toMatchObject({ phase: "failed", stage: "auth", authFailure: "revoked" }));

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

  it("has no latency before the first heartbeat, then the measured round trip", async () => {
    const hub = transport();
    const connection = supervisor(await storedMachine("latency"));
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    expect(connection.snapshot().latencyMs).toBeUndefined();
    const boundary = hub.requests.length;
    await vi.advanceTimersByTimeAsync(HEARTBEAT_INTERVAL_MS);
    await vi.waitFor(() => expect(connection.snapshot().latencyMs).toBe(41));
    expect(hub.requests.slice(boundary).map((request) => request.path)).toEqual(["/v1/sessions", "/v1/machine"]);
  });

  it.each([false, true])("keeps a healthy legacy attach through a peer exit (replay=%s, cas-49cc)", async (replay) => {
    const hub = transport();
    TransportSocket.instances = [];
    vi.stubGlobal("WebSocket", TransportSocket);
    vi.spyOn(Math, "random").mockReturnValue(0.5);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    const events: Record<string, unknown>[] = [];
    const connection = supervisor(await storedMachine("peer-clean-exit"), () => {}, event => events.push(event));
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    for (const session of ["healthy", "removed-peer"]) {
      await connection.attach(session);
      const socket = TransportSocket.instances.at(-1)!;
      socket.open();
      socket.receive({ Welcome: { state: { panes: [] }, protocol_version: 3, capabilities: ["conversation_history"] } });
    }
    const healthy = TransportSocket.instances[0]!;
    const peer = TransportSocket.instances[1]!;
    const window = Array.from({ length: 1021 }, (_, index) => ({ kind: "pane_added", sequence: 4180 + index, session: "removed-peer" }));
    const tail = [
      { kind: "pane_exited", sequence: 5201, session: "removed-peer", pane_id: "worker" },
      { kind: "daemon_disconnected", sequence: 5202, session: "removed-peer", diagnostic: { cause: { kind: "clean_exit", code: 0 }, next_action: "Inspect the factory daemon log and session metadata; do not infer a cause from a closed socket alone." } },
      { kind: "session_removed", sequence: 5203, session: "removed-peer" },
    ];
    const metadata = { kind: "stream_metadata", epoch: "stable-hub", oldest_sequence: 4180, latest_sequence: 5203 };
    hub.replay([metadata, ...window, { kind: "replay_complete" }, ...tail]);
    peer.close(1000);
    await vi.waitFor(() => expect(events).toHaveLength(1024));
    expect(healthy.readyState).toBe(TransportSocket.OPEN);
    if (replay) {
      hub.endEvents();
      await vi.waitFor(() => expect(connection.snapshot().phase).toBe("backoff"));
      await vi.advanceTimersByTimeAsync(1_000);
      await vi.waitFor(() => expect(hub.streams).toHaveLength(2));
      hub.replay([metadata, ...window, ...tail, { kind: "replay_complete" }]);
    }
    await vi.advanceTimersByTimeAsync(12_000);
    expect(events).toHaveLength(1024);
    expect(healthy.readyState).toBe(TransportSocket.OPEN);
    expect(TransportSocket.instances.filter(socket => socket.url.pathname.includes("/healthy/"))).toHaveLength(1);
    expect(connection.attachSnapshot("healthy")?.phase).toBe("live");
  });

  it("refreshes the session catalog on event recovery without replacing its speaking attach (cas-49cc)", async () => {
    const hub = transport();
    TransportSocket.instances = [];
    vi.stubGlobal("WebSocket", TransportSocket);
    vi.spyOn(Math, "random").mockReturnValue(0.5);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    let listed: string | undefined;
    hub.catalogRevision(1);
    const connection = supervisor(await storedMachine("event-catalog"), () => {}, () => {}, sessions => { listed = sessions[0]?.name; });
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    expect(listed).toBe("catalog-1");
    await connection.attach("healthy");
    const socket = TransportSocket.instances[0]!;
    socket.open(); socket.receive({ Welcome: { state: { panes: [] } } });
    hub.endEvents();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("backoff"));
    hub.catalogRevision(2);
    await vi.advanceTimersByTimeAsync(1_000);
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    expect(listed).toBe("catalog-2");
    expect(TransportSocket.instances).toHaveLength(1);
    expect(socket.readyState).toBe(TransportSocket.OPEN);
  });

  it.each([false, true])("measures current ready transport reads without relaxing the send fence (multiplex=%s, cas-49cc)", async (multiplex) => {
    const hub = transport(multiplex);
    TransportSocket.instances = [];
    vi.stubGlobal("WebSocket", TransportSocket);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    const connection = supervisor(await storedMachine("read-health"));
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    const pending = connection.attach("healthy");
    await vi.waitFor(() => expect(TransportSocket.instances).toHaveLength(1));
    const socket = TransportSocket.instances[0]!;
    socket.open();
    if (multiplex) socket.receive({ proto: 2 });
    await pending;
    const receive = (message: unknown) => socket.receive(multiplex ? { channel: "pty:healthy", message } : message);
    receive({ Welcome: { state: { panes: [] } } });
    await vi.waitFor(() => expect(connection.attachSnapshot("healthy")?.phase).toBe("live"));
    expect(connection.hasLiveAttach("healthy")).toBe(true);
    expect(connection.hasLiveAttach("missing")).toBe(false);
    hub.endEvents();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("backoff"));
    expect(connection.hasLiveAttach("healthy")).toBe(true);
    expect(connection.send("healthy", { SendMessage: { target: "supervisor", text: "Still fenced during event recovery" } })).toBe(false);
    // Move the read clock without running retry timers or heartbeat signing.
    vi.setSystemTime(Date.now() + 20_000);
    expect(connection.hasLiveAttach("healthy")).toBe(false);
    receive({ StateUpdate: { state: { panes: [] } } });
    expect(connection.hasLiveAttach("healthy")).toBe(true);
    socket.close(1000);
    expect(connection.hasLiveAttach("healthy")).toBe(false);
  });

  it("keeps a speaking legacy attach after a successful network hint (cas-49cc)", async () => {
    const hub = transport();
    const hints = new EventTarget();
    vi.stubGlobal("addEventListener", hints.addEventListener.bind(hints));
    vi.stubGlobal("removeEventListener", hints.removeEventListener.bind(hints));
    vi.stubGlobal("document", new EventTarget());
    TransportSocket.instances = [];
    vi.stubGlobal("WebSocket", TransportSocket);
    const connection = supervisor(await storedMachine("healthy-hint"));
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    await connection.attach("healthy");
    const socket = TransportSocket.instances[0]!;
    socket.open();
    socket.receive({ Welcome: { state: { panes: [] } } });
    const reads = hub.requests.filter(row => row.path === "/v1/machine").length;
    hints.dispatchEvent(new Event("online"));
    await vi.waitFor(() => expect(hub.requests.filter(row => row.path === "/v1/machine")).toHaveLength(reads + 1));
    expect(socket.readyState).toBe(TransportSocket.OPEN);
    expect(TransportSocket.instances).toHaveLength(1);
    expect(hub.requests.filter(row => row.path === "/v1/auth/websocket-ticket")).toHaveLength(1);
  });

  it("bounds 30 seconds of 1Hz SSE flaps without redialing a speaking attach (cas-49cc)", async () => {
    const hub = transport();
    TransportSocket.instances = [];
    class SpeakingSocket extends TransportSocket {
      constructor(url: URL) {
        super(url);
        queueMicrotask(() => { this.open(); this.receive({ Welcome: { state: { panes: [] } } }); });
      }
    }
    vi.stubGlobal("WebSocket", SpeakingSocket);
    vi.spyOn(Math, "random").mockReturnValue(0.5);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    const retries: number[] = [];
    const connection = supervisor(await storedMachine("flapping-events"), state => {
      if (state.phase === "backoff") retries.push(state.retryInMs!);
    });
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    await connection.attach("healthy");
    await vi.waitFor(() => expect(connection.attachSnapshot("healthy")?.phase).toBe("live"));
    const socket = TransportSocket.instances[0]!;
    const started = Date.now();
    const pulse = setInterval(() => {
      socket.receive({ StateUpdate: { state: { panes: [] } } });
      try { hub.endEvents(); } catch { /* no stream open during backoff */ }
    }, 1_000);
    for (let second = 0; second < 30; second++) {
      await vi.advanceTimersByTimeAsync(1_000);
      // Let real signing/fetch work settle before the next protocol second;
      // otherwise fake time can manufacture a signing deadline failure.
      await vi.waitFor(() => expect(["live", "backoff"]).toContain(connection.snapshot().phase), { interval: 1 });
    }
    clearInterval(pulse);
    console.info("cas-49cc flap proof", { elapsedMs: Date.now() - started, streams: hub.streams.length,
      attaches: TransportSocket.instances.length, tickets: hub.requests.filter(row => row.path === "/v1/auth/websocket-ticket").length, retries });
    expect(hub.streams.length).toBeLessThanOrEqual(6);
    expect(retries.slice(0, 4)).toEqual([1_000, 2_000, 4_000, 8_000]);
    expect(TransportSocket.instances).toHaveLength(1);
    expect(hub.requests.filter(row => row.path === "/v1/auth/websocket-ticket")).toHaveLength(1);
    expect(socket.readyState).toBe(TransportSocket.OPEN);
  });

  it.each([false, true])("still replaces a stale or proved-offline legacy attach (offline=%s, cas-49cc)", async (offline) => {
    transport();
    const hints = new EventTarget();
    vi.stubGlobal("addEventListener", hints.addEventListener.bind(hints));
    vi.stubGlobal("removeEventListener", hints.removeEventListener.bind(hints));
    vi.stubGlobal("document", new EventTarget());
    TransportSocket.instances = [];
    vi.stubGlobal("WebSocket", TransportSocket);
    vi.spyOn(Math, "random").mockReturnValue(0.5);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    const connection = supervisor(await storedMachine("untrusted-attach"));
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    await connection.attach("healthy");
    const socket = TransportSocket.instances[0]!;
    socket.open();
    socket.receive({ Welcome: { state: { panes: [] } } });
    await vi.waitFor(() => expect(connection.attachSnapshot("healthy")?.phase).toBe("live"));
    if (!offline) await vi.advanceTimersByTimeAsync(20_000);
    hints.dispatchEvent(new Event(offline ? "offline" : "online"));
    await vi.waitFor(() => expect(socket.readyState).toBe(3));
    expect(connection.send("healthy", { SendMessage: { target: "supervisor", text: "Never into a dead socket" } })).toBe(false);
    if (!offline) {
      await vi.waitFor(() => expect(TransportSocket.instances).toHaveLength(2));
      expect(TransportSocket.instances[1]!.readyState).toBe(TransportSocket.CONNECTING);
    }
  });

  it("resets event retry backoff only after a stream survives ten seconds (cas-49cc)", async () => {
    const hub = transport();
    vi.spyOn(Math, "random").mockReturnValue(0.5);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    const connection = supervisor(await storedMachine("event-settle"));
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    hub.endEvents();
    await vi.waitFor(() => expect(connection.snapshot().retryInMs).toBe(1_000));
    await vi.advanceTimersByTimeAsync(1_000);
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    hub.endEvents();
    await vi.waitFor(() => expect(connection.snapshot().retryInMs).toBe(2_000));
    await vi.advanceTimersByTimeAsync(2_000);
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    for (let beat = 0; beat < 2; beat++) {
      const reads = hub.requests.filter(row => row.path === "/v1/machine").length;
      await vi.advanceTimersByTimeAsync(5_000);
      await vi.waitFor(() => expect(hub.requests.filter(row => row.path === "/v1/machine")).toHaveLength(reads + 1));
    }
    hub.endEvents();
    await vi.waitFor(() => expect(connection.snapshot().retryInMs).toBe(1_000));
  });

  it("holds a send while the event stream reconnects despite an open legacy socket (cas-9dc6)", async () => {
    const hub = transport();
    TransportSocket.instances = [];
    vi.stubGlobal("WebSocket", TransportSocket);
    const connection = supervisor(await storedMachine("reconnecting-send"));
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    await connection.attach("session-a");
    const socket = TransportSocket.instances[0]!;
    socket.open();
    socket.receive({ Welcome: { state: { panes: [] }, protocol_version: 3, capabilities: ["conversation_history"] } });
    await vi.waitFor(() => expect(connection.attachSnapshot("session-a")?.phase).toBe("live"));
    hub.event({ kind: "viewer_lagged" });
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("backoff"));
    // A dispatch's IndexedDB/credential await can finish after this transition.
    // Event recovery still fences sends, while recent daemon frames keep
    // the healthy terminal available for its in-flight receipts (cas-49cc).
    expect(socket.readyState).toBe(TransportSocket.OPEN);
    const before = socket.sent.length;
    expect(connection.send("session-a", { SendMessage: { target: "supervisor", text: "Did the Mac tests start?", client_ref: "late-dispatch" } })).toBe(false);
    expect(socket.sent).toHaveLength(before);
    connection.retry();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    expect(TransportSocket.instances).toHaveLength(1);
    const recovered = socket;
    await vi.waitFor(() => expect(connection.attachSnapshot("session-a")?.phase).toBe("live"));
    expect(connection.send("session-a", { SendMessage: { target: "supervisor", text: "Did the Mac tests start?", client_ref: "late-dispatch" } })).toBe(true);
    expect(socket.sent).toHaveLength(before + 1);
    expect(recovered.sent.map(frame => JSON.parse(frame)).filter(frame => frame.SendMessage?.client_ref === "late-dispatch")).toHaveLength(1);
  });

  it("releases a held send when the machine becomes live after its session (cas-9dc6)", async () => {
    const hub = transport(true);
    TransportSocket.instances = [];
    vi.stubGlobal("WebSocket", TransportSocket);
    const fetchNow = globalThis.fetch;
    let releaseEvents!: () => void;
    const eventsReady = new Promise<void>(resolve => { releaseEvents = resolve; });
    vi.stubGlobal("fetch", vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const response = await fetchNow(input, init);
      if (new URL(String(input)).pathname === "/v1/events") await eventsReady;
      return response;
    }));
    let written = false;
    const connection: HubConnectionSupervisor = new HubConnectionSupervisor(await storedMachine("session-before-machine"), {
      onState: () => {}, onSessions: () => {}, onMachineEvent: () => {},
      onSessionState: () => {}, onOutput: () => {}, onPaneKeyframe: () => {}, onSocketError: () => {},
      onAttachState: (session, state) => {
        if (state.phase === "live" && !written) written = connection.send(session, {
          SendMessage: { target: "supervisor", text: "Held in the first tab", client_ref: "held-before-live" },
        });
      },
    });
    supervisors.push(connection);
    connection.start();
    await vi.waitFor(() => expect(hub.streams).toHaveLength(1));
    const attached = connection.attach("session-a");
    await vi.waitFor(() => expect(TransportSocket.instances).toHaveLength(1));
    const socket = TransportSocket.instances[0]!;
    socket.open(); socket.receive({ proto: 2 });
    await attached;
    socket.receive({ channel: "pty:session-a", message: { Welcome: {
      state: { panes: [] }, protocol_version: 3, capabilities: [],
    } } });
    expect(connection.attachSnapshot("session-a")?.phase).toBe("live");
    expect(connection.snapshot().phase).toBe("attaching");
    expect(written).toBe(false);
    releaseEvents();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    expect(written).toBe(true);
    expect(socket.sent.map(frame => JSON.parse(frame)).filter(frame => frame.message?.SendMessage?.client_ref === "held-before-live")).toHaveLength(1);
  });

  it("checks the machine immediately when a fresh session recovers during event backoff (cas-9dc6)", async () => {
    const hub = transport(true);
    TransportSocket.instances = [];
    vi.stubGlobal("WebSocket", TransportSocket);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    const connection = supervisor(await storedMachine("session-recovers-first"));
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    const initial = connection.attach("session-a");
    await vi.waitFor(() => expect(TransportSocket.instances).toHaveLength(1));
    const old = TransportSocket.instances[0]!;
    old.open(); old.receive({ proto: 2 });
    await initial;
    old.receive({ channel: "pty:session-a", message: { Welcome: { state: { panes: [] } } } });
    hub.block(true);
    hub.event({ kind: "viewer_lagged" });
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("backoff"));
    old.close(1011);
    hub.block(false);
    const recovering = connection.attach("session-a");
    await vi.waitFor(() => expect(TransportSocket.instances).toHaveLength(2));
    const recovered = TransportSocket.instances[1]!;
    recovered.open(); recovered.receive({ proto: 2 });
    await recovering;
    // The fresh session is reachable, while the machine's independent retry
    // still waits. Its Welcome must prompt a check, not bypass send fencing.
    expect(connection.snapshot().phase).toBe("backoff");
    const streams = hub.streams.length;
    recovered.receive({ channel: "pty:session-a", message: { Welcome: { state: { panes: [] } } } });
    await vi.waitFor(() => expect(hub.streams).toHaveLength(streams + 1), { timeout: 250, interval: 10 });
    expect(connection.snapshot().phase).toBe("live");
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
  });
});
