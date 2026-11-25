import { readFileSync } from "node:fs";
import type { Page, Route, WebSocketRoute } from "@playwright/test";
import Ajv2020 from "ajv/dist/2020.js";
import addFormats from "ajv-formats";
import { expect, it } from "vitest";
import { HubDouble, SCOPES } from "../e2e/journeys/hub-double";

type Response = Parameters<Route["fulfill"]>[0];
type HttpHandler = (route: Route) => Promise<void>;
type SocketHandler = (socket: WebSocketRoute) => void;

it("emits catalog, receipt, lease and machine-event fixtures that match the Rust wire schemas", async () => {
  const schemas = JSON.parse(readFileSync(new URL("./fixtures/hub-wire.schema.json", import.meta.url), "utf8"));
  const ajv = addFormats(new Ajv2020({ allErrors: true, strict: false }));
  for (const schema of Object.values(schemas)) ajv.addSchema(schema as object);
  const check = (name: string, payload: unknown) => {
    const id = `urn:cassy:hub:${name}`;
    const validate = ajv.getSchema(id);
    expect(validate, id).toBeDefined();
    expect(validate!(payload), `${id}: ${ajv.errorsText(validate!.errors)}`).toBe(true);
  };

  const origin = "https://fixture.test";
  const now = "2026-11-24T12:00:00Z";
  let handleHttp!: HttpHandler;
  let handleSocket!: SocketHandler;
  const events: Array<{ kind: string }> = [];
  const page = {
    addInitScript() {},
    context: () => ({ route() {} }),
    route(pattern: string, handler: HttpHandler) {
      if (pattern === "https://*.test/**") handleHttp = handler;
    },
    routeWebSocket(_matcher: unknown, handler: SocketHandler) { handleSocket = handler; },
    evaluate(_callback: unknown, [_host, data]: [string, string]) {
      events.push(JSON.parse(/^data: (.+)$/m.exec(data)?.[1] ?? data));
      return Promise.resolve(1);
    },
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

  const http = async (path: string, method = "GET", body: unknown = null) => {
    let response: Response | undefined;
    await handleHttp({
      request: () => ({ url: () => `${origin}${path}`, method: () => method, postDataJSON: () => body, headers: () => ({}) }),
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
  check("SessionEndOutcome", stopped_sessions.find((session: { name: string }) => session.name === "ended")?.ended);
  const changed = double.announceCatalog("fixture", { added: ["running"], removed: ["stale"] });
  await catalog();
  await changed;
  expect((await http("/v1/sessions/running/operations", "POST", {
    op_id: "focus", op: { kind: "focus_epic", epic_id: "epic" }, expected: { epic_id: null },
  })).status).toBe(200);

  const lease = "/v1/sessions/running/lease";
  check("LeaseSummary", (await http(lease)).json);
  double.holdLease("running", "Other device");
  check("LeaseSummary", (await http(lease)).json);
  const busy = await http(lease, "POST", { force: false });
  check("LeaseRefusal", busy.json);
  expect([busy.status, busy.json.error]).toEqual([409, "lease_unavailable"]);
  const forceDenied = await http(lease, "POST", { force: true });
  check("LeaseRefusal", forceDenied.json);
  expect([forceDenied.status, forceDenied.json.error]).toEqual([401, "unauthorized"]);
  double.setScopes("fixture", ["session-read", "hub-admin", "factory-manage"]);
  const inputDenied = await http(lease, "POST", { force: false });
  check("LeaseRefusal", inputDenied.json);
  expect([inputDenied.status, inputDenied.json.required_scope]).toEqual([403, "pane:input"]);
  double.setScopes("fixture", [...SCOPES, "hub-admin", "factory-manage"]);
  check("LeaseSummary", (await http(lease, "POST", { force: true })).json);
  const released = await http(lease, "DELETE");
  expect([released.status, released.body]).toEqual([204, ""]);
  check("LeaseSummary", (await http(lease)).json);

  let clientSend!: (text: string) => void;
  const receipts: unknown[] = [];
  handleSocket({
    url: () => "wss://fixture.test/v1/sessions/running/attach",
    onMessage: (handler: (text: string) => void) => { clientSend = handler; },
    send(text: string) {
      const frame = JSON.parse(text);
      if (frame.SupervisorCommandReceipt) receipts.push(frame.SupervisorCommandReceipt.receipt);
    },
  } as unknown as WebSocketRoute);
  clientSend(JSON.stringify({ SupervisorCommand: { command: "standby", client_ref: "command", confirmed: true } }));
  expect(receipts).toHaveLength(1);
  for (const receipt of receipts) check("SupervisorCommandReceipt", receipt);

  for (const [session, outcome] of [["running", "ended"], ["stale", "cleaned_stale"]]) {
    const result = await http(`/v1/sessions/${session}`, "DELETE");
    check("EndSessionReceipt", result.json);
    expect(result.json.outcome).toBe(outcome);
  }
  expect(events.map(event => event.kind)).toEqual(expect.arrayContaining(["session_added", "fleet_changed"]));
  for (const event of events) check("MachineEvent", event);
});
