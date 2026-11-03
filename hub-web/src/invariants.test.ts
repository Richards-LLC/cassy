import { webcrypto } from "node:crypto";
import { afterEach, describe, expect, it, vi } from "vitest";
import { HubConnectionSupervisor, TransientAuthError, type ConnectionState, type HubCallbacks } from "./connection";
import { connectingView } from "./connection-state-view";
import { createDeviceKey, dpopHeaders } from "./dpop";
import { consumePairingFragment } from "./fragment";
import type { StoredMachine } from "./types";

Object.defineProperty(globalThis, "crypto", { value: webcrypto, configurable: true });

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("binding Cassy Cloud browser invariants", () => {
  it("H4-CATALOG-01 consumes pairing fragments synchronously and preserves no capability in the URL", () => {
    const token = "A".repeat(43);
    let replacement = "";
    const location = { hash: `#pair=${token}&hub=machine-1`, pathname: "/", search: "" } as Location;
    const history = { replaceState: (_: unknown, __: string, path: string) => { replacement = path; } } as unknown as History;
    expect(consumePairingFragment(location, history)).toEqual({ token, hubId: "machine-1" });
    expect(replacement).toBe("/");
    expect(replacement).not.toContain(token);
  });

  it("H4-STORAGE-02 creates a non-extractable P-256 signing key and valid proof", async () => {
    const { privateKey, publicKey } = await createDeviceKey();
    expect(privateKey.extractable).toBe(false);
    await expect(crypto.subtle.exportKey("jwk", privateKey)).rejects.toThrow();
    const machine = {
      id: "machine", label: "Machine", baseUrl: "https://hub.example", deviceId: "device",
      credentialId: "credential-id", credential: "opaque-credential", expiresAt: new Date(Date.now() + 60_000).toISOString(),
      scopes: ["machine-read"], publicKey, privateKey,
    } satisfies StoredMachine;
    const headers = await dpopHeaders(machine, "GET", "/v1/machine");
    const [encodedHeader, encodedClaims, encodedSignature] = headers.DPoP.split(".");
    const decode = (value: string) => Buffer.from(value, "base64url");
    expect(JSON.parse(decode(encodedClaims).toString())).toMatchObject({ htm: "GET", htu: "/v1/machine" });
    const imported = await crypto.subtle.importKey("jwk", publicKey, { name: "ECDSA", namedCurve: "P-256" }, false, ["verify"]);
    expect(await crypto.subtle.verify({ name: "ECDSA", hash: "SHA-256" }, imported, decode(encodedSignature), new TextEncoder().encode(`${encodedHeader}.${encodedClaims}`))).toBe(true);
  });

  // Contract: keeps retrying opaque authenticated reads without claiming a pairing refusal.
  // Consumer: Commander application render and event handlers (main.ts), operating the pairing dialog.
  // Historical regression retained; see cas-9d89 inventory for behavioural coverage gaps.
  it("keeps retrying opaque authenticated reads without claiming a pairing refusal", async () => {
    vi.stubGlobal("window", globalThis);
    const { privateKey, publicKey } = await createDeviceKey();
    const machine = {
      id: "machine", label: "Machine", baseUrl: "https://hub.example", deviceId: "device",
      credentialId: "credential-id", credential: "opaque-credential", expiresAt: new Date(Date.now() + 60_000).toISOString(),
      scopes: ["machine-read"], publicKey, privateKey,
    } satisfies StoredMachine;
    const callbacks = {
      onState: vi.fn(), onAuthFailure: vi.fn(), onSessions: vi.fn(), onMachineEvent: vi.fn(),
      onSessionState: vi.fn(), onOutput: vi.fn(), onSocketError: vi.fn(),
    } satisfies HubCallbacks;
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      if (new URL(String(input)).pathname === "/v1/health") {
        return { ok: true, status: 200 };
      }
      throw new TypeError("Failed to fetch");
    });
    vi.stubGlobal("fetch", fetchMock);

    const supervisor = new HubConnectionSupervisor(machine, callbacks);
    supervisor.start();

    await vi.waitFor(() => expect(supervisor.snapshot()).toMatchObject({ phase: "backoff", stage: "auth" }));
    expect(callbacks.onAuthFailure).not.toHaveBeenCalled();
    expect(supervisor.snapshot().authFailure).toBeUndefined();
    expect(callbacks.onState).toHaveBeenCalledWith(expect.objectContaining({ phase: "backoff" }));
    // A successful public probe does not classify an opaque fetch rejection.
    expect(fetchMock.mock.calls.map(([input]) => new URL(String(input)).pathname)).toEqual([
      "/v1/health", "/v1/machine",
    ]);
    // Recovery actions are exercised through the DOM for every auth failure
    // by connection-state-view.test.ts, rather than pinning an enumeration
    // that omitted expired credentials (cas-8e01).
    supervisor.stop();
  });

  it("keeps a health-probe failure in the offline dialing backoff", async () => {
    vi.stubGlobal("window", globalThis);
    const { privateKey, publicKey } = await createDeviceKey();
    const machine = {
      id: "offline-machine", label: "Offline machine", baseUrl: "https://offline.example", deviceId: "device",
      credentialId: "credential-id", credential: "opaque-credential", expiresAt: new Date(Date.now() + 60_000).toISOString(),
      scopes: ["machine-read"], publicKey, privateKey,
    } satisfies StoredMachine;
    const callbacks = {
      onState: vi.fn(), onAuthFailure: vi.fn(), onSessions: vi.fn(), onMachineEvent: vi.fn(),
      onSessionState: vi.fn(), onOutput: vi.fn(), onSocketError: vi.fn(),
    } satisfies HubCallbacks;
    vi.stubGlobal("fetch", vi.fn(async () => { throw new TypeError("Failed to fetch"); }));

    const supervisor = new HubConnectionSupervisor(machine, callbacks);
    supervisor.start();

    await vi.waitFor(() => expect(supervisor.snapshot()).toMatchObject({ phase: "backoff", stage: "dialing" }));
    expect(callbacks.onAuthFailure).not.toHaveBeenCalled();
    supervisor.stop();
  });

  it("fails an engine missing a transport API once, with the reason, instead of retrying it forever", async () => {
    vi.useFakeTimers();
    vi.stubGlobal("window", globalThis);
    const fetchMock = vi.fn(async () => ({ status: 200, ok: true, json: async () => ({}) }));
    vi.stubGlobal("fetch", fetchMock);
    const timeout = (AbortSignal as unknown as { timeout?: unknown }).timeout;
    Reflect.deleteProperty(AbortSignal as unknown as Record<string, unknown>, "timeout");
    try {
      const { privateKey, publicKey } = await createDeviceKey();
      const machine = {
        id: "machine", label: "Machine", baseUrl: "https://hub.example", deviceId: "device",
        credentialId: "credential-id", credential: "opaque-credential", expiresAt: new Date(Date.now() + 60_000).toISOString(),
        scopes: ["pane-read"], publicKey, privateKey,
      } satisfies StoredMachine;
      const callbacks = {
        onState: vi.fn(), onAttachState: vi.fn(), onSessions: vi.fn(), onMachineEvent: vi.fn(),
        onSessionState: vi.fn(), onOutput: vi.fn(), onSocketError: vi.fn(),
      } satisfies HubCallbacks;
      const supervisor = new HubConnectionSupervisor(machine, callbacks);
      const internals = supervisor as unknown as { desired: boolean; attachRetryTimers: Map<string, number> };
      Object.assign(internals, { desired: true, lifecycle: { phase: "live", stage: "live" } });

      await supervisor.attach("factory-a");

      const snapshot = supervisor.attachSnapshot("factory-a");
      expect(snapshot).toMatchObject({ phase: "failed", fatal: true });
      expect(snapshot?.reason).toContain("AbortSignal.timeout");
      expect(snapshot?.reason).toContain("Update to Chrome");
      // The overlay states it and offers the escape hatch on the first frame.
      expect(connectingView(snapshot!, Date.now())).toMatchObject({ step: snapshot?.reason, actionsAvailable: true });
      // No retry is scheduled, and the failure is not misreported as revoked.
      expect(internals.attachRetryTimers.size).toBe(0);
      expect(callbacks.onSocketError).toHaveBeenCalledWith("factory-a", snapshot?.reason);
      expect(callbacks.onSocketError).toHaveBeenCalledTimes(1);
      await vi.advanceTimersByTimeAsync(60_000);
      expect(callbacks.onSocketError).toHaveBeenCalledTimes(1);
      supervisor.stop();
    } finally {
      if (timeout !== undefined) Object.defineProperty(AbortSignal, "timeout", { value: timeout, configurable: true, writable: true });
    }
  });

  it("keeps one connect clock running across machine retries so the 5s and 15s states appear", async () => {
    vi.useFakeTimers();
    vi.stubGlobal("window", globalThis);
    vi.stubGlobal("fetch", vi.fn(async () => { throw new TypeError("Failed to fetch"); }));
    const { privateKey, publicKey } = await createDeviceKey();
    const machine = {
      id: "offline", label: "Offline", baseUrl: "https://offline.example", deviceId: "device",
      credentialId: "credential-id", credential: "opaque-credential", expiresAt: new Date(Date.now() + 600_000).toISOString(),
      scopes: ["machine-read"], publicKey, privateKey,
    } satisfies StoredMachine;
    const states: ConnectionState[] = [];
    const callbacks = {
      onState: (state: ConnectionState) => states.push(state), onSessions: vi.fn(), onMachineEvent: vi.fn(),
      onSessionState: vi.fn(), onOutput: vi.fn(), onSocketError: vi.fn(),
    } satisfies HubCallbacks;
    const supervisor = new HubConnectionSupervisor(machine, callbacks);
    const startedAt = Date.now();
    supervisor.start();
    await vi.advanceTimersByTimeAsync(30_000);

    const latest = supervisor.snapshot();
    // `since` is rewritten by every transition — that is what froze the
    // overlay at 0s — while the connecting anchor holds the true start.
    expect(latest.since).toBeGreaterThan(startedAt);
    expect(latest.connectingSince).toBe(startedAt);
    expect(connectingView(latest, Date.now())).toMatchObject({ actionsAvailable: true });
    expect(connectingView(latest, Date.now()).elapsedSeconds).toBeGreaterThanOrEqual(15);
    expect(states.filter((state) => state.connectingSince !== startedAt)).toHaveLength(0);
    supervisor.stop();
  });

  it("bounds a terminal that opens but never sends its initial session state", async () => {
    vi.useFakeTimers();
    vi.stubGlobal("window", globalThis);
    vi.stubGlobal("fetch", vi.fn(async () => ({ status: 200, ok: true, json: async () => ({ ticket: "unused" }) })));
    class FakeWebSocket {
      static readonly OPEN = 1;
      static readonly CONNECTING = 0;
      static instances: FakeWebSocket[] = [];
      readyState = FakeWebSocket.CONNECTING;
      binaryType = "";
      onopen: (() => void) | null = null;
      onmessage: ((message: MessageEvent) => void) | null = null;
      onclose: ((event: CloseEvent) => void) | null = null;
      onerror: (() => void) | null = null;
      constructor() { FakeWebSocket.instances.push(this); }
      open(): void { this.readyState = FakeWebSocket.OPEN; this.onopen?.(); }
      close(): void { this.readyState = 3; this.onclose?.({ code: 1006 } as CloseEvent); }
      send(): void {}
    }
    vi.stubGlobal("WebSocket", FakeWebSocket);

    const { privateKey, publicKey } = await createDeviceKey();
    const machine = {
      id: "machine", label: "Machine", baseUrl: "https://hub.example", deviceId: "device",
      credentialId: "credential-id", credential: "opaque-credential", expiresAt: new Date(Date.now() + 60_000).toISOString(),
      scopes: ["pane-read"], publicKey, privateKey,
    } satisfies StoredMachine;
    const callbacks = {
      onState: vi.fn(), onAttachState: vi.fn(), onSessions: vi.fn(), onMachineEvent: vi.fn(),
      onSessionState: vi.fn(), onOutput: vi.fn(), onSocketError: vi.fn(),
    } satisfies HubCallbacks;
    const supervisor = new HubConnectionSupervisor(machine, callbacks);
    Object.assign(supervisor, { desired: true, lifecycle: { phase: "live", stage: "live" } });

    await supervisor.attach("factory-a");
    FakeWebSocket.instances[0].open();
    await vi.advanceTimersByTimeAsync(3_000);

    expect(callbacks.onSocketError).toHaveBeenCalledWith(
      "factory-a",
      "Terminal attach opened but sent no session state within 3s. Retrying…",
    );
    expect(callbacks.onAttachState.mock.calls.map(([, state]) => [state.phase, state.stage])).toEqual([
      ["auth", "auth"],
      ["dialing", "dialing"],
      ["attaching", "attaching"],
      ["failed", "attaching"],
      ["backoff", "attaching"],
    ]);
    expect(new Set(callbacks.onAttachState.mock.calls.slice(0, -1).map(([, state]) => state.attachSince)).size).toBe(1);
    expect(supervisor.attachSnapshot("factory-a")?.phase).toBe("backoff");
    supervisor.stop();
  });

  it("multiplexes sessions on one proto-2 socket and routes raw PTY binary frames", async () => {
    vi.stubGlobal("window", globalThis);
    vi.stubGlobal("fetch", vi.fn(async () => ({
      status: 200,
      ok: true,
      json: async () => ({ ticket: "machine-ticket" }),
    })));
    class FakeWebSocket {
      static readonly OPEN = 1;
      static readonly CONNECTING = 0;
      static instances: FakeWebSocket[] = [];
      readyState = FakeWebSocket.CONNECTING;
      binaryType = "";
      sent: string[] = [];
      onopen: (() => void) | null = null;
      onmessage: ((message: MessageEvent) => void) | null = null;
      onclose: ((event: CloseEvent) => void) | null = null;
      onerror: (() => void) | null = null;
      constructor(readonly url: URL) { FakeWebSocket.instances.push(this); }
      open(): void { this.readyState = FakeWebSocket.OPEN; this.onopen?.(); }
      receive(data: string | ArrayBuffer): void { this.onmessage?.({ data } as MessageEvent); }
      close(code = 1000): void { this.readyState = 3; this.onclose?.({ code } as CloseEvent); }
      send(value: string): void { this.sent.push(value); }
    }
    vi.stubGlobal("WebSocket", FakeWebSocket);

    const { privateKey, publicKey } = await createDeviceKey();
    const machine = {
      id: "machine", label: "Machine", baseUrl: "https://hub.example", deviceId: "device",
      credentialId: "credential-id", credential: "opaque-credential", expiresAt: new Date(Date.now() + 60_000).toISOString(),
      scopes: ["pane-read"], publicKey, privateKey,
    } satisfies StoredMachine;
    const callbacks = {
      onState: vi.fn(), onAttachState: vi.fn(), onSessions: vi.fn(), onMachineEvent: vi.fn(),
      onSessionState: vi.fn(), onOutput: vi.fn(), onScrollbackPage: vi.fn(),
      onFlowControlReset: vi.fn(), onSocketError: vi.fn(), onConversationHistory: vi.fn(),
    } satisfies HubCallbacks;
    const supervisor = new HubConnectionSupervisor(machine, callbacks);
    const internals = supervisor as unknown as { desired: boolean; machineMultiplex: boolean };
    Object.assign(internals, { desired: true, lifecycle: { phase: "live", stage: "live" } });
    internals.machineMultiplex = true;

    const firstAttach = supervisor.attach("factory-a");
    await vi.waitFor(() => expect(FakeWebSocket.instances).toHaveLength(1));
    const socket = FakeWebSocket.instances[0];
    socket.open();
    socket.receive(JSON.stringify({ proto: 2, capabilities: ["pty_binary", "machine_multiplex"] }));
    await firstAttach;
    await supervisor.attach("factory-b");
    await supervisor.attach("factory-a");

    expect(FakeWebSocket.instances).toHaveLength(1);
    expect(socket.sent.map((value) => JSON.parse(value))).toEqual(expect.arrayContaining([
      { proto: 2 },
      { channel: "events", subscribe: true },
      // Supervisors only (cas-6261): the hub never streams workers, so the subscribe names none.
      { channel: "pty:factory-a", subscribe: true },
      { channel: "pty:factory-b", subscribe: true },
    ]));
    expect(socket.sent.filter((value) => value === JSON.stringify({ channel: "pty:factory-a", subscribe: true }))).toHaveLength(1);

    socket.receive(JSON.stringify({
      channel: "pty:factory-a",
      message: { Welcome: {
        session_name: "factory-a",
        state: { focused_pane: "supervisor", panes: [{ id: "supervisor", kind: "Supervisor" }] },
        protocol_version: 3,
        capabilities: ["authoritative_pane_keyframes", "paged_scrollback"],
        pane_bootstrap: [{ pane_id: "supervisor", epoch: 1788970165000, cols: 100, rows: 28, scrollback_start_row: 0, scrollback_end_row: 412 }],
      } },
    }));
    // cas-f285: no terminal runs here, so no keyframe is asked for; the pane's
    // rows are read through its announced generation instead.
    expect(socket.sent.some((value) => value.includes("RequestPaneKeyframe"))).toBe(false);
    expect(callbacks.onSessionState).toHaveBeenCalledWith("factory-a", expect.objectContaining({ focused_pane: "supervisor" }), undefined);
    expect(supervisor.paneBootstrap("factory-a", "supervisor")).toMatchObject({ epoch: 1788970165000, scrollback_end_row: 412 });
    expect(supervisor.requestScrollback("factory-a", "supervisor", 1788970165000, 212, 200)).toBe(true);
    expect(socket.sent.map((value) => JSON.parse(value) as Record<string, any>).find((value) => value.message?.ScrollbackRequest)?.message.ScrollbackRequest)
      .toEqual({ pane_id: "supervisor", generation: 1788970165000, start_row: 212, count: 200 });
    const page = { pane_id: "supervisor", generation: 1788970165000, start_row: 212, next_row: 213, rows: [{ screen_row: 212, text: "ready", style_runs: [] }] };
    socket.receive(JSON.stringify({ channel: "pty:factory-a", message: { ScrollbackPage: page } }));
    expect(callbacks.onScrollbackPage).toHaveBeenCalledWith("factory-a", page);
    const historyRequest = socket.sent
      .map((value) => JSON.parse(value) as Record<string, any>)
      .find((value) => value.channel === "pty:factory-a" && value.message?.ConversationHistoryRequest);
    expect(historyRequest?.message.ConversationHistoryRequest).toMatchObject({ limit: 50, device_id: "device" });
    const historyPage = {
      request_id: historyRequest?.message.ConversationHistoryRequest.request_id,
      messages: [{ notification_id: 17, target: "supervisor", text: "Earlier question", state: "acknowledged", stamped: true, device_id: "device", at: "2026-09-21T12:00:00Z" }],
      replies: [{ notification_id: 18, reply_to: 17, message: "Earlier answer", summary: "", device_id: "device", kind: "answer", attachments: [], at: "2026-09-21T12:01:00Z" }],
      has_earlier: false,
    };
    socket.receive(JSON.stringify({ channel: "pty:factory-a", message: { ConversationHistory: historyPage } }));
    expect(callbacks.onConversationHistory).toHaveBeenCalledWith("factory-a", historyPage, { credentialId: "credential-id", generation: 0 });
    const session = new TextEncoder().encode("factory-a");
    const pane = new TextEncoder().encode("supervisor");
    const payload = new Uint8Array([0x1b, 0x5b, 0x48, 0x4f, 0x4b]);
    const frame = new Uint8Array(9 + session.length + pane.length + payload.length);
    frame.set(new TextEncoder().encode("CAS2"));
    frame[4] = 1;
    new DataView(frame.buffer).setUint16(5, session.length);
    new DataView(frame.buffer).setUint16(7, pane.length);
    frame.set(session, 9);
    frame.set(pane, 9 + session.length);
    frame.set(payload, 9 + session.length + pane.length);
    socket.receive(frame.buffer);
    expect(callbacks.onOutput).toHaveBeenCalledWith("factory-a", "supervisor", payload);

    socket.receive(JSON.stringify({ channel: "pty:factory-a", keyframe_required: { skipped: 200 } }));
    expect(callbacks.onFlowControlReset).toHaveBeenCalledWith("factory-a");
    expect(socket.sent.some((value) => value.includes("RequestPaneKeyframe"))).toBe(false);
    supervisor.stop();
  });

  it.each(["stop", "auth-block"] as const)("cancels a scheduled attach retry on %s", async (terminalAction) => {
    vi.useFakeTimers();
    vi.stubGlobal("window", globalThis);
    const fetchMock = vi.fn(async () => ({ status: 200, ok: true, json: async () => ({ ticket: "unused" }) }));
    vi.stubGlobal("fetch", fetchMock);
    const socketOpened = vi.fn();
    class FakeWebSocket {
      static readonly OPEN = 1;
      static readonly CONNECTING = 0;
      readonly readyState = FakeWebSocket.OPEN;
      binaryType = "";
      onopen: (() => void) | null = null;
      onmessage: ((message: MessageEvent) => void) | null = null;
      onclose: ((event: CloseEvent) => void) | null = null;
      onerror: (() => void) | null = null;
      constructor() { socketOpened(); }
      close(): void {}
      send(): void {}
    }
    vi.stubGlobal("WebSocket", FakeWebSocket);

    const { privateKey, publicKey } = await createDeviceKey();
    const machine = {
      id: "machine", label: "Machine", baseUrl: "https://hub.example", deviceId: "device",
      credentialId: "credential-id", credential: "opaque-credential", expiresAt: new Date(Date.now() + 60_000).toISOString(),
      scopes: ["pane-read"], publicKey, privateKey,
    } satisfies StoredMachine;
    const callbacks = {
      onState: vi.fn(), onSessions: vi.fn(), onMachineEvent: vi.fn(),
      onSessionState: vi.fn(), onOutput: vi.fn(), onSocketError: vi.fn(),
    } satisfies HubCallbacks;
    const supervisor = new HubConnectionSupervisor(machine, callbacks);
    const internals = supervisor as unknown as {
      desired: boolean;
      scheduleAttach(session: string): void;
      blockAuthentication(detail: string, session?: string): void;
      attachRetryTimers: Map<string, number>;
      socketAttempts: Map<string, number>;
    };
    Object.assign(internals, { desired: true, lifecycle: { phase: "live", stage: "live" } });
    internals.scheduleAttach("factory-a");
    internals.scheduleAttach("factory-a");

    expect(vi.getTimerCount()).toBe(1);
    if (terminalAction === "stop") supervisor.stop();
    else internals.blockAuthentication("revoked", "factory-a");
    await vi.advanceTimersByTimeAsync(20_000);
    await supervisor.attach("factory-a");

    expect(internals.attachRetryTimers.size).toBe(0);
    expect(internals.socketAttempts.size).toBe(0);
    expect(fetchMock).not.toHaveBeenCalled();
    expect(socketOpened).not.toHaveBeenCalled();
  });
});

/**
 * cas-d636: on soundwave a phone's proofs were signed before it slept and sent
 * when it woke, 266 s later; the hub refused them and every 401 read as a
 * revoked pairing, so Commander went dark until a reload. A refusal now says
 * why: a retryable one is tried again with a fresh proof on the hub's clock,
 * and only a definitive one ends the pairing.
 */
describe("a refused DPoP proof is not a lost pairing (cas-d636)", () => {
  afterEach(() => { vi.unstubAllGlobals(); });

  type Reply = { status: number; ok: boolean; json: () => Promise<unknown>; clone: () => Reply };
  const reply = (status: number, body: unknown = {}): Reply => ({ status, ok: status >= 200 && status < 300, json: async () => body, clone: () => reply(status, body) });
  const claims = (init: RequestInit | undefined): { iat: number; jti: string } => {
    const proof = (init?.headers as Record<string, string>).DPoP;
    return JSON.parse(Buffer.from(proof.split(".")[1]!, "base64url").toString("utf8"));
  };
  async function supervisorWith(replies: (path: string, call: number, init?: RequestInit) => Reply | Promise<never>) {
    vi.stubGlobal("window", globalThis);
    const { privateKey, publicKey } = await createDeviceKey();
    const machine = {
      id: "machine", label: "Machine", baseUrl: "https://hub.example", deviceId: "device",
      credentialId: "credential-id", credential: "opaque-credential", expiresAt: new Date(Date.now() + 86_400_000).toISOString(),
      scopes: ["machine-read"], publicKey, privateKey,
    } satisfies StoredMachine;
    const callbacks = {
      onState: vi.fn(), onAuthFailure: vi.fn(), onSessions: vi.fn(), onMachineEvent: vi.fn(),
      onSessionState: vi.fn(), onOutput: vi.fn(), onSocketError: vi.fn(),
    } satisfies HubCallbacks;
    let calls = 0;
    const fetchMock = vi.fn(async (input: RequestInfo | URL, _init?: RequestInit) => replies(new URL(String(input)).pathname, calls++, _init));
    vi.stubGlobal("fetch", fetchMock);
    return { supervisor: new HubConnectionSupervisor(machine, callbacks), callbacks, fetchMock };
  }

  it("retries a stale proof once on the hub's clock, including clients 1–4 seconds ahead", async () => {
    const hubNow = 1_800_000_000;
    const clock = vi.spyOn(Date, "now");
    try {
      // Recover within the hub's proof window, including small fast clocks.
      for (const ahead of [-60, 1, 2, 3, 4]) {
        clock.mockReturnValue(hubNow * 1000 + ahead * 1000 + 750);
        const { supervisor, fetchMock } = await supervisorWith((_path, _call, init) => claims(init).iat !== hubNow
          ? reply(401, { error: "unauthorized", reason: "stale_proof", retryable: true, server_time: hubNow })
          : reply(200, { schema_version: 1 }));
        await expect(supervisor.request("GET", "/v1/machine")).resolves.toEqual({ schema_version: 1 });
        expect(fetchMock).toHaveBeenCalledTimes(2);
        const [first, second] = fetchMock.mock.calls.map(([, init]) => claims(init));
        expect(first!.iat).toBe(hubNow + ahead);
        expect(second!.iat).toBe(hubNow);
        expect(second!.jti).not.toBe(first!.jti);
        // Subsequent requests retain the correction without another refusal.
        await supervisor.request("GET", "/v1/machine");
        expect(fetchMock).toHaveBeenCalledTimes(3);
      }
    } finally { clock.mockRestore(); }
  });

  it.each([86_400, -365 * 86_400])("refuses an oversized hub clock offset (%s seconds) without poisoning later proofs", async (offset) => {
    const now = 1_800_000_000;
    const clock = vi.spyOn(Date, "now").mockReturnValue(now * 1000 + 250);
    try {
      const { supervisor, fetchMock, callbacks } = await supervisorWith((_path, call) => call === 0
        ? reply(401, { reason: "stale_proof", retryable: true, server_time: now + offset })
        : reply(200, { schema_version: 1 }));
      await expect(supervisor.request("GET", "/v1/machine")).rejects.toThrow(/clocks.*60 seconds/i);
      expect(fetchMock).toHaveBeenCalledTimes(1);
      expect(callbacks.onAuthFailure).not.toHaveBeenCalled();
      await expect(supervisor.request("GET", "/v1/machine")).resolves.toEqual({ schema_version: 1 });
      expect(fetchMock.mock.calls.map(([, init]) => claims(init).iat)).toEqual([now, now]);
      fetchMock.mockImplementation(async (input) => new URL(String(input)).pathname === "/v1/health"
        ? reply(200, { status: "ok" })
        : reply(401, { reason: "stale_proof", retryable: true, server_time: now + offset }));
      supervisor.start();
      try {
        await vi.waitFor(() => expect(callbacks.onState).toHaveBeenCalledWith(expect.objectContaining({
          phase: "backoff", stage: "auth", cause: expect.objectContaining({ code: "clock_skew" }),
        })));
        expect(supervisor.snapshot().reason).toMatch(/clocks.*60 seconds/i);
        expect(callbacks.onAuthFailure).not.toHaveBeenCalled();
      } finally { supervisor.stop(); }
    } finally { clock.mockRestore(); }
  });

  it("reads a definitive refusal as a lost pairing at once, without a retry", async () => {
    const { supervisor, fetchMock } = await supervisorWith(() => reply(401, { reason: "revoked", retryable: false, server_time: Math.floor(Date.now() / 1000) }));
    await expect(supervisor.request("GET", "/v1/machine")).rejects.toMatchObject({ kind: "revoked", message: "pairing was revoked" });
    expect(fetchMock).toHaveBeenCalledTimes(1);
    const expired = await supervisorWith(() => reply(401, { reason: "expired", retryable: false }));
    await expect(expired.supervisor.request("GET", "/v1/machine")).rejects.toMatchObject({ kind: "expired" });
    const key = await supervisorWith(() => reply(401, { reason: "key_mismatch", retryable: false }));
    await expect(key.supervisor.request("GET", "/v1/machine")).rejects.toMatchObject({ kind: "revoked", message: "this browser's key no longer matches the pairing" });
  });

  // cas-2904: a hub's internal failure also answered a bare 401, and reading
  // it as "revoked" purged and permanently fenced a live pairing.
  it("reads a bare 401 as needs pairing, never as a revocation, after one fresh proof", async () => {
    const { supervisor, fetchMock } = await supervisorWith(() => reply(401, { error: "unauthorized" }));
    await expect(supervisor.request("GET", "/v1/machine")).rejects.toMatchObject({ kind: "needs-pairing" });
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it("retries a proof refused twice like a network failure, never as re-pair", async () => {
    const { supervisor, callbacks } = await supervisorWith((path) => path === "/v1/health"
      ? reply(200, { status: "ok" })
      : reply(401, { reason: "stale_proof", retryable: true, server_time: Math.floor(Date.now() / 1000) }));
    await expect(supervisor.request("GET", "/v1/machine")).rejects.toBeInstanceOf(TransientAuthError);
    supervisor.start();
    await vi.waitFor(() => {
      expect(callbacks.onState).toHaveBeenCalledWith(expect.objectContaining({ phase: "backoff", stage: "auth" }));
    });
    expect(callbacks.onAuthFailure).not.toHaveBeenCalled();
    expect(supervisor.snapshot().authFailure).toBeUndefined();
    supervisor.stop();
  });
});
