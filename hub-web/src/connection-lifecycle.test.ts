import { webcrypto } from "node:crypto";
import { afterEach, describe, expect, it, vi } from "vitest";
import { replaceMachineConnection } from "./connection-lifecycle";
import { HubConnectionSupervisor, type ConnectionState, type HubCallbacks } from "./connection";
import { HEARTBEAT_INTERVAL_MS, MACHINE_RETRY_CEILING_MS, STAGE_TIMEOUT_MS } from "./connection-state";
import { createDeviceKey } from "./dpop";
import { ConversationHistory } from "./conversation-history";
import type { MessageQueued, StoredMachine } from "./types";

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
  const requests: { path: string; authorization: string | null; at?: number }[] = [];
  const streams: { signal: AbortSignal; credential: string | null }[] = [];
  let blocked = false;
  let refused = false;
  let stalledRefresh = false;
  let catalogRevision: number | undefined;
  let eventController: ReadableStreamDefaultController<Uint8Array> | undefined;
  vi.stubGlobal("fetch", vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const path = new URL(String(input)).pathname;
    const authorization = new Headers(init?.headers).get("Authorization");
    requests.push({ path, authorization, ...(path === "/v1/auth/websocket-ticket" ? { at: Date.now() } : {}) });
    if (blocked) throw new TypeError("Failed to fetch");
    if (path === "/v1/health") { clock += 24; return new Response("{}"); }
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

// Keep the real signer, but let event delivery get ahead of its asynchronous
// completion deterministically instead of relying on a loaded host (cas-33167).
function pauseSigning(): () => void {
  const sign = crypto.subtle.sign.bind(crypto.subtle);
  let release!: () => void;
  const gate = new Promise<void>(resolve => { release = resolve; });
  vi.spyOn(crypto.subtle, "sign").mockImplementation(async (...args) => {
    await gate;
    return sign(...args);
  });
  return release;
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
  receive(message: unknown): void {
    // Native WebSocket discards incoming frames once close() starts. Keeping
    // onmessage installed cannot make a closed transport deliver a receipt.
    if (this.readyState !== TransportSocket.OPEN) return;
    this.onmessage?.({ data: JSON.stringify(message) } as MessageEvent);
  }
  close(code = 1000): void { this.readyState = 3; this.onclose?.({ code } as CloseEvent); }
  send(value: string): void { this.sent.push(value); }
}

function supervisor(machine: StoredMachine, onState: HubCallbacks["onState"] = () => {}, onMachineEvent: HubCallbacks["onMachineEvent"] = () => {}, onSessions: HubCallbacks["onSessions"] = () => {}): HubConnectionSupervisor {
  const connection = new HubConnectionSupervisor(machine, {
    onState, onSessions, onMachineEvent, onSessionState: () => {},
    onOutput: () => {}, onSocketError: () => {},
  });
  supervisors.push(connection);
  return connection;
}

function networkHints(): EventTarget {
  const hints = new EventTarget();
  vi.stubGlobal("addEventListener", hints.addEventListener.bind(hints));
  vi.stubGlobal("removeEventListener", hints.removeEventListener.bind(hints));
  vi.stubGlobal("document", new EventTarget());
  return hints;
}

describe("Commander live connection lifecycle", () => {
  it("authenticates before loading the catalog and stays live while that catalog is stalled", async () => {
    const hub = transport();
    hub.stallRefresh(true);
    const connection = supervisor(await storedMachine("auth-before-catalog"));
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    await vi.waitFor(() => expect(hub.requests.some(read => read.path === "/v1/sessions")).toBe(true));
    expect(hub.streams).toHaveLength(1);
    expect(connection.snapshot().phase).toBe("live");
  });

  it("retains known hub capabilities when a reconnect's capability read is temporarily unavailable", async () => {
    transport(true);
    const decoded: ReadableStream<string>[] = [];
    const Decoder = TextDecoderStream;
    vi.stubGlobal("TextDecoderStream", class extends Decoder {
      constructor() { super(); decoded.push(this.readable); }
    });
    const fetchNow = globalThis.fetch;
    let unavailable = false;
    vi.stubGlobal("fetch", vi.fn((input: RequestInfo | URL, init?: RequestInit) =>
      unavailable && new URL(String(input)).pathname === "/v1/machine"
        ? Promise.resolve(Response.json({ error: "busy" }, { status: 503 })) : fetchNow(input, init)));
    const onMachineInfo = vi.fn();
    const connection = new HubConnectionSupervisor(await storedMachine("known-capabilities"), {
      onState: () => {}, onSessions: () => {}, onMachineEvent: () => {}, onSessionState: () => {},
      onOutput: () => {}, onSocketError: () => {}, onMachineInfo,
    });
    supervisors.push(connection);
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    const known = onMachineInfo.mock.calls.at(-1)?.[0];
    expect(known.capabilities).toContain("machine_multiplex_v2");
    await vi.waitFor(() => expect(decoded).toHaveLength(1));
    const cancel = ReadableStreamDefaultReader.prototype.cancel;
    let completeCancel!: () => void;
    let cancelStarted!: () => void;
    const cancellation = new Promise<void>(resolve => { completeCancel = resolve; });
    const started = new Promise<void>(resolve => { cancelStarted = resolve; });
    vi.spyOn(ReadableStreamDefaultReader.prototype, "cancel").mockImplementationOnce(async function (this: ReadableStreamDefaultReader<unknown>, reason) {
      cancelStarted();
      await cancellation;
      return cancel.call(this, reason);
    });
    connection.stop();
    // Cancellation can outlive an event-loop turn. The consumer owns its
    // decoded reader until cancellation finishes and finally releases it.
    await started;
    try { expect(decoded[0]!.locked).toBe(true); } finally { completeCancel(); }
    await vi.waitFor(() => expect(decoded[0]!.locked).toBe(false));
    unavailable = true;
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("backoff"));
    expect(onMachineInfo.mock.calls.at(-1)?.[0]).toEqual(known);
  });

  it("settles a resend receipt when an overlapping legacy attach's ticket finishes later (cas-547a)", async () => {
    transport();
    TransportSocket.instances = [];
    vi.stubGlobal("WebSocket", TransportSocket);
    const fetchNow = globalThis.fetch;
    const tickets: Array<() => void> = [];
    vi.stubGlobal("fetch", vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      if (new URL(String(input)).pathname === "/v1/auth/websocket-ticket") {
        await new Promise<void>(resolve => tickets.push(resolve));
      }
      return fetchNow(input, init);
    }));
    const history = new ConversationHistory();
    const connection = new HubConnectionSupervisor(await storedMachine("overlapping-attach"), {
      onState: () => {}, onSessions: () => {}, onMachineEvent: () => {},
      onSessionState: () => {}, onOutput: () => {}, onSocketError: () => {},
      onMessageQueued: (_session, receipt) => { history.acknowledge(receipt); },
      onConversationHistory: (_session, page) => { page.messages.forEach(message => history.hydrateSend(message)); },
    });
    supervisors.push(connection);
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    const first = connection.attach("session-a");
    await vi.waitFor(() => expect(tickets).toHaveLength(1));
    const second = connection.attach("session-a");
    await vi.waitFor(() => expect(tickets).toHaveLength(2));
    tickets[0]!();
    await first;
    const writer = TransportSocket.instances[0]!;
    writer.open();
    writer.receive({ Welcome: { state: { panes: [] }, protocol_version: 3, capabilities: ["conversation_history"] } });
    const text = "Did the Mac tests start?", clientRef = "resend";
    const message = { SendMessage: { target: "supervisor", text, client_ref: clientRef } };
    history.submit(clientRef, "supervisor", text, 1_000);
    expect(connection.send("session-a", message)).toBe(true);
    history.unconfirmSilent(16_000);
    history.reply({ notification_id: 98, reply_to: null, message: "Tests are running on the Mac.", summary: "", device_id: "phone" }, 17_000);
    expect(history.discardRefused(clientRef)).toBe(true);
    history.hold(clientRef, "supervisor", text, 18_000);
    expect(connection.send("session-a", message)).toBe(true);
    history.release(clientRef, 18_000);
    // A second caller passed the pre-ticket socket check while the first was
    // still authenticating. It must not retire the socket that wrote Retry.
    tickets[1]!();
    await second;
    writer.receive({ MessageQueued: { client_ref: clientRef, notification_id: 99, target: "supervisor", stamped: true } });
    const send = history.events.find(event => event.kind === "send");
    expect(send?.kind === "send" && history.showsDeliveryStatus(send.value)).toBe(true);
    expect(TransportSocket.instances).toHaveLength(1);
    expect(history.events.filter(event => event.kind === "send")).toHaveLength(1);
    expect(writer.sent.map(frame => JSON.parse(frame)).filter(frame => frame.SendMessage?.client_ref === clientRef)).toHaveLength(2);
  });
  it.each(["recovery", "stop"])("rejects a legacy ticket completed after %s (cas-547a)", async (change) => {
    const hub = transport();
    const hints = networkHints();
    TransportSocket.instances = [];
    vi.stubGlobal("WebSocket", TransportSocket);
    const fetchNow = globalThis.fetch;
    const tickets: Array<() => void> = [];
    vi.stubGlobal("fetch", vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      if (new URL(String(input)).pathname === "/v1/auth/websocket-ticket") {
        await new Promise<void>(resolve => tickets.push(resolve));
      }
      return fetchNow(input, init);
    }));
    const connection = supervisor(await storedMachine("stale-legacy-ticket"));
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    const oldAttach = connection.attach("session-a");
    await vi.waitFor(() => expect(tickets).toHaveLength(1));
    if (change === "stop") connection.stop();
    else {
      hints.dispatchEvent(new Event("offline"));
      await vi.waitFor(() => expect(connection.snapshot().phase).toBe("backoff"));
      connection.retry();
      // Recovery has abandoned the old generation and started a fresh
      // ticket, which stays pending while the retired ticket finishes.
      await vi.waitFor(() => expect(tickets).toHaveLength(2));
    }
    tickets[0]!();
    await oldAttach;
    expect(TransportSocket.instances).toHaveLength(0);
    if (change === "recovery") {
      tickets[1]!();
      await vi.waitFor(() => expect(TransportSocket.instances).toHaveLength(1));
      const socket = TransportSocket.instances[0]!;
      socket.open();
      socket.receive({ Welcome: { state: { panes: [] }, protocol_version: 3 } });
      expect(connection.send("session-a", { SendMessage: { target: "supervisor", text: "Kept across recovery", client_ref: "kept" } })).toBe(true);
    }
  });
  it.each(["same pairing", "stale attach", "already confirmed", "replaced pairing", "new generation", "stopped"])("fences a resend receipt queued on a retired legacy socket: %s (cas-547a)", async (change) => {
    const hub = transport();
    const hints = networkHints();
    TransportSocket.instances = [];
    vi.stubGlobal("WebSocket", TransportSocket);
    const machine = await storedMachine("retired-resend");
    const history = new ConversationHistory(), queued = vi.fn((_session: string, receipt: MessageQueued) => history.acknowledge(receipt));
    const state = vi.fn(), error = vi.fn();
    const connection = new HubConnectionSupervisor(machine, {
      onState: () => {}, onSessions: () => {}, onMachineEvent: () => {},
      onSessionState: state, onOutput: () => {}, onSocketError: error,
      onMessageQueued: queued,
    });
    supervisors.push(connection);
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    await connection.attach("session-a");
    const writer = TransportSocket.instances[0]!;
    const welcome = { Welcome: { state: { panes: [] }, protocol_version: 3 } };
    writer.open(); writer.receive(welcome);
    const message = { SendMessage: { target: "supervisor", text: "Did the Mac tests start?", client_ref: "resend" } };
    history.submit("resend", "supervisor", message.SendMessage.text, 1_000);
    expect(connection.send("session-a", message)).toBe(true);
    history.unconfirmSilent(16_000);
    history.reply({ notification_id: 98, reply_to: null, message: "Tests are running on the Mac.", summary: "", device_id: "phone" }, 17_000);
    history.discardRefused("resend");
    history.hold("resend", "supervisor", message.SendMessage.text, 18_000);
    expect(connection.send("session-a", message)).toBe(true);
    history.release("resend", 18_000);
    const receipt = { MessageQueued: { client_ref: "resend", notification_id: 99, target: "supervisor", stamped: true } };
    if (change === "already confirmed") {
      writer.receive(receipt);
      expect(queued).toHaveBeenCalledTimes(1);
      queued.mockClear();
    }
    if (change === "stale attach") {
      // Event-only recovery preserves speaking sockets, but a stale writer
      // is retired per session and must keep the same bounded receipt drain.
      vi.useFakeTimers({ toFake: ["Date"] });
      vi.setSystemTime(Date.now() + 20_000);
      hub.event({ kind: "viewer_lagged" });
    } else hints.dispatchEvent(new Event("offline"));
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("backoff"));
    connection.retry();
    await vi.waitFor(() => expect(TransportSocket.instances).toHaveLength(2));
    if (change === "replaced pairing") machine.credentialId = "new-pairing";
    if (change === "new generation") machine.credentialGeneration = 2;
    if (change === "stopped") connection.stop();
    const phase = connection.attachSnapshot("session-a")?.phase;
    const stateCount = state.mock.calls.length, errorCount = error.mock.calls.length;
    // Retired traffic cannot re-open an attach, fail a newer send, or
    // manufacture confirmation for a ref that socket never wrote.
    writer.receive(welcome);
    writer.receive({ Error: { code: "upstream_unavailable", client_ref: "resend" } });
    writer.receive({ MessageQueued: { client_ref: "unknown", notification_id: 99, target: "supervisor", stamped: true } });
    writer.receive({ MessageQueued: { client_ref: "resend", notification_id: 99, target: "other-supervisor", stamped: true } });
    expect(queued).not.toHaveBeenCalled();
    writer.receive(receipt);
    writer.receive({ MessageQueued: { ...receipt.MessageQueued, notification_id: 100 } });
    const accepted = change === "same pairing" || change === "stale attach";
    expect(queued).toHaveBeenCalledTimes(accepted ? 1 : 0);
    const send = history.events.find(event => event.kind === "send");
    expect(send?.kind === "send" && history.showsDeliveryStatus(send.value)).toBe(accepted || change === "already confirmed");
    if (accepted || change === "already confirmed") expect(send?.kind === "send" && send.value.notificationId).toBe(99);
    expect(history.events.filter(event => event.kind === "send")).toHaveLength(1);
    expect(connection.attachSnapshot("session-a")?.phase).toBe(phase);
    expect(state).toHaveBeenCalledTimes(stateCount);
    expect(error).toHaveBeenCalledTimes(errorCount);
  });
  it.each(["deadline", "stop", "authentication refusal"])("closes an undrained retired writer on %s (cas-547a)", async (end) => {
    const hub = transport();
    const hints = networkHints();
    TransportSocket.instances = [];
    vi.stubGlobal("WebSocket", TransportSocket);
    const queued = vi.fn();
    const connection = new HubConnectionSupervisor(await storedMachine("drain-lifetime"), {
      onState: () => {}, onSessions: () => {}, onMachineEvent: () => {},
      onSessionState: () => {}, onOutput: () => {}, onSocketError: () => {},
      onMessageQueued: queued,
    });
    supervisors.push(connection);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    await connection.attach("session-a");
    const writer = TransportSocket.instances[0]!;
    const welcome = { Welcome: { state: { panes: [] }, protocol_version: 3 } };
    writer.open(); writer.receive(welcome);
    expect(connection.send("session-a", { SendMessage: { target: "supervisor", text: "Waiting for its receipt", client_ref: "draining" } })).toBe(true);
    hints.dispatchEvent(new Event("offline"));
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("backoff"));
    connection.retry();
    await vi.waitFor(() => expect(TransportSocket.instances).toHaveLength(2));
    const replacement = TransportSocket.instances[1]!;
    replacement.open(); replacement.receive(welcome);
    expect(writer.readyState).toBe(TransportSocket.OPEN);
    if (end === "stop") connection.stop();
    else if (end === "authentication refusal") {
      hub.refusePairing(true);
      connection.retry();
      await vi.waitFor(() => expect(connection.snapshot().authFailure).toBe("revoked"));
    } else await vi.advanceTimersByTimeAsync(5_000);
    expect(writer.readyState).toBe(3);
    expect(writer.onmessage).toBeNull();
    writer.receive({ MessageQueued: { client_ref: "draining", notification_id: 99, target: "supervisor", stamped: true } });
    expect(queued).not.toHaveBeenCalled();
  });
  it("delivers a stalled catalog's entire burst and joins manual refreshes to its flight (cas-b55b)", async () => {
    const hub = transport();
    const events: Record<string, unknown>[] = [];
    const connection = supervisor(await storedMachine("burst"), () => {}, event => events.push(event));
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    const before = hub.requests.filter(row => row.path === "/v1/sessions").length;
    hub.stallRefresh(true);
    const releaseSigning = pauseSigning();
    for (let i = 1; i <= 100; i++) hub.event({ kind: "session_added", sequence: i });
    await vi.waitFor(() => expect(events).toHaveLength(100));
    releaseSigning();
    const manual = Array.from({ length: 10 }, () => connection.refreshSessions().catch(() => {}));
    await vi.waitFor(() => expect(hub.requests.filter(row => row.path === "/v1/sessions").length - before).toBe(1));
    connection.stop();
    await Promise.all(manual);
  });
  it("refreshes a fleet change after an overlapping heartbeat's older catalog (cas-66ea)", async () => {
    const hub = transport();
    hub.catalogRevision(1);
    let latest: string | undefined;
    const events: Record<string, unknown>[] = [];
    const connection = supervisor(await storedMachine("heartbeat-fleet-change"), () => {}, event => events.push(event), sessions => { latest = sessions[0]?.name; });
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));

    // Freeze the heartbeat's response at the HTTP boundary, before Stop changes
    // the catalog. Signing, event delivery and refresh coalescing stay real.
    const fetchNow = globalThis.fetch;
    let held = false;
    let release!: () => void;
    const pending = new Promise<void>(resolve => { release = resolve; });
    vi.stubGlobal("fetch", async (input: RequestInfo | URL, init?: RequestInit) => {
      const response = await fetchNow(input, init);
      if (new URL(String(input)).pathname === "/v1/sessions" && !held) {
        held = true;
        await pending;
      }
      return response;
    });
    await vi.advanceTimersByTimeAsync(HEARTBEAT_INTERVAL_MS);
    await vi.waitFor(() => expect(held).toBe(true));
    hub.catalogRevision(2);
    hub.event({ kind: "fleet_changed", session: "session-a" });
    await vi.waitFor(() => expect(events).toHaveLength(1));
    expect(latest).toBe("catalog-1");
    release();

    // The event must obtain a post-change catalog without another heartbeat.
    await vi.waitFor(() => expect(latest).toBe("catalog-2"));
  });
  it("refreshes a fleet change even when the overlapping heartbeat's read fails (cas-1966)", async () => {
    const hub = transport();
    hub.catalogRevision(1);
    let latest: string | undefined;
    const events: Record<string, unknown>[] = [];
    const connection = supervisor(await storedMachine("heartbeat-fleet-change-failed"), () => {}, event => events.push(event), sessions => { latest = sessions[0]?.name; });
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));

    // The heartbeat's catalog read is in flight when Stop changes the fleet,
    // and then fails on a flaky network.
    const fetchNow = globalThis.fetch;
    let held = false;
    let fail!: () => void;
    const pending = new Promise<void>(resolve => { fail = resolve; });
    vi.stubGlobal("fetch", async (input: RequestInfo | URL, init?: RequestInit) => {
      if (new URL(String(input)).pathname === "/v1/sessions" && !held) {
        held = true;
        await pending;
        throw new TypeError("Failed to fetch");
      }
      return fetchNow(input, init);
    });
    await vi.advanceTimersByTimeAsync(HEARTBEAT_INTERVAL_MS);
    await vi.waitFor(() => expect(held).toBe(true));
    hub.catalogRevision(2);
    hub.event({ kind: "fleet_changed", session: "session-a" });
    await vi.waitFor(() => expect(events).toHaveLength(1));
    fail();

    // The drained read's failure must not cost the fresh read.
    await vi.waitFor(() => expect(latest).toBe("catalog-2"));
  });
  it("ends the stream without a fresh read when the drained heartbeat read is refused for auth (cas-1966)", async () => {
    const hub = transport();
    hub.catalogRevision(1);
    const events: Record<string, unknown>[] = [];
    const connection = supervisor(await storedMachine("heartbeat-fleet-change-refused"), () => {}, event => events.push(event));
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    const stream = hub.streams.at(-1)!.signal;

    // The heartbeat's catalog read is in flight when the fleet changes, and
    // the hub then refuses the pairing.
    const fetchNow = globalThis.fetch;
    let held = false;
    let release!: () => void;
    const pending = new Promise<void>(resolve => { release = resolve; });
    vi.stubGlobal("fetch", async (input: RequestInfo | URL, init?: RequestInit) => {
      if (new URL(String(input)).pathname === "/v1/sessions" && !held) {
        held = true;
        await pending;
      }
      return fetchNow(input, init);
    });
    await vi.advanceTimersByTimeAsync(HEARTBEAT_INTERVAL_MS);
    await vi.waitFor(() => expect(held).toBe(true));
    const catalogReads = () => hub.requests.filter(row => row.path === "/v1/sessions").length;
    const before = catalogReads();
    let readsAtEnd: number | undefined;
    stream.addEventListener("abort", () => { readsAtEnd = catalogReads(); }, { once: true });
    hub.event({ kind: "fleet_changed", session: "session-a" });
    await vi.waitFor(() => expect(events).toHaveLength(1));
    hub.refusePairing(true);
    release();

    await vi.waitFor(() => expect(stream.aborted).toBe(true));
    expect(readsAtEnd! - before).toBe(1);
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
    const releaseSigning = pauseSigning();
    for (let i = 1; i <= 100; i++) hub.event({ kind: "session_added", sequence: i, revision: 0 });
    await vi.waitFor(() => expect(events).toHaveLength(100));
    releaseSigning();
    await vi.waitFor(() => expect(hub.requests.filter(row => row.path === "/v1/sessions").length - before).toBe(1));
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
    const before = hub.requests.filter(request => request.path === "/v1/health").length;
    hints.rtt = 250;
    hints.downlink = 1;
    hints.effectiveType = "3g";
    hints.dispatchEvent(new Event("change"));
    await vi.advanceTimersByTimeAsync(HEARTBEAT_INTERVAL_MS);
    await vi.waitFor(() => expect(connection.snapshot().missedHeartbeats).toBe(2));
    expect(connection.snapshot().phase).toBe("live");
    expect(connection.snapshot().degraded).toBe(true);
    // The scheduled heartbeat now probes the machine even when its catalog
    // fails; the quality estimate must not add an opportunistic probe.
    expect(hub.requests.filter(request => request.path === "/v1/health")).toHaveLength(before + 1);
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
    const before = hub.requests.filter(request => request.path === "/v1/health").length;
    hub.block(true);
    hints.type = "cellular";
    hints.dispatchEvent(new Event("change"));
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("backoff"));
    expect(hub.requests.filter(request => request.path === "/v1/health")).toHaveLength(before + 1);
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
      await vi.waitFor(() => expect(sessionReads()).toBeGreaterThan(reads));
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
  it("recovers from a sustained network outage without discarding an active pairing (cas-b85a)", async () => {
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
    await vi.waitFor(() => expect(connection.snapshot()).toMatchObject({ phase: "backoff", stage: "dialing" }));
    expect(connection.snapshot().authFailure).toBeUndefined();
    // A longer real outage must still recover within five seconds of the
    // machine returning, even after the retry delay has reached its ceiling.
    for (const delay of [2_000, 4_000, 8_000]) {
      await vi.advanceTimersByTimeAsync(delay);
      await vi.waitFor(() => expect(connection.snapshot().phase).toBe("backoff"));
    }
    hub.block(false);
    const retryIn = connection.snapshot().nextRetryAt! - Date.now();
    expect(retryIn, "leave time for dialing within the five-second recovery window").toBeLessThanOrEqual(4_000);
    // Stop at the retry so the synthetic clock cannot outrun native signing
    // and manufacture a request deadline after the machine has returned.
    await vi.advanceTimersByTimeAsync(retryIn);
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
    await vi.waitFor(() => expect(hub.requests.filter(request => request.path === "/v1/sessions").length).toBeGreaterThan(0));
    const boundary = hub.requests.length;
    await vi.advanceTimersByTimeAsync(HEARTBEAT_INTERVAL_MS);
    await vi.waitFor(() => expect(installed.snapshot().latencyMs).toBe(24));
    await vi.waitFor(() => expect(hub.requests.slice(boundary).map((request) => request.authorization)).toEqual([
      null, hub.streams[1]!.credential,
    ]));
    expect(old.snapshot().phase).toBe("idle");
  });

  it("has no latency before the first heartbeat, then the measured round trip", async () => {
    const hub = transport();
    const connection = supervisor(await storedMachine("latency"));
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    expect(connection.snapshot().latencyMs).toBeUndefined();
    await vi.waitFor(() => expect(hub.requests.filter(request => request.path === "/v1/sessions").length).toBeGreaterThan(0));
    const boundary = hub.requests.length;
    await vi.advanceTimersByTimeAsync(HEARTBEAT_INTERVAL_MS);
    await vi.waitFor(() => expect(connection.snapshot().latencyMs).toBe(24));
    await vi.waitFor(() => expect(hub.requests.slice(boundary).map((request) => request.path)).toEqual(["/v1/health", "/v1/sessions"]));
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
    const reads = hub.requests.filter(row => row.path === "/v1/health").length;
    hints.dispatchEvent(new Event("online"));
    await vi.waitFor(() => expect(hub.requests.filter(row => row.path === "/v1/health")).toHaveLength(reads + 1));
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
      const reads = hub.requests.filter(row => row.path === "/v1/health").length;
      await vi.advanceTimersByTimeAsync(5_000);
      await vi.waitFor(() => expect(hub.requests.filter(row => row.path === "/v1/health")).toHaveLength(reads + 1));
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

  it("pauses session attaches while the machine is down and resumes at two ticket requests per second", async () => {
    const hub = transport();
    TransportSocket.instances = [];
    vi.stubGlobal("WebSocket", TransportSocket);
    vi.spyOn(Math, "random").mockReturnValue(0.5);
    const connection = supervisor(await storedMachine("bounded-recovery-attaches"));
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    hub.block(true);
    hub.endEvents();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("backoff"));
    await Promise.all(["a", "b", "c", "d"].map(session => connection.attach(session)));
    const tickets = () => hub.requests.filter(request => request.path === "/v1/auth/websocket-ticket").length;
    expect(tickets()).toBe(0);
    hub.block(false);
    await vi.advanceTimersByTimeAsync(connection.snapshot().retryInMs!);
    await vi.waitFor(() => expect(tickets()).toBe(2));
    await vi.advanceTimersByTimeAsync(1_000);
    await vi.waitFor(() => expect(tickets()).toBe(4));
    const starts = hub.requests.filter(request => request.path === "/v1/auth/websocket-ticket").map(request => request.at!);
    expect(starts[2]! - starts[0]!).toBeGreaterThanOrEqual(1_000);
    expect(starts[3]! - starts[1]!).toBeGreaterThanOrEqual(1_000);
  });

  it.each(["session_not_found", "daemon_offline"])("reattaches after a replacement is absent between Closed and session_added (%s, cas-d53c)", async (code) => {
    transport(true);
    TransportSocket.instances = [];
    vi.stubGlobal("WebSocket", TransportSocket);
    vi.spyOn(Math, "random").mockReturnValue(0.5);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    const connection = supervisor(await storedMachine("replacement-gap"));
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    const attached = connection.attach("session-a");
    await vi.waitFor(() => expect(TransportSocket.instances).toHaveLength(1));
    const socket = TransportSocket.instances[0]!;
    socket.open(); socket.receive({ proto: 2 });
    await attached;
    const welcome = { channel: "pty:session-a", message: { Welcome: { state: { panes: [] } } } };
    socket.receive(welcome);
    expect(connection.hasLiveAttach("session-a")).toBe(true);
    const subscriptions = () => socket.sent.map(frame => JSON.parse(frame))
      .filter(frame => frame.channel === "pty:session-a" && frame.subscribe);

    socket.receive({ channel: "pty:session-a", closed: true });
    await vi.advanceTimersByTimeAsync(1_000);
    expect(subscriptions()).toHaveLength(2);
    // The real catalog has removed the old daemon but not yet found its replacement.
    socket.receive({ channel: "pty:session-a", error: { code } });
    socket.receive({ channel: "events", event: { kind: "session_added", session: "session-a", sequence: 1 } });
    expect(connection.hasLiveAttach("session-a")).toBe(false);
    await vi.advanceTimersByTimeAsync(2_000);
    expect(subscriptions()).toHaveLength(3);
    socket.receive(welcome);
    expect(connection.hasLiveAttach("session-a")).toBe(true);
    expect(TransportSocket.instances).toHaveLength(1);
    expect(connection.send("session-a", { SendMessage: { target: "supervisor", text: "The replacement is here" } })).toBe(true);
  });

  it("keeps a responsive machine live when the session catalog fails under load", async () => {
    transport(true);
    TransportSocket.instances = [];
    vi.stubGlobal("WebSocket", TransportSocket);
    const connection = supervisor(await storedMachine("slow-catalog-health"));
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    const attached = connection.attach("session-a");
    await vi.waitFor(() => expect(TransportSocket.instances).toHaveLength(1));
    const socket = TransportSocket.instances[0]!;
    socket.open();
    socket.receive({ proto: 2, capabilities: ["pty_binary", "machine_multiplex"] });
    await attached;
    const fetchNow = globalThis.fetch;
    vi.stubGlobal("fetch", vi.fn((input: RequestInfo | URL, init?: RequestInit) =>
      new URL(String(input)).pathname === "/v1/sessions"
        ? Promise.resolve(new Response("catalog busy", { status: 503 }))
        : fetchNow(input, init)));
    let pings = 0;
    vi.spyOn(socket, "send").mockImplementation(value => {
      const frame = JSON.parse(value);
      if (frame.channel === "health") {
        pings += 1;
        socket.receive({ channel: "health", pong: frame.ping });
      }
    });
    for (let beat = 0; beat < 6; beat += 1) {
      await vi.advanceTimersByTimeAsync(HEARTBEAT_INTERVAL_MS);
      expect(pings, "catalog work must not prevent the socket liveness probe").toBe(beat + 1);
      expect(connection.snapshot()).toMatchObject({ phase: "live", degraded: false, missedHeartbeats: 0 });
    }
  });

  it("does not subscribe a session that ends while waiting for its attach rate slot", async () => {
    transport(true);
    TransportSocket.instances = [];
    vi.stubGlobal("WebSocket", TransportSocket);
    const connection = supervisor(await storedMachine("ended-rate-slot"));
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    const attached = connection.attach("session-a");
    await vi.waitFor(() => expect(TransportSocket.instances).toHaveLength(1));
    const socket = TransportSocket.instances[0]!;
    socket.open(); socket.receive({ proto: 2 });
    await attached;
    const queued = connection.attach("session-b");
    connection.setSessionEnded("session-b", true);
    await vi.advanceTimersByTimeAsync(1_000);
    await queued;
    expect(socket.sent.map(frame => JSON.parse(frame)).filter(frame => frame.channel === "pty:session-b" && frame.subscribe)).toEqual([]);
  });

  it.each([
    { pong: true, health: false },
    { pong: false, health: true },
    { pong: false, health: false },
  ])("requires both socket and health failure before declaring loss (pong=$pong, health=$health)", async ({ pong, health }) => {
    transport(true);
    TransportSocket.instances = [];
    vi.stubGlobal("WebSocket", TransportSocket);
    const connection = supervisor(await storedMachine("independent-health-signals"));
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    const attached = connection.attach("session-a");
    await vi.waitFor(() => expect(TransportSocket.instances).toHaveLength(1));
    const socket = TransportSocket.instances[0]!;
    socket.open(); socket.receive({ proto: 2 });
    await attached;
    const fetchNow = globalThis.fetch;
    vi.stubGlobal("fetch", vi.fn((input: RequestInfo | URL, init?: RequestInit) =>
      !health && new URL(String(input)).pathname === "/v1/health"
        ? Promise.reject(new TypeError("health route down")) : fetchNow(input, init)));
    vi.spyOn(socket, "send").mockImplementation(value => {
      const frame = JSON.parse(value);
      if (pong && frame.channel === "health") socket.receive({ channel: "health", pong: frame.ping });
    });
    for (let beat = 0; beat < 5; beat += 1) await vi.advanceTimersByTimeAsync(HEARTBEAT_INTERVAL_MS);
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe(pong || health ? "live" : "backoff"));
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
    // Socket latency excludes the independent 17ms catalog refresh.
    expect(connection.snapshot().latencyMs).toBe(24);
  });
});

/** A half-open socket: close() starts the closing handshake, which never completes, so onclose never fires. */
class HalfOpenSocket extends TransportSocket {
  static override instances: HalfOpenSocket[] = [];
  constructor(url: URL) { super(url); HalfOpenSocket.instances.push(this); }
  override close(): void { this.readyState = 2; }
}

// cas-5dbc: after an outage the legacy attach chain could die, leaving the
// session "Reconnecting" with no further attempt (HUB-J4, 1 in ~300 runs).
// Every path that ends an attempt must leave the next one scheduled.
describe("a session's attach chain always has its next attempt (cas-5dbc)", () => {
  const timers = { toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval"] as ("setTimeout" | "clearTimeout" | "setInterval" | "clearInterval")[] };

  // Advancing deadlines starts the retry, but its ticket still awaits native
  // WebCrypto. Wait for the socket as well as draining the browser timers.
  it("replaces a dial stuck on a half-open socket on the backoff, without waiting for its close", async () => {
    transport();
    HalfOpenSocket.instances = [];
    vi.stubGlobal("WebSocket", HalfOpenSocket);
    const connection = supervisor(await storedMachine("half-open-dial"));
    vi.useFakeTimers(timers);
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    await connection.attach("session-a");
    expect(HalfOpenSocket.instances).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(STAGE_TIMEOUT_MS.dialing + MACHINE_RETRY_CEILING_MS);
    await vi.waitFor(() => expect(HalfOpenSocket.instances.length, "a new attempt after the dial timed out").toBeGreaterThan(1));
  });

  it("replaces an opened socket that sends no session state and never finishes closing", async () => {
    transport();
    HalfOpenSocket.instances = [];
    vi.stubGlobal("WebSocket", HalfOpenSocket);
    const connection = supervisor(await storedMachine("half-open-ready"));
    vi.useFakeTimers(timers);
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    await connection.attach("session-a");
    HalfOpenSocket.instances[0]!.open();
    await vi.advanceTimersByTimeAsync(STAGE_TIMEOUT_MS.attaching + MACHINE_RETRY_CEILING_MS);
    await vi.waitFor(() => expect(HalfOpenSocket.instances.length, "a new attempt after the ready deadline").toBeGreaterThan(1));
  });

  it("finishes replacing the timed-out socket after slow retry signing (cas-ec55)", async () => {
    transport();
    HalfOpenSocket.instances = [];
    vi.stubGlobal("WebSocket", HalfOpenSocket);
    vi.spyOn(Math, "random").mockReturnValue(0.5);
    const connection = supervisor(await storedMachine("slow-retry-signing"));
    vi.useFakeTimers(timers);
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    await connection.attach("session-a");

    // Native WebCrypto can still be working after the fake timers drain.
    // Hold the retry's credential digest, then perform the real signing.
    const digest = crypto.subtle.digest.bind(crypto.subtle);
    let releaseSigning!: () => void;
    const signing = new Promise<void>(resolve => { releaseSigning = resolve; });
    vi.spyOn(crypto.subtle, "digest").mockImplementationOnce(async (...args) => {
      await signing;
      return digest(...args);
    });
    const retired = HalfOpenSocket.instances[0]!;
    retired.open();
    await vi.advanceTimersByTimeAsync(STAGE_TIMEOUT_MS.attaching + MACHINE_RETRY_CEILING_MS);
    expect(retired.readyState).toBe(2);
    expect(connection.attachSnapshot("session-a")?.phase).toBe("auth");
    expect(HalfOpenSocket.instances).toHaveLength(1);
    releaseSigning();
    await vi.waitFor(() => expect(HalfOpenSocket.instances.length, "a replacement after retry signing completes").toBeGreaterThan(1));
  });

  it("reattaches a session whose attempt was abandoned while its ticket was in flight", async () => {
    transport();
    TransportSocket.instances = [];
    vi.stubGlobal("WebSocket", TransportSocket);
    const fetchNow = globalThis.fetch;
    const tickets: Array<() => void> = [];
    vi.stubGlobal("fetch", vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      if (new URL(String(input)).pathname === "/v1/auth/websocket-ticket" && tickets.length === 0) {
        await new Promise<void>(resolve => tickets.push(resolve));
      }
      return fetchNow(input, init);
    }));
    const connection = supervisor(await storedMachine("abandoned-ticket"));
    vi.useFakeTimers(timers);
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    const attempt = connection.attach("session-a");
    await vi.waitFor(() => expect(tickets).toHaveLength(1));
    // The machine's sockets are dropped (lost heartbeats, a half-open
    // machine socket) while this attempt awaits its ticket: the attempt
    // ends without a socket, and no retry was pending.
    (connection as unknown as { abandonSockets(reason: string): void }).abandonSockets("Lost connection to the machine");
    tickets[0]!();
    await attempt;
    expect(TransportSocket.instances).toHaveLength(0);
    await vi.advanceTimersByTimeAsync(HEARTBEAT_INTERVAL_MS + MACHINE_RETRY_CEILING_MS);
    await vi.waitFor(() => expect(TransportSocket.instances.length, "the live machine's heartbeat reattached the session").toBeGreaterThan(0));
  });
});
