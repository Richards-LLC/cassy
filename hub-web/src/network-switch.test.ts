import { afterEach, describe, expect, it, vi } from "vitest";
import { HubConnectionSupervisor, messageRejection, TransientAuthError, UPSTREAM_STREAK_SETTLE_MS, type HubCallbacks } from "./connection";
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
  it("wakes a network backoff immediately without resetting its failure streak (cas-49cc)", () => {
    vi.stubGlobal("window", globalThis);
    const { internals } = supervisor();
    (internals as unknown as { desired: boolean }).desired = true;
    internals.lifecycle = { phase: "backoff", stage: "dialing" };
    internals.attempt = 5;
    internals.retryTimer = setTimeout(() => {}, 60_000) as unknown as number;
    const connect = vi.spyOn(internals, "connect").mockResolvedValue();

    internals.networkChanged();

    expect(connect).toHaveBeenCalledTimes(1);
    expect(internals.attempt).toBe(5);
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

  it("refuses a supervisor message while the machine socket is in doubt, so it is held, not lost", () => {
    vi.stubGlobal("window", globalThis);
    const { sup, internals } = supervisor();
    const machine = fakeSocket();
    internals.machineSocket = machine;
    internals.machineSocketReady = true;
    (internals as unknown as { probePingId?: number }).probePingId = 17;
    expect(sup.send("patient-pelican-9", { SendMessage: { target: "t", text: "x" } }, "ref-1")).toBe(false);
    expect(machine.send).not.toHaveBeenCalled();
    // Reads and keystrokes are not held.
    expect(sup.send("patient-pelican-9", "GetState")).toBe(true);
  });

  it("holds a supervisor message while the machine is unsteady, and lets it go when a heartbeat answers (cas-a6f0)", async () => {
    vi.stubGlobal("window", globalThis);
    const onAttachState = vi.fn();
    const { sup, internals } = supervisor({ onAttachState });
    const machine = fakeSocket();
    internals.machineSocket = machine;
    internals.machineSocketReady = true;
    const fields = internals as unknown as {
      missedHeartbeats: number;
      healthPing?: { id: number; startedAt: number };
      attachLifecycles: Map<string, { phase: string }>;
      handleMachineMessage(input: string): Promise<void>;
    };
    internals.lifecycle = { phase: "live", stage: "live" };
    fields.attachLifecycles.set("patient-pelican-9", { phase: "live" });
    const message = { SendMessage: { target: "t", text: "x" } };

    // One unanswered heartbeat is not yet unsteady: the message goes.
    fields.missedHeartbeats = 1;
    expect(sup.holdsMessages()).toBe(false);
    expect(sup.send("patient-pelican-9", message, "ref-1")).toBe(true);

    // Two are: the header says Unsteady, and the message is held, not lost.
    fields.missedHeartbeats = 2;
    machine.send.mockClear();
    expect(sup.holdsMessages()).toBe(true);
    expect(sup.send("patient-pelican-9", message, "ref-2")).toBe(false);
    expect(machine.send).not.toHaveBeenCalled();
    expect(sup.send("patient-pelican-9", "GetState"), "reads and keystrokes are not held").toBe(true);

    // The heartbeat answers: steady again, and every live attach is announced
    // so the page sends what it held.
    fields.healthPing = { id: 41, startedAt: performance.now() };
    onAttachState.mockClear();
    await fields.handleMachineMessage(JSON.stringify({ channel: "health", pong: 41 }));
    expect(sup.holdsMessages()).toBe(false);
    expect(onAttachState).toHaveBeenCalledWith("patient-pelican-9", expect.objectContaining({ phase: "live" }));
  });

  it("does not announce attaches when a heartbeat answers a steady machine", async () => {
    vi.stubGlobal("window", globalThis);
    const onAttachState = vi.fn();
    const { internals } = supervisor({ onAttachState });
    const fields = internals as unknown as { healthPing?: { id: number; startedAt: number }; attachLifecycles: Map<string, { phase: string }>; handleMachineMessage(input: string): Promise<void> };
    internals.lifecycle = { phase: "live", stage: "live" };
    fields.attachLifecycles.set("patient-pelican-9", { phase: "live" });
    fields.healthPing = { id: 42, startedAt: performance.now() };
    await fields.handleMachineMessage(JSON.stringify({ channel: "health", pong: 42 }));
    expect(onAttachState).not.toHaveBeenCalled();
  });

  it("opens no machine socket for an opening abandoned while its ticket was on the way (cas-7b31)", async () => {
    vi.stubGlobal("window", globalThis);
    const opened: string[] = [];
    vi.stubGlobal("WebSocket", class { static CONNECTING = 0; static OPEN = 1; readyState = 0; constructor(url: string) { opened.push(String(url)); } close() {} send() {} });
    const { internals } = supervisor();
    (internals as unknown as { desired: boolean; machine: { baseUrl: string } }).desired = true;
    let answer!: (value: unknown) => void;
    vi.spyOn(internals, "request").mockReturnValue(new Promise((resolve) => { answer = resolve; }));
    const opening = internals.openMachineSocket("patient-pelican-9");
    // The network changed: every socket, and this opening, is abandoned.
    internals.abandonSockets("Reconnected after the network changed");
    answer({ ticket: "late" });
    expect(await opening).toBe(true);
    expect(opened, "the stale opening opened nothing").toEqual([]);
    // A fresh opening still opens its socket.
    vi.spyOn(internals, "request").mockResolvedValue({ ticket: "fresh" });
    void internals.openMachineSocket("patient-pelican-9");
    await vi.waitFor(() => expect(opened).toHaveLength(1));
    expect(opened[0]).toContain("ticket=fresh");
  });

  it("beats no heartbeat after a pairing refusal, so the machine never reads live again by itself (cas-7b31)", async () => {
    vi.stubGlobal("window", globalThis);
    const onState = vi.fn();
    const { internals } = supervisor({ onState });
    const fields = internals as unknown as { desired: boolean; heartbeat(): Promise<void>; startHeartbeat(): void; heartbeatTimer?: number; blockAuthentication(kind: string, detail: string): void };
    fields.desired = true;
    internals.lifecycle = { phase: "live", stage: "live" };
    fields.startHeartbeat();
    vi.spyOn(internals, "request").mockRejectedValue(new Error("401"));
    fields.blockAuthentication("revoked", "pairing was revoked");
    expect(fields.heartbeatTimer, "the heartbeat stops with the refusal").toBeUndefined();
    onState.mockClear();
    await fields.heartbeat();
    expect(onState, "a stray beat changes nothing").not.toHaveBeenCalled();
    expect(internals.lifecycle).toMatchObject({ phase: "failed", authFailure: "revoked" });
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

  it("settles in-flight sends as Not confirmed when no receipt can come, leaving held ones alone (cas-a6f0)", () => {
    const history = new ConversationHistory();
    history.submit("on-wire", "patient-pelican-9", "Sent before the revoke", 1_000);
    history.hold("held", "patient-pelican-9", "Waiting", 2_000);
    expect(history.unconfirmInFlight(3_000)).toEqual(["on-wire"]);
    const states = Object.fromEntries(history.events.flatMap((event) => event.kind === "send" ? [[event.value.id, event.value.state]] : []));
    expect(states).toEqual({ "on-wire": "unconfirmed", held: "sending" });
    expect(history.unconfirmInFlight(4_000), "once").toEqual([]);
  });

  it("turns a held send that never went out into Not sent", () => {
    const history = new ConversationHistory();
    history.hold("send-2", "patient-pelican-9", "Hello?", 1_000);
    expect(history.reject("send-2", "Not sent: lost connection to Atlas · Linux.")).toBe(true);
    expect(history.preview()).toBe("Not sent: Hello?");
  });
});

// cas-0653: the hub answers `upstream_unavailable` (retryable) when the
// session's daemon link is missing; the send is held and goes out once on the
// next live attach instead of reading "Not sent".
describe("a send the hub could not forward (cas-0653)", () => {
  it("reads the code and retry hint from both socket shapes, and nothing else as retryable", () => {
    expect(messageRejection({ code: "upstream_unavailable", retryable: true, client_ref: "send-1" }))
      .toEqual({ code: "upstream_unavailable", retryable: true });
    expect(messageRejection("upstream_unavailable", { error: "upstream_unavailable", retryable: true }))
      .toEqual({ code: "upstream_unavailable", retryable: true });
    expect(messageRejection({ code: "forbidden", client_ref: "send-1" })).toEqual({ code: "forbidden", retryable: false });
    expect(messageRejection("forbidden", { error: "forbidden" })).toEqual({ code: "forbidden", retryable: false });
    expect(messageRejection(undefined)).toEqual({ retryable: false });
  });

  it("puts a refused, unreceipted send back on hold with no receipt clock", () => {
    const history = new ConversationHistory();
    history.submit("send-3", "patient-pelican-9", "Do the burn down", 1_000);
    const held = history.rehold("send-3");
    expect(held).toMatchObject({ id: "send-3", text: "Do the burn down", held: true, state: "sending" });
    expect(held && "sentAt" in held ? held.sentAt : undefined).toBeUndefined();
    expect(history.nextReceiptCheck(5_000), "a held send cannot time out").toBeUndefined();
    expect(history.release("send-3", 9_000), "it goes out once more").toBe(true);

    history.acknowledge({ client_ref: "send-3", notification_id: 812, target: "patient-pelican-9", stamped: true });
    expect(history.rehold("send-3"), "a delivered send is never held again").toBeUndefined();
    expect(history.rehold("unknown")).toBeUndefined();
  });

  it("hands a retryable refusal over with its hint, and reattaches when the hub closes the stream", () => {
    vi.stubGlobal("window", globalThis);
    const onMessageRejected = vi.fn();
    const { sup, internals } = supervisor({ onMessageRejected, onSocketError: vi.fn(), onAttachState: vi.fn() });
    const machine = internals as unknown as { machineSubscriptions: Set<string>; desired: boolean; attachRetryTimers: Map<string, number>; handleMachineMessage(input: string): Promise<void> };
    machine.desired = true;
    machine.machineSubscriptions.add("factory-a");
    void machine.handleMachineMessage(JSON.stringify({
      channel: "pty:factory-a",
      error: { code: "upstream_unavailable", retryable: true, message: "The session's daemon connection is reconnecting.", client_ref: "send-4" },
    }));
    expect(onMessageRejected).toHaveBeenCalledWith("factory-a", "send-4", "The session's daemon connection is reconnecting.", { code: "upstream_unavailable", retryable: true });
    // The hub drops the session's stream right after (cas-0653).
    void machine.handleMachineMessage(JSON.stringify({ channel: "pty:factory-a", closed: true }));
    expect(machine.machineSubscriptions.has("factory-a"), "the next attach subscribes afresh").toBe(false);
    expect(machine.attachRetryTimers.has("factory-a"), "an attach is scheduled").toBe(true);
    sup.stop();
  });

  it("reports a refusal that is not retryable as before", () => {
    vi.stubGlobal("window", globalThis);
    const onMessageRejected = vi.fn();
    const { internals } = supervisor({ onMessageRejected, onSocketError: vi.fn(), onAttachState: vi.fn() });
    const machine = internals as unknown as { machineSubscriptions: Set<string>; desired: boolean; handleMachineMessage(input: string): Promise<void> };
    machine.desired = true;
    machine.machineSubscriptions.add("factory-a");
    void machine.handleMachineMessage(JSON.stringify({ channel: "pty:factory-a", error: { code: "forbidden", client_ref: "send-5" } }));
    expect(onMessageRejected).toHaveBeenCalledWith("factory-a", "send-5", "forbidden", { code: "forbidden", retryable: false });
    expect(machine.machineSubscriptions.has("factory-a")).toBe(true);
  });
});

// cas-a355: while the daemon link stays down, each live attach used to resend
// the held message and be refused again about once a second. And on the
// legacy socket, a message written after the refused one was never answered.
describe("held-send polish after cas-0653 (cas-a355)", () => {
  type Refusing = {
    desired: boolean;
    machineSubscriptions: Set<string>;
    attachRetryTimers: Map<string, number>;
    socketAttempts: Map<string, number>;
    sockets: Map<string, unknown>;
    machineSocketReady: boolean;
    handleMachineMessage(input: string): Promise<void>;
    handleDaemonMessage(session: string, input: string): Promise<void>;
  };
  const refused = (ref: string) => JSON.stringify({ channel: "pty:factory-a", error: { code: "upstream_unavailable", retryable: true, message: "The session's daemon connection is reconnecting.", client_ref: ref } });
  const closed = JSON.stringify({ channel: "pty:factory-a", closed: true });

  it("backs the reattach off 1, 2, 4, then 8 s while the upstream keeps refusing, and starts afresh once a send lands", async () => {
    vi.stubGlobal("window", globalThis);
    vi.spyOn(Math, "random").mockReturnValue(0.5);
    const onMessageRejected = vi.fn();
    const onAttachState = vi.fn();
    const { sup, internals } = supervisor({ onMessageRejected, onAttachState, onSocketError: vi.fn(), onMessageQueued: vi.fn() });
    const machine = internals as unknown as Refusing;
    machine.desired = true;
    const delays: number[] = [];
    const cycle = async (ref: string) => {
      // A live attach (Welcome) resets the attempt count, as it does in the page.
      machine.socketAttempts.set("factory-a", 0);
      machine.machineSubscriptions.add("factory-a");
      await machine.handleMachineMessage(refused(ref));
      await machine.handleMachineMessage(closed);
      const backoff = onAttachState.mock.calls.filter(([, snapshot]) => snapshot.phase === "backoff").at(-1)?.[1];
      delays.push(backoff?.retryInMs);
      for (const timer of machine.attachRetryTimers.values()) clearTimeout(timer);
      machine.attachRetryTimers.clear();
    };
    for (let refusal = 1; refusal <= 5; refusal += 1) await cycle(`send-${refusal}`);
    expect(delays, "each refusal waits longer, capped at about 8 s").toEqual([1_000, 2_000, 4_000, 8_000, 8_000]);
    expect(onMessageRejected).toHaveBeenCalledTimes(5);

    await machine.handleMachineMessage(JSON.stringify({ channel: "pty:factory-a", message: { MessageQueued: { notification_id: 7, target: "patient-pelican-9", client_ref: "send-6" } } }));
    await cycle("send-7");
    expect(delays.at(-1), "a delivered send resets the backoff").toBe(1_000);
    sup.stop();
    vi.restoreAllMocks();
  });

  // cas-4ce5: a session attach held down through several failures kept doubling
  // (16 s at the fifth, 30 s later), so a machine back after a long outage
  // stayed "Reconnecting" well past the 10 s the catalog promises (HUB-J11).
  it("caps the session attach backoff at the machine retry ceiling", () => {
    vi.useFakeTimers();
    vi.stubGlobal("window", globalThis);
    vi.spyOn(Math, "random").mockReturnValue(0.5);
    const onAttachState = vi.fn();
    const { sup, internals } = supervisor({ onAttachState });
    const machine = internals as unknown as Refusing & { scheduleAttach(session: string): void };
    machine.desired = true;
    const delays: Array<number | undefined> = [];
    for (let failure = 0; failure < 7; failure += 1) {
      machine.scheduleAttach("factory-a");
      delays.push(onAttachState.mock.calls.filter(([, snapshot]) => snapshot.phase === "backoff").at(-1)?.[1]?.retryInMs);
      for (const timer of machine.attachRetryTimers.values()) clearTimeout(timer);
      machine.attachRetryTimers.clear();
    }
    expect(delays).toEqual([1_000, 2_000, 4_000, 8_000, MACHINE_RETRY_CEILING_MS, MACHINE_RETRY_CEILING_MS, MACHINE_RETRY_CEILING_MS]);
    sup.stop();
  });

  // cas-2036 (cas-a355 QA N3): only a receipt used to reset the streak, so a
  // held send that expired unsent left the next drop starting at 8 s.
  it("starts the backoff afresh once the session stays live with no refusal, delivered send or not", async () => {
    vi.useFakeTimers();
    vi.stubGlobal("window", globalThis);
    vi.spyOn(Math, "random").mockReturnValue(0.5);
    const onAttachState = vi.fn();
    const known: Record<string, unknown> = { onAttachState, onState: vi.fn() };
    const callbacks = new Proxy(known, { get: (target, key: string) => (target[key] ??= vi.fn()) });
    const sup = new HubConnectionSupervisor({ baseUrl: "https://atlas.test" } as StoredMachine, callbacks as unknown as HubCallbacks);
    const machine = sup as unknown as Refusing & { handleDaemonObject(session: string, message: Record<string, unknown>): void };
    machine.desired = true;
    machine.machineSocketReady = true;
    const live = () => machine.handleDaemonObject("factory-a", { Welcome: { state: { panes: [], cols: 80, rows: 24 }, scrollback: {}, protocol_version: 1 } });
    const refusedThenClosed = async (ref: string): Promise<number | undefined> => {
      machine.machineSubscriptions.add("factory-a");
      await machine.handleMachineMessage(refused(ref));
      await machine.handleMachineMessage(closed);
      const delay = onAttachState.mock.calls.filter(([, snapshot]) => snapshot.phase === "backoff").at(-1)?.[1]?.retryInMs;
      for (const timer of machine.attachRetryTimers.values()) clearTimeout(timer);
      machine.attachRetryTimers.clear();
      return delay;
    };
    // The upstream stays gone: every live attach is refused again at once.
    const delays: Array<number | undefined> = [];
    for (const ref of ["send-1", "send-2", "send-3"]) { live(); delays.push(await refusedThenClosed(ref)); }
    expect(delays).toEqual([1_000, 2_000, 4_000]);
    // Live again and refused just inside the settle window: still gone, so it keeps backing off.
    live();
    vi.advanceTimersByTime(UPSTREAM_STREAK_SETTLE_MS - 1);
    expect(await refusedThenClosed("send-4")).toBe(8_000);
    // Live and quiet for the settle window, nothing delivered (the held send
    // expired unsent): the upstream is back, and the next drop starts at 1 s.
    live();
    vi.advanceTimersByTime(UPSTREAM_STREAK_SETTLE_MS);
    expect(await refusedThenClosed("send-5")).toBe(1_000);
    sup.stop();
    vi.restoreAllMocks();
  });

  it("refuses, as retryable, every message written to the legacy socket after the refused one, and nothing before it", async () => {
    vi.stubGlobal("window", globalThis);
    const onMessageRejected = vi.fn();
    const { sup, internals } = supervisor({ onMessageRejected, onSocketError: vi.fn() });
    const machine = internals as unknown as Refusing;
    machine.desired = true;
    machine.machineSocketReady = false;
    internals.lifecycle = { phase: "live", stage: "live" };
    const socket = fakeSocket();
    machine.sockets.set("factory-a", socket);
    for (const ref of ["send-1", "send-2", "send-3"]) {
      expect(sup.send("factory-a", { SendMessage: { target: "patient-pelican-9", message: ref, client_ref: ref } })).toBe(true);
    }
    // send-1 was forwarded (its receipt is pending); send-2 is refused and
    // the hub closes the socket, so send-3 is never read.
    await machine.handleDaemonMessage("factory-a", JSON.stringify({ error: "upstream_unavailable", retryable: true, message: "The session's daemon connection is reconnecting.", client_ref: "send-2" }));
    const rejection = { code: "upstream_unavailable", retryable: true };
    expect(onMessageRejected.mock.calls).toEqual([
      ["factory-a", "send-2", "The session's daemon connection is reconnecting.", rejection],
      ["factory-a", "send-3", "The session's daemon connection is reconnecting.", rejection],
    ]);

    // A refusal that is not retryable speaks only for its own message.
    onMessageRejected.mockClear();
    const next = fakeSocket();
    machine.sockets.set("factory-a", next);
    for (const ref of ["send-4", "send-5"]) sup.send("factory-a", { SendMessage: { target: "patient-pelican-9", message: ref, client_ref: ref } });
    await machine.handleDaemonMessage("factory-a", JSON.stringify({ error: "forbidden", client_ref: "send-4" }));
    expect(onMessageRejected.mock.calls).toEqual([["factory-a", "send-4", "forbidden", { code: "forbidden", retryable: false }]]);
    sup.stop();
  });
});

// cas-8fe2: HUB-J12 "stale proofs retry with no re-pair request" flaked in the
// merge queue, stuck on "Reconnecting". Two attaches shared one refused
// machine-socket ticket; the slower one's reachability probes finished after a
// retry had opened the socket and subscribed the session, and marked it failed
// with a retry that, finding the session already subscribed, changed nothing.
describe("a late attach failure after a retry succeeded (cas-8fe2)", () => {
  type Late = {
    desired: boolean;
    machineSocket?: unknown;
    machineSocketReady: boolean;
    machineSubscriptions: Set<string>;
    attachRetryTimers: Map<string, number>;
    handleAttachFailure(session: string, error: unknown): Promise<void>;
  };
  const SESSION = "patient-pelican-9";

  async function lateFailure(retrySubscribed: boolean) {
    vi.stubGlobal("window", globalThis);
    const onAttachState = vi.fn();
    const { sup, internals } = supervisor({ onAttachState, onSocketError: vi.fn() });
    const machine = internals as unknown as Late;
    machine.desired = true;
    // Another attach finished before this caller handles its shared failure.
    if (retrySubscribed) {
      machine.machineSocket = fakeSocket();
      machine.machineSocketReady = true;
      machine.machineSubscriptions.add(SESSION);
    }
    await machine.handleAttachFailure(SESSION, new TransientAuthError("stale_proof"));
    const failed = onAttachState.mock.calls.some(([session, state]) => session === SESSION && state.phase === "failed");
    const retrying = machine.attachRetryTimers.has(SESSION);
    sup.stop();
    return { failed, retrying };
  }

  it("leaves a session the retry already subscribed alone", async () => {
    expect(await lateFailure(true)).toEqual({ failed: false, retrying: false });
  });

  it("still retries when nothing else brought the session back", async () => {
    expect(await lateFailure(false)).toEqual({ failed: true, retrying: true });
  });
});
