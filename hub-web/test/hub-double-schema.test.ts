import type { Page, Route, WebSocketRoute } from "@playwright/test";
import { expect, it, onTestFinished, vi } from "vitest";
import { HubDouble, SCOPES } from "../e2e/journeys/hub-double";
import { hubWireValidator } from "../e2e/journeys/hub-wire";

type Response = Parameters<Route["fulfill"]>[0];
type HttpHandler = (route: Route) => Promise<void>;
type SocketHandler = (socket: WebSocketRoute) => void;

it("emits catalog, receipt, lease and machine-event fixtures that match the Rust wire schemas", async () => {
  const check = (name: string, payload: unknown, valid = true) => {
    const id = `urn:cassy:hub:${name}`;
    const validate = hubWireValidator.getSchema(id);
    expect.soft(validate, id).toBeDefined();
    if (validate) expect.soft(validate(payload), `${id}: ${hubWireValidator.errorsText(validate.errors)}`).toBe(valid);
  };

  const origin = "https://fixture.test";
  const now = "2026-11-24T12:00:00Z";
  let handleHttp!: HttpHandler;
  let handleSocket!: SocketHandler;
  const browser = { fetch: globalThis.fetch, __journeyResetEvents: (_host: string) => {} };
  vi.stubGlobal("window", browser);
  vi.stubGlobal("location", { href: origin });
  vi.stubGlobal("navigator", { onLine: true });
  onTestFinished(() => { vi.unstubAllGlobals(); });
  const page = {
    addInitScript(script: (bases: string[]) => void, bases?: string[]) { if (bases) script(bases); },
    context: () => ({ route() {} }),
    route(pattern: string, handler: HttpHandler) {
      if (pattern === "https://*.test/**") handleHttp = handler;
    },
    routeWebSocket(_matcher: unknown, handler: SocketHandler) { handleSocket = handler; },
    async evaluate<Args, Result>(callback: (args: Args) => Result, args: Args) { return callback(args); },
  } as unknown as Page;
  const row = { supervisor: "lead", project_dir: "/fixture", workers: [], started_at: now };
  const ended = { at: now, reason: "exit", workers_stopped: [], worker_states: [], orphans: [], unverified: [] };
  const double = new HubDouble(page, {
    machines: [{ id: "fixture", label: "Fixture", sessions: [
      { ...row, name: "running", liveness: "live" },
      { ...row, name: "stale", liveness: "stale_metadata" },
      { ...row, name: "ended", liveness: "stale_metadata", ended },
    ] }],
    fleet: { running: { agents: [], tasks: [], epics: [], focused_epic: null, spawnNames: [] } },
    scopes: { fixture: [...SCOPES, "factory-manage", "factory-operate"] },
  });
  await double.install();
  const stream = (await browser.fetch(`${origin}/v1/events`)).body!.getReader();

  const http = async (path: string, method = "GET", body: unknown = null, headers: Record<string, string> = {}) => {
    let response: Response | undefined;
    await handleHttp({
      request: () => ({ url: () => `${origin}${path}`, method: () => method, postDataJSON: () => body, headers: () => headers }),
      fulfill: async (result: Response) => { response = result; },
    } as unknown as Route);
    expect(response, `${method} ${path}`).toBeDefined();
    return response!;
  };

  const catalog = async () => {
    const { json } = await http("/v1/sessions");
    check("SessionsResponse", json);
    return json;
  };
  const { sessions, stopped_sessions } = await catalog();
  check("HubSession", sessions[0]);
  check("HubSession", { ...sessions[0], machine_online: null }, false);
  check("HubSession", { ...sessions[0], agents: [] }, false);
  check("HubSession", { ...sessions[0], ws_port: null });
  const stopped = stopped_sessions.find((session: { name: string }) => session.name === "ended");
  check("SessionEndOutcome", stopped?.ended);
  const changed = double.announceCatalog("fixture", { added: ["running"], removed: ["stale"] });
  await catalog();
  await changed;
  const refreshed = double.announceCatalog("fixture");
  await catalog();
  await refreshed;
  expect((await http("/v1/sessions/running/operations", "POST", {
    op_id: "focus", op: { kind: "focus_epic", epic_id: "epic" }, expected: { epic_id: null },
  })).status).toBe(200);

  const lease = "/v1/sessions/running/lease";
  check("LeaseSummary", (await http(lease)).json);
  double.holdLease("running", "Other device");
  check("LeaseSummary", (await http(lease)).json);
  double.setScopes("fixture", [...SCOPES, "hub-admin", "factory-manage"]);
  double.installations.set("restricted-device", {
    machine: "fixture", device_id: "restricted-device", credential_id: "restricted-id", credential_generation: 1,
    credential: "restricted-secret", scopes: ["pane-read", "session-read"], device_label: "Restricted device", operator_label: "Operator",
    controller_origin: origin, public_key_jwk: {}, revoked_at: null,
  });
  const restricted = { authorization: "DPoP restricted-secret" };
  const scopeDenied = await http(lease, "POST", { force: false }, restricted);
  check("LeaseRefusal", scopeDenied.json);
  expect.soft([scopeDenied.status, scopeDenied.json.error, scopeDenied.json.required_scope]).toEqual([403, "scope_denied", "pane:input"]);
  const busy = await http(lease, "POST", { force: false });
  check("LeaseRefusal", busy.json);
  expect([busy.status, busy.json.error]).toEqual([409, "lease_unavailable"]);
  const forceDenied = await http(lease, "POST", { force: true }, restricted);
  check("LeaseRefusal", forceDenied.json);
  expect.soft([forceDenied.status, forceDenied.json.error]).toEqual([401, "unauthorized"]);
  double.installations.set("expired-device", {
    machine: "fixture", device_id: "expired-device", credential_id: "expired-id", credential_generation: 1,
    credential: "expired-secret", scopes: SCOPES, device_label: "Expired device", operator_label: "Operator",
    controller_origin: origin, public_key_jwk: {}, revoked_at: null,
  });
  double.expireInstallation("expired-device");
  const expired = await http(lease, "POST", { force: false }, { authorization: "DPoP expired-secret" });
  check("LeaseRefusal", expired.json);
  expect.soft([expired.status, expired.json.error, expired.json.reason, expired.json.retryable]).toEqual([401, "unauthorized", "expired", false]);
  check("LeaseSummary", (await http(lease, "POST", { force: true })).json);
  const released = await http(lease, "DELETE");
  expect([released.status, released.body]).toEqual([204, ""]);
  check("LeaseSummary", (await http(lease)).json);
  await double.down("fixture", { http: "auth_store_unavailable" });
  const unavailable = await http(lease, "POST", { force: false });
  check("LeaseRefusal", unavailable.json);
  expect.soft([unavailable.status, unavailable.json.error, unavailable.json.reason, unavailable.json.retryable])
    .toEqual([503, "auth_store_unavailable", "auth_store_unavailable", true]);
  await double.up("fixture");

  const connect = (path: string) => {
    let clientSend!: (text: string) => void;
    const frames: Array<Record<string, any>> = [];
    handleSocket({
      url: () => `wss://fixture.test${path}`,
      onMessage: (handler: (text: string) => void) => { clientSend = handler; },
      send(text: string) { frames.push(JSON.parse(text)); },
    } as unknown as WebSocketRoute);
    return { frames, send: (message: unknown) => clientSend(JSON.stringify(message)) };
  };
  const multiplex = connect("/v1/attach");
  multiplex.send({ proto: 2 });
  multiplex.send({ channel: "pty:running", subscribe: true });
  const scripted = { device_id: "journey-device", client_ref: "scripted", command: "finish", phase: "observed", detail: "", output: "", interactive: false };
  double.send("running", { SupervisorCommandReceipt: { receipt: scripted } });
  for (const malformed of [
    { SupervisorCommandReceipt: { receipt: { ...scripted, client_ref: null } } },
    { SupervisorCommandReceipt: { receipt: scripted }, Output: {} },
    { SupervisorCommandReceipt: { receipt: scripted, extra: true } },
  ]) {
    expect.soft(() => double.send("running", malformed)).toThrow(expect.objectContaining({ schemaId: "urn:cassy:hub:SupervisorCommandReceiptFrame" }));
  }
  expect.soft(multiplex.frames).toHaveLength(3);
  expect.soft(multiplex.frames.slice(1).map(frame => frame.channel)).toEqual(["pty:running", "pty:running"]);
  check("DaemonWelcome", multiplex.frames[1]?.message);
  check("DaemonWelcome", { Welcome: { ...multiplex.frames[1]?.message.Welcome, pane_bootstrap: [] } }, false);
  check("SupervisorCommandReceiptFrame", multiplex.frames[2]?.message);

  const legacy = connect("/v1/sessions/running/attach");
  legacy.send({ SupervisorCommand: { command: "standby", client_ref: "command", confirmed: true } });
  expect(legacy.frames).toHaveLength(2);
  check("DaemonWelcome", legacy.frames[0]);
  check("SupervisorCommandReceiptFrame", legacy.frames[1]);

  const result = await http("/v1/sessions/running", "DELETE");
  check("EndSessionReceipt", result.json);
  expect(result.json.outcome).toBe("ended");
  const raw = { sequence: 4, revision: 0, kind: "daemon_error", session: "running", pane_id: null, at: now, payload: { message: "fixture" }, enrichment_pending: true };
  const metadata = { kind: "stream_metadata", epoch: "fixture", oldest_sequence: 4, latest_sequence: 4, retained: 1 };
  await double.sendMachineEvents("fixture", [metadata, raw, { kind: "replay_complete" }, raw]);
  expect.soft(() => double.sendMachineEvents("fixture", [metadata, { ...raw, at: null }]))
    .toThrow(expect.objectContaining({ schemaId: "urn:cassy:hub:MachineEvent" }));
  browser.__journeyResetEvents("fixture.test");
  const decoder = new TextDecoder();
  const markers: string[] = [];
  const events: Array<Record<string, any>> = [];
  for (;;) {
    const { value, done } = await stream.read();
    if (done) break;
    const frame = decoder.decode(value);
    const data = /^data: (.+)$/m.exec(frame)?.[1];
    if (!data) continue;
    const event = JSON.parse(data);
    if (event.kind === "stream_metadata" || event.kind === "replay_complete") {
      markers.push(event.kind);
      expect.soft(frame.split("\n")[0]).toBe(`event: ${event.kind}`);
      continue;
    }
    events.push(event);
    check("MachineEvent", event);
    expect.soft(event.sequence).toBeGreaterThan(0);
    expect.soft(frame.split("\n").slice(0, 2)).toEqual([`id: ${event.sequence}.${event.revision}`, `event: ${event.kind.replaceAll("_", "")}`]);
  }
  expect.soft(markers).toEqual(["stream_metadata", "replay_complete"]);
  expect.soft(events.map(event => event.kind)).toEqual(["session_added", "session_removed", "fleet_changed", "daemon_error", "daemon_error"]);
  expect.soft(events.map(event => event.sequence)).toEqual([1, 2, 3, 4, 4]);
  check("MachineEvent", { ...events[0], diagnostic: null }, false);
  check("MachineEvent", { ...raw, enrichment_pending: false }, false);
});
