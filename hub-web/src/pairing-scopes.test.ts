import { CONTROL_CAPABILITY, FACTORY_MANAGE_CAPABILITY, FACTORY_OPERATE_CAPABILITY, READ_CAPABILITY, canEnableFactoryOperate, scopeGrantCommand, scopeSummary } from "./pairing-scopes";
import { NOT_ALLOWED_ON_PAIRING, fleetControlGate, fleetOperationScope, type FleetOperation } from "./fleet-permissions";
import { readFile } from "node:fs/promises";
import { describe, expect, it, vi } from "vitest";
import { consumePairingFragment } from "./fragment";
import { createPairingDraft } from "./pairing-draft";
import { exchangePendingPairing, PairingExchangeError } from "./pairing-exchange";
import { pairingExchangeFailure, unreachableHubMessage } from "./pairing-messages";
import {
  PAIRING_SCOPES,
  READ_ONLY_PAIRING_SCOPES,
  pairCommand,
  parseGrantedScopes,
  preselectedScopes,
  scopeChoices,
  ungrantedScopes,
} from "./pairing-scopes";
import { PendingPairingStore } from "./pending-pairing";
import type { Scope } from "./types";

const TOKEN = "A".repeat(43);
const ORIGIN = "https://commander.example";

class MemoryStorage implements Storage {
  readonly values = new Map<string, string>();
  get length(): number { return this.values.size; }
  clear(): void { this.values.clear(); }
  getItem(key: string): string | null { return this.values.get(key) ?? null; }
  key(index: number): string | null { return [...this.values.keys()][index] ?? null; }
  removeItem(key: string): void { this.values.delete(key); }
  setItem(key: string, value: string): void { this.values.set(key, value); }
}

function fragment(hash: string): { invitation: ReturnType<typeof consumePairingFragment>; store: PendingPairingStore } {
  const store = new PendingPairingStore(new MemoryStorage());
  const location = { hash, pathname: "/", search: "" } as Location;
  const history = { replaceState: () => undefined } as unknown as History;
  return { invitation: consumePairingFragment(location, history, store), store };
}

/** The pairing exchange's rejection, typed, so its copy and recoverability can be asserted. */
async function failure(attempt: Promise<unknown>): Promise<PairingExchangeError> {
  try {
    await attempt;
  } catch (error) {
    return error as PairingExchangeError;
  }
  throw new Error("expected the pairing exchange to fail");
}

function response(status: number, body?: unknown): Response {
  return new Response(body === undefined ? null : JSON.stringify(body), {
    status,
    headers: body === undefined ? {} : { "Content-Type": "application/json" },
  });
}

const credential = {
  device_id: "device",
  credential_id: "credential",
  credential: "opaque",
  expires_at: "2027-01-01T00:00:00Z",
  scopes: READ_ONLY_PAIRING_SCOPES,
};

function invitationWith(scopes?: readonly Scope[]) {
  return {
    kind: "invitation" as const,
    token: "one-time-token",
    hubId: "machine-uuid",
    hubUrl: "https://workstation.tail.example",
    controllerOrigin: ORIGIN,
    ...(scopes ? { scopes } : {}),
  };
}

describe("pairing scope ceiling", () => {
  it("carries the granted scopes from a scope-aware invitation fragment", () => {
    const { invitation, store } = fragment(`#pair=${TOKEN}&hub=machine-uuid&scopes=machine-read,session-read,pane-read`);
    expect(invitation).toMatchObject({ kind: "invitation", token: TOKEN, hubId: "machine-uuid", scopes: READ_ONLY_PAIRING_SCOPES });
    // The ceiling has to survive the reload that a phone browser can force.
    expect(store.load()).toMatchObject({ scopes: READ_ONLY_PAIRING_SCOPES });
  });

  it("accepts the colon spelling the CLI prints and rejects unknown, duplicate, or empty scope lists", () => {
    expect(parseGrantedScopes("machine:read,pane:input")).toEqual(["machine-read", "pane-input"]);
    expect(parseGrantedScopes("machine-read,machine-read")).toBeUndefined();
    expect(parseGrantedScopes("machine-read,root-everything")).toBeUndefined();
    expect(parseGrantedScopes("")).toBeUndefined();
    expect(parseGrantedScopes(null)).toBeUndefined();
  });

  it("still consumes a pre-scope fragment and leaves its ceiling unknown", () => {
    const { invitation } = fragment(`#pair=${TOKEN}&hub=machine-uuid`);
    expect(invitation).toMatchObject({ kind: "invitation", token: TOKEN, hubId: "machine-uuid" });
    expect(invitation?.scopes).toBeUndefined();
  });

  it("consumes a scope-aware fragment even when the scope list is malformed", () => {
    const { invitation } = fragment(`#pair=${TOKEN}&hub=machine-uuid&scopes=not-a-scope`);
    expect(invitation).toMatchObject({ kind: "invitation", token: TOKEN });
    expect(invitation?.scopes).toBeUndefined();
  });

  it("preselects exactly the granted scopes, and read-only for an invitation that declares none", () => {
    expect(preselectedScopes({ kind: "invitation", token: TOKEN, hubId: "m", scopes: ["machine-read", "pane-input"] })).toEqual(["machine-read", "pane-input"]);
    expect(preselectedScopes({ kind: "invitation", token: TOKEN, hubId: "m" })).toEqual(READ_ONLY_PAIRING_SCOPES);
    expect(preselectedScopes(null)).toEqual(PAIRING_SCOPES);
  });

  it("offers every scope but checks and enables only the granted ones", () => {
    const choices = scopeChoices(READ_ONLY_PAIRING_SCOPES, READ_ONLY_PAIRING_SCOPES);
    expect(choices).toHaveLength(6);
    expect(choices.filter((choice) => choice.checked).map((choice) => choice.scope)).toEqual(READ_ONLY_PAIRING_SCOPES);
    expect(choices.filter((choice) => !choice.granted).map((choice) => choice.scope)).toEqual(["pane-input", "message-send", "pane-interrupt"]);
    expect(choices.map((choice) => choice.label)).toContain("pane:input");
    expect(ungrantedScopes(READ_ONLY_PAIRING_SCOPES)).toEqual(["pane-input", "message-send", "pane-interrupt"]);
  });

  it("leaves every box enabled when the invitation predates scope-aware fragments", () => {
    const choices = scopeChoices(undefined, READ_ONLY_PAIRING_SCOPES);
    expect(choices.every((choice) => choice.granted)).toBe(true);
    expect(choices.filter((choice) => choice.checked).map((choice) => choice.scope)).toEqual(READ_ONLY_PAIRING_SCOPES);
    expect(ungrantedScopes(undefined)).toEqual([]);
  });

  it("names the exact command that mints the missing scopes", () => {
    expect(pairCommand(ORIGIN, PAIRING_SCOPES)).toBe(
      "cas hub pair --origin https://commander.example --scopes machine:read,session:read,pane:read,pane:input,message:send,pane:interrupt",
    );
  });

  it("seeds the pairing draft from the invitation's ceiling", () => {
    expect(createPairingDraft(ORIGIN, READ_ONLY_PAIRING_SCOPES).scopes).toEqual(READ_ONLY_PAIRING_SCOPES);
    expect(createPairingDraft(ORIGIN).scopes).toEqual(PAIRING_SCOPES);
  });

  it("never requests a scope above the invitation ceiling", async () => {
    const requests: Record<string, unknown>[] = [];
    await exchangePendingPairing({
      invitation: invitationWith(READ_ONLY_PAIRING_SCOPES),
      controllerOrigin: ORIGIN,
      deviceLabel: "Browser",
      operatorLabel: "Operator",
      // A stale form (or a tampered DOM) asking for control scopes must not reach the hub.
      requestedScopes: [...PAIRING_SCOPES],
      fetcher: async (_input, init) => {
        requests.push(JSON.parse(String(init?.body)) as Record<string, unknown>);
        return response(200, credential);
      },
      createKey: async () => ({ privateKey: {} as CryptoKey, publicKey: { kty: "EC" } }),
      installationGeneration: 1,
      stagePersisted: async () => undefined,
      activatePersisted: async () => true,
      rollbackPersisted: async () => true,
    });
    expect(requests).toHaveLength(1);
    expect(requests[0]!.requested_scopes).toEqual(READ_ONLY_PAIRING_SCOPES);
  });

  it("requests only the boxes the operator left ticked", async () => {
    const requests: Record<string, unknown>[] = [];
    const machine = await exchangePendingPairing({
      invitation: invitationWith(READ_ONLY_PAIRING_SCOPES),
      controllerOrigin: ORIGIN,
      deviceLabel: "Browser",
      operatorLabel: "Operator",
      requestedScopes: ["machine-read", "session-read"],
      fetcher: async (_input, init) => {
        requests.push(JSON.parse(String(init?.body)) as Record<string, unknown>);
        return response(200, { ...credential, scopes: ["machine-read", "session-read"] });
      },
      createKey: async () => ({ privateKey: {} as CryptoKey, publicKey: { kty: "EC" } }),
      installationGeneration: 1,
      stagePersisted: async () => undefined,
      activatePersisted: async () => true,
      rollbackPersisted: async () => true,
    });
    expect(requests[0]!.requested_scopes).toEqual(["machine-read", "session-read"]);
    expect(machine.scopes).toEqual(["machine-read", "session-read"]);
  });

  it("keeps the invitation and says what to do when no scope is left ticked", async () => {
    const fetcher = vi.fn();
    const error = await failure(exchangePendingPairing({
      invitation: invitationWith(READ_ONLY_PAIRING_SCOPES),
      controllerOrigin: ORIGIN,
      deviceLabel: "Browser",
      operatorLabel: "Operator",
      requestedScopes: [],
      fetcher,
      createKey: async () => ({ privateKey: {} as CryptoKey, publicKey: { kty: "EC" } }),
      installationGeneration: 1,
      stagePersisted: async () => undefined,
      activatePersisted: async () => true,
      rollbackPersisted: async () => true,
    }));
    expect(fetcher).not.toHaveBeenCalled();
    expect(error).toBeInstanceOf(PairingExchangeError);
    expect(error.recoverable).toBe(true);
    expect(error.message).toMatch(/Tick at least one scope/);
  });

  it("uses the invitation's own scopes when the form offers no scope boxes", async () => {
    const requests: Record<string, unknown>[] = [];
    await exchangePendingPairing({
      invitation: invitationWith(["machine-read"]),
      controllerOrigin: ORIGIN,
      deviceLabel: "Browser",
      operatorLabel: "Operator",
      fetcher: async (_input, init) => {
        requests.push(JSON.parse(String(init?.body)) as Record<string, unknown>);
        return response(200, { ...credential, scopes: ["machine-read"] });
      },
      createKey: async () => ({ privateKey: {} as CryptoKey, publicKey: { kty: "EC" } }),
      installationGeneration: 1,
      stagePersisted: async () => undefined,
      activatePersisted: async () => true,
      rollbackPersisted: async () => true,
    });
    expect(requests[0]!.requested_scopes).toEqual(["machine-read"]);
  });
});

describe("pairing failure copy", () => {
  it("replaces the bare unauthorized wire token with a cause and a next step", () => {
    const failure = pairingExchangeFailure({ status: 401, body: JSON.stringify({ error: "unauthorized" }), controllerOrigin: ORIGIN });
    expect(failure.message).not.toContain("unauthorized");
    expect(failure.message).toContain("cas hub pair --origin https://commander.example");
    expect(failure.message).toMatch(/once|expire/i);
    expect(failure.keepInvitation).toBe(false);
  });

  it("does not present a bare wire phrase as an explanation", () => {
    const failure = pairingExchangeFailure({ status: 401, body: "pairing exchange refused", controllerOrigin: ORIGIN });
    expect(failure.message).not.toContain("pairing exchange refused");
    expect(failure.message).toContain("cas hub pair --origin");
  });

  it("keeps a hub sentence when the hub sends one", () => {
    const failure = pairingExchangeFailure({
      status: 403,
      body: JSON.stringify({ error_description: "This device is not approved for this hub." }),
      controllerOrigin: ORIGIN,
    });
    expect(failure.message).toContain("This device is not approved for this hub.");
    expect(failure.message).toContain("cas hub pair --origin");
  });

  it("keeps the invitation for a hub restart and a wrong hub URL", () => {
    for (const status of [500, 503, 404]) {
      const failure = pairingExchangeFailure({ status, body: "", controllerOrigin: ORIGIN });
      expect(failure.keepInvitation).toBe(true);
      expect(failure.message.trim().endsWith(".")).toBe(true);
    }
    expect(pairingExchangeFailure({ status: 404, body: "", controllerOrigin: ORIGIN }).message).toMatch(/machine's hub address/);
  });

  it("uses Retry-After for throttling without promising the invitation is unused", async () => {
    const error = await failure(exchangePendingPairing({
      invitation: invitationWith(READ_ONLY_PAIRING_SCOPES),
      controllerOrigin: ORIGIN,
      deviceLabel: "Browser",
      operatorLabel: "Operator",
      fetcher: async () => new Response(JSON.stringify({ error: "slow_down" }), {
        status: 429,
        headers: { "Content-Type": "application/json", "Retry-After": "37" },
      }),
      createKey: async () => ({ privateKey: {} as CryptoKey, publicKey: { kty: "EC" } }),
      installationGeneration: 1,
      stagePersisted: async () => undefined,
      activatePersisted: async () => true,
      rollbackPersisted: async () => true,
    }));
    expect(error.recoverable).toBe(true);
    expect(error.message).toContain("37 seconds");
    expect(error.message).toMatch(/fresh invitation/i);
    expect(error.message).not.toMatch(/still open|unused|untouched/i);
  });

  it("names the network as the cause when the hub cannot be reached at all", async () => {
    const message = unreachableHubMessage("https://workstation.tail.example");
    expect(message).toContain("workstation.tail.example");
    expect(message).toMatch(/Tailscale/);

    const error = await failure(exchangePendingPairing({
      invitation: invitationWith(READ_ONLY_PAIRING_SCOPES),
      controllerOrigin: ORIGIN,
      deviceLabel: "Browser",
      operatorLabel: "Operator",
      fetcher: async () => { throw new TypeError("Failed to fetch"); },
      createKey: async () => ({ privateKey: {} as CryptoKey, publicKey: { kty: "EC" } }),
      installationGeneration: 1,
      stagePersisted: async () => undefined,
      activatePersisted: async () => true,
      rollbackPersisted: async () => true,
    }));
    expect(error).toBeInstanceOf(PairingExchangeError);
    expect(error.recoverable).toBe(true);
    expect(error.message).toContain("workstation.tail.example");
  });

  it("marks a hub refusal unrecoverable so the burnt invitation is not offered again", async () => {
    const error = await failure(exchangePendingPairing({
      invitation: invitationWith(READ_ONLY_PAIRING_SCOPES),
      controllerOrigin: ORIGIN,
      deviceLabel: "Browser",
      operatorLabel: "Operator",
      fetcher: async () => response(401, { error: "unauthorized" }),
      createKey: async () => ({ privateKey: {} as CryptoKey, publicKey: { kty: "EC" } }),
      installationGeneration: 1,
      stagePersisted: async () => undefined,
      activatePersisted: async () => true,
      rollbackPersisted: async () => true,
    }));
    expect(error.recoverable).toBe(false);
    expect(error.message).not.toContain("unauthorized");
  });
});

describe("pairing form scope invariants", () => {
  // Contract: a recoverable failure retains the invitation for another attempt.
  // Consumer: main.ts pairing exchange error handler and its next form render.
  // exchangePendingPairing tests cover recoverability, but no current journey
  // proves this handler's retry state. Retain this historical pin for follow-up.
  it("keeps a recoverable pairing failure's invitation instead of discarding it", async () => {
    const source = await readFile(new URL("main.ts", import.meta.url), "utf8");
    expect(source).toContain("error instanceof PairingExchangeError && error.recoverable");
    expect(source).toContain("preselectedScopes(");
  });
});

describe("plain capability summary beside the exact scopes (cas-8051 F7)", () => {
  it("collapses a complete group to one phrase", () => {
    expect(scopeSummary(["machine-read", "session-read", "pane-read"])).toEqual([READ_CAPABILITY]);
    expect(scopeSummary(["machine-read", "session-read", "pane-read", "pane-input", "message-send", "pane-interrupt"])).toEqual([READ_CAPABILITY, CONTROL_CAPABILITY]);
    expect(scopeSummary(["pane-input", "message-send", "pane-interrupt"])).toEqual([CONTROL_CAPABILITY]);
  });

  it("names exactly the granted capabilities for a singleton (review 01:10)", () => {
    expect(scopeSummary(["pane-interrupt"])).toEqual(["Interrupt supervisors"]);
    expect(scopeSummary(["message-send"])).toEqual(["Send messages to supervisors"]);
    expect(scopeSummary(["pane-input"])).toEqual(["Type into its sessions"]);
    expect(scopeSummary(["machine-read"])).toEqual(["See this machine"]);
    expect(scopeSummary(["pane-read"])).toEqual(["Read raw output"]);
    for (const single of ["pane-interrupt", "message-send", "pane-input", "machine-read", "session-read", "pane-read"] as const) {
      const words = scopeSummary([single]);
      expect(words).toHaveLength(1);
      expect(words[0]).not.toBe(READ_CAPABILITY);
      expect(words[0]).not.toBe(CONTROL_CAPABILITY);
    }
  });

  it("names exactly the granted subset of a group, keeping the other group's phrase when complete", () => {
    expect(scopeSummary(["machine-read", "session-read"])).toEqual(["See this machine", "See its sessions"]);
    expect(scopeSummary(["machine-read", "session-read", "pane-read", "message-send"])).toEqual([READ_CAPABILITY, "Send messages to supervisors"]);
    expect(scopeSummary(["session-read", "pane-input", "message-send", "pane-interrupt"])).toEqual(["See its sessions", CONTROL_CAPABILITY]);
    expect(scopeSummary(["pane-read", "pane-input"])).toEqual(["Read raw output", "Type into its sessions"]);
    // Never a capability the grant does not contain.
    expect(scopeSummary(["message-send"]).join(" ")).not.toMatch(/interrupt|Type/);
    expect(scopeSummary(["pane-read"]).join(" ")).not.toMatch(/sessions/);
  });

  it("names any scope outside those two sets as itself", () => {
    expect(scopeSummary(["machine-read", "hub-admin"])).toEqual(["See this machine", "See and revoke this machine's other browser installations"]);
    // cas-d382: the brief's plain words, not "Manage the factory".
    expect(scopeSummary(["factory-manage"])).toEqual(["Stop and restart workers and sessions"]);
    expect(scopeSummary([])).toEqual([]);
  });
});

describe("fleet operation scopes (cas-d382, fleet-operations brief S4)", () => {
  const CONTROL: Scope[] = [...PAIRING_SCOPES];
  it("reads factory:operate and factory:manage from an invitation, in either spelling", () => {
    expect(parseGrantedScopes("machine:read,session:read,pane:read,pane:input,message:send,pane:interrupt,factory:manage")).toEqual([...PAIRING_SCOPES, "factory-manage"]);
    expect(parseGrantedScopes("machine-read,factory-operate")).toEqual(["machine-read", "factory-operate"]);
    // cas-5e53 F08: an admin invitation keeps its ceiling instead of falling back to read-only.
    expect(parseGrantedScopes("machine:read,hub:admin")).toEqual(["machine-read", "hub-admin"]);
    const { invitation } = fragment(`#pair=${TOKEN}&hub=machine-uuid&scopes=machine:read,session:read,pane:read,factory:manage`);
    expect(invitation?.scopes).toEqual(["machine-read", "session-read", "pane-read", "factory-manage"]);
  });

  it("says what they allow in plain words, beside the exact scopes", () => {
    expect(scopeSummary([...CONTROL, "factory-operate"])).toEqual([READ_CAPABILITY, CONTROL_CAPABILITY, "Manage workers and tasks"]);
    expect(scopeSummary([...CONTROL, "factory-manage"])).toEqual([READ_CAPABILITY, CONTROL_CAPABILITY, "Stop and restart workers and sessions"]);
    expect(FACTORY_OPERATE_CAPABILITY).toBe("Manage workers and tasks");
    expect(FACTORY_MANAGE_CAPABILITY).toBe("Stop and restart workers and sessions");
  });

  it("offers a fleet scope's box only when the invitation grants it, ticked", () => {
    const granted: Scope[] = [...CONTROL, "factory-manage"];
    const choices = scopeChoices(granted, granted);
    expect(choices.map((choice) => choice.scope)).toEqual(granted);
    expect(choices.find((choice) => choice.scope === "factory-manage")).toMatchObject({ label: "factory:manage", granted: true, checked: true });
    expect(scopeChoices(CONTROL, CONTROL).map((choice) => choice.scope)).not.toContain("factory-manage");
    expect(scopeChoices(CONTROL, CONTROL).map((choice) => choice.scope)).not.toContain("factory-operate");
  });

  it("gates each operation by the operator's decision: Stop, Restart and End session need factory:manage", () => {
    const expected: Record<FleetOperation, Scope> = {
      "ask-merge": "message-send", "focus-epic": "factory-operate", "add-workers": "factory-operate", "pause-worker": "factory-operate",
      "assign-task": "factory-operate", "restart-worker": "factory-manage", "stop-worker": "factory-manage", "end-session": "factory-manage",
    };
    for (const [operation, scope] of Object.entries(expected) as [FleetOperation, Scope][]) expect(fleetOperationScope(operation), operation).toBe(scope);
  });

  it("says a missing scope in words and names the command that adds it to the current pairing", () => {
    const stop = fleetControlGate(CONTROL, "stop-worker", ORIGIN);
    expect(stop).toMatchObject({ allowed: false, scope: "factory-manage", state: NOT_ALLOWED_ON_PAIRING, grantable: false });
    if (stop.allowed) throw new Error("unreachable");
    expect(stop.state).toBe("Not allowed on this pairing");
    expect(stop.reason).toBe("Needs the Stop and restart workers and sessions permission.");
    expect(stop.command).toBe("cas hub pair --origin https://commander.example --scopes machine:read,session:read,pane:read,pane:input,message:send,pane:interrupt,factory:manage");
    expect(fleetControlGate([...CONTROL, "factory-manage"], "stop-worker", ORIGIN)).toEqual({ allowed: true, scope: "factory-manage" });
    expect(fleetControlGate([...CONTROL, "factory-manage"], "end-session", ORIGIN).allowed).toBe(true);
    // factory:manage is never self-granted, even with control scopes.
    expect(fleetControlGate([...CONTROL, "factory-operate"], "restart-worker", ORIGIN)).toMatchObject({ allowed: false, grantable: false });
  });

  it("offers the one-time Allow managing workers grant only to a control pairing", () => {
    expect(canEnableFactoryOperate(CONTROL)).toBe(true);
    expect(canEnableFactoryOperate(READ_ONLY_PAIRING_SCOPES)).toBe(false);
    expect(fleetControlGate(CONTROL, "add-workers", ORIGIN)).toMatchObject({ allowed: false, scope: "factory-operate", grantable: true });
    const readOnly = fleetControlGate(READ_ONLY_PAIRING_SCOPES, "pause-worker", ORIGIN);
    expect(readOnly).toMatchObject({ allowed: false, grantable: false });
    if (!readOnly.allowed) expect(readOnly.command).toBe("cas hub pair --origin https://commander.example --scopes machine:read,session:read,pane:read,factory:operate");
    expect(fleetControlGate([...CONTROL, "factory-operate"], "assign-task", ORIGIN).allowed).toBe(true);
    expect(scopeGrantCommand(ORIGIN, ["machine-read", "factory-operate"], "factory-operate")).toBe("cas hub pair --origin https://commander.example --scopes machine:read,factory:operate");
  });
});

describe("hub:admin needs explicit consent (cas-5e53 F08)", () => {
  it("is offered when the invitation grants it, never pre-ticked, with what it allows", () => {
    const granted = parseGrantedScopes("machine:read,session:read,pane:read,hub:admin")!;
    const selected = preselectedScopes({ kind: "invitation", token: TOKEN, hubId: "machine-uuid", scopes: granted } as never);
    expect(selected).toEqual(["machine-read", "session-read", "pane-read"]);
    const admin = scopeChoices(granted, selected).find((choice) => choice.scope === "hub-admin")!;
    expect(admin).toEqual({ scope: "hub-admin", label: "hub:admin", granted: true, checked: false, note: "See and revoke this machine's other browser installations" });
    expect(scopeChoices(granted, [...selected, "hub-admin"]).find((choice) => choice.scope === "hub-admin")!.checked).toBe(true);
  });
  it("is not offered by an invitation that does not grant it", () => {
    expect(scopeChoices(["machine-read"], ["machine-read"]).some((choice) => choice.scope === "hub-admin")).toBe(false);
  });
});
