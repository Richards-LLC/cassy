import { readFileSync } from "node:fs";
import type { Page, Route, WebSocketRoute } from "@playwright/test";
import Ajv2020 from "ajv/dist/2020.js";
import addFormats from "ajv-formats";
import { expect, it, onTestFinished, vi } from "vitest";
import { HubDouble, SCOPES } from "../e2e/journeys/hub-double";

type Response = Parameters<Route["fulfill"]>[0];
type HttpHandler = (route: Route) => Promise<void>;
type SocketHandler = (socket: WebSocketRoute) => void;

it("emits catalog, receipt, lease and machine-event fixtures that match the Rust wire schemas", async () => {
  const schemas = JSON.parse(readFileSync(new URL("./fixtures/hub-wire.schema.json", import.meta.url), "utf8"));
  const ajv = addFormats(new Ajv2020({ allErrors: true, strict: false }));
  for (const schema of Object.values(schemas)) ajv.addSchema(schema as object);
  const check = (name: string, payload: unknown, valid = true) => {
    const id = `urn:cassy:hub:${name}`;
    const validate = ajv.getSchema(id);
    expect.soft(validate, id).toBeDefined();
    if (validate) expect.soft(validate(payload), `${id}: ${ajv.errorsText(validate.errors)}`).toBe(valid);
  };

  const origin = "https://fixture.test";
  const now = "2026-11-24T12:00:00Z";
  let handleHttp!: HttpHandler;
  let handleSocket!: SocketHandler;
  const browser = { fetch: globalThis.fetch, __journeyResetEvents: (_host: string) => {} };
  vi.stubGlobal("window", browser);
  vi.stubGlobal("location", { href: origin });
  vi.stubGlobal("navigator", { onLine: true });
  onTestFinished(() => vi.unstubAllGlobals());
  const page = {
    addInitScript(script: (bases: string[]) => void, bases?: string[]) { if (bases) script(bases); },
    context: () => ({ route() {} }),
    route(pattern: string, handler: HttpHandler) {
      if (pattern === "https://*.test/**") handleHttp = handler;
    },
    routeWebSocket(_matcher: unknown, handler: SocketHandler) { handleSocket = handler; },
    async evaluate<T>(callback: (args: readonly [string, string]) => T, args: readonly [string, string]) { return callback(args); },
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
  double.setScopes("fixture", ["pane-read", "session-read"]);
  const scopeDenied = await http(lease, "POST", { force: false });
  check("LeaseRefusal", scopeDenied.json);
  expect.soft([scopeDenied.status, scopeDenied.json.error, scopeDenied.json.required_scope]).toEqual([403, "scope_denied", "pane:input"]);
  double.setScopes("fixture", SCOPES);
  const busy = await http(lease, "POST", { force: false });
  check("LeaseRefusal", busy.json);
  expect([busy.status, busy.json.error]).toEqual([409, "lease_unavailable"]);
  const forceDenied = await http(lease, "POST", { force: true });
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
  double.setScopes("fixture", [...SCOPES, "hub-admin", "factory-manage"]);
  check("LeaseSummary", (await http(lease, "POST", { force: true })).json);
  const released = await http(lease, "DELETE");
  expect([released.status, released.body]).toEqual([204, ""]);
  check("LeaseSummary", (await http(lease)).json);

  let clientSend!: (text: string) => void;
  const frames: Array<Record<string, any>> = [];
  handleSocket({
    url: () => "wss://fixture.test/v1/sessions/running/attach",
    onMessage: (handler: (text: string) => void) => { clientSend = handler; },
    send(text: string) { frames.push(JSON.parse(text)); },
  } as unknown as WebSocketRoute);
  clientSend(JSON.stringify({ SupervisorCommand: { command: "standby", client_ref: "command", confirmed: true } }));
  double.send("running", { SupervisorCommandReceipt: { receipt: { device_id: "journey-device", client_ref: "scripted", command: "finish", phase: "observed", detail: "", output: "", interactive: false } } });
  const [welcome, ...receipts] = frames;
  check("DaemonWelcome", welcome?.Welcome);
  expect(receipts).toHaveLength(2);
  for (const frame of receipts) check("SupervisorCommandReceipt", frame.SupervisorCommandReceipt.receipt);

  const result = await http("/v1/sessions/running", "DELETE");
  check("EndSessionReceipt", result.json);
  expect(result.json.outcome).toBe("ended");
  browser.__journeyResetEvents("fixture.test");
  const decoder = new TextDecoder();
  const events: Array<Record<string, any>> = [];
  for (;;) {
    const { value, done } = await stream.read();
    if (done) break;
    const frame = decoder.decode(value);
    const data = /^data: (.+)$/m.exec(frame)?.[1];
    if (!data) continue;
    const event = JSON.parse(data);
    events.push(event);
    check("MachineEvent", event);
    expect.soft(event.sequence).toBeGreaterThan(0);
    expect.soft(frame.split("\n").slice(0, 2)).toEqual([`id: ${event.sequence}.${event.revision}`, `event: ${event.kind.replaceAll("_", "")}`]);
  }
  expect.soft(events.map(event => event.kind)).toEqual(["session_added", "session_removed", "fleet_changed"]);
  check("MachineEvent", { ...events[0], diagnostic: null }, false);
});
