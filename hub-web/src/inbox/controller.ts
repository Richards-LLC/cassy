// The operator inbox of one browser profile (cas-9b7d S3).
//
// It loads independently of any MachineConnection: with every hub off, an
// enrolled profile still signs in, replays and renders retained history.
//
// - One replayer per profile: the round runs under a Web Lock when the
//   browser has them, so parallel tabs never page the same cursor at once;
//   the others learn of new events through a BroadcastChannel hint and read
//   them from IndexedDB.
// - A revoked or expired grant wipes this profile's inbox keys and plaintext
//   (downloaded plaintext elsewhere cannot be recalled; that is disclosed at
//   consent) and returns to signed-out.
// - A feed generation change (account reset) is surfaced as a warning, then
//   the device restarts on the new generation with its new keys.

import { cancelOfflineMessage, queueOfflineMessage, syncCommands, type CommandContext, type CommandTarget } from "./commands";
import { EpochKeyError, completeEnrollment, pollEnrollment, refreshEpochKeys, startEnrollment } from "./enrollment";
import { IssuerKeys } from "./issuer";
import { parsePresenceSnapshot, type PresenceSnapshot } from "./presence";
import { replayRound, ReplayPageError, type MachineDirectory, type ReplayContext } from "./replay";
import type { InboxEvent, InboxIdentity, InboxStore, PendingEnrollment, QueuedCommand } from "./store";
import { OperatorClient, OperatorWireError, record, type Fetcher } from "./wire";

export type InboxState =
  | { kind: "signed_out" }
  | { kind: "awaiting_approval"; userCode: string; approvalUrl: string; expiresAt: string }
  | { kind: "ready"; accountHint: string | null; label: string }
  | { kind: "revoked"; reason: string }
  | { kind: "unavailable"; reason: string };

export interface InboxMachine {
  machineId: string;
  hubId: string;
  label: string | null;
  status: string;
  projects: string[];
}

export interface InboxSnapshot {
  state: InboxState;
  events: InboxEvent[];
  commands: QueuedCommand[];
  machines: InboxMachine[];
  /** Set after an account reset until the operator dismisses it. */
  generationWarning: string | null;
  /** Expired history this device accepted, for "History expired" markers. */
  expiredThrough: string | null;
  presence?: PresenceSnapshot | null;
  presenceError?: string | null;
}

export interface ControllerOptions {
  origin: string;
  store: InboxStore;
  pageOrigin: string;
  fetch?: Fetcher;
  now?: () => number;
  /** Web Locks (navigator.locks) when available. */
  locks?: Pick<LockManager, "request"> | null;
  channel?: Pick<BroadcastChannel, "postMessage" | "addEventListener"> | null;
}

const PRINCIPALS_TTL_MS = 5 * 60_000;
const LOCK_NAME = "cas-operator-inbox-replay";

export class OperatorInboxController {
  readonly client: OperatorClient;
  readonly issuer: IssuerKeys;
  private identity: InboxIdentity | null = null;
  private pending: PendingEnrollment | null = null;
  private state: InboxState = { kind: "signed_out" };
  private machinesCache: { at: number; machines: InboxMachine[] } | null = null;
  private generationWarning: string | null = null;
  private presenceSnapshot: PresenceSnapshot | null = null;
  private presenceError: string | null = null;
  private presenceFetchedAt = -Infinity;
  private presenceRequest: Promise<void> | null = null;
  private readonly monitoringDecisions = new Map<string, { enabled: boolean; decisionId: string; generation: string }>();
  private readonly listeners = new Set<(snapshot: InboxSnapshot) => void>();
  private readonly now: () => number;

  constructor(private readonly options: ControllerOptions) {
    this.client = new OperatorClient({ origin: options.origin, fetch: options.fetch, now: options.now });
    this.issuer = new IssuerKeys(async () => {
      const response = await (options.fetch ?? globalThis.fetch)(`${this.client.origin}/api/operator/jwks`, { credentials: "omit" });
      if (!response.ok) throw new Error(`jwks ${response.status}`);
      return response.json();
    }, options.now);
    this.now = options.now ?? Date.now;
    options.channel?.addEventListener("message", () => void this.emit());
  }

  /** Restore this profile's identity or pending sign-in from storage. */
  async load(): Promise<InboxState> {
    this.identity = await this.options.store.loadIdentity();
    this.pending = this.identity ? null : await this.options.store.loadPending();
    if (this.identity) {
      this.client.credential = {
        key: this.identity.signingKey,
        grantId: this.identity.grant.grantId,
        generation: this.identity.grant.generation,
      };
      this.state = { kind: "ready", accountHint: this.identity.emailHint, label: this.identity.label };
    } else if (this.pending && Date.parse(this.pending.expiresAt) > this.now()) {
      this.state = { kind: "awaiting_approval", userCode: this.pending.userCode, approvalUrl: this.pending.approvalUrl, expiresAt: this.pending.expiresAt };
    } else {
      this.pending = null;
      this.state = { kind: "signed_out" };
    }
    await this.emit();
    return this.state;
  }

  subscribe(listener: (snapshot: InboxSnapshot) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  current(): InboxState {
    return this.state;
  }

  async snapshot(): Promise<InboxSnapshot> {
    const identity = this.identity;
    const events = identity ? await this.options.store.events(identity.accountId, identity.feedGeneration) : [];
    const commands = identity ? await this.options.store.commands(identity.accountId) : [];
    const cursor = identity ? await this.options.store.loadCursor(identity.accountId, identity.feedGeneration) : null;
    return {
      state: this.state,
      events,
      commands,
      machines: this.machinesCache?.machines ?? [],
      generationWarning: this.generationWarning,
      expiredThrough: cursor?.acceptedExpiredThrough ?? null,
      presence: this.presenceSnapshot,
      presenceError: this.presenceError,
    };
  }

  private async emit(): Promise<void> {
    if (this.listeners.size === 0) return;
    const snapshot = await this.snapshot();
    for (const listener of this.listeners) listener(snapshot);
  }

  private async changed(): Promise<void> {
    this.options.channel?.postMessage({ type: "inbox-changed" });
    await this.emit();
  }

  // ---------------------------------------------------------------- sign-in

  async beginSignIn(label: string, email?: string): Promise<InboxState> {
    this.pending = await startEnrollment(this.client, this.options.store, { pageOrigin: this.options.pageOrigin, label, email });
    this.state = { kind: "awaiting_approval", userCode: this.pending.userCode, approvalUrl: this.pending.approvalUrl, expiresAt: this.pending.expiresAt };
    await this.changed();
    return this.state;
  }

  /** One poll; returns the next delay in ms, or null when sign-in settled. */
  async pollSignIn(): Promise<number | null> {
    const pending = this.pending;
    if (!pending) return null;
    const outcome = await pollEnrollment(this.client, pending);
    if (outcome.status === "pending") {
      pending.intervalS = outcome.intervalS;
      return outcome.intervalS * 1000;
    }
    if (outcome.status !== "approved") {
      await this.options.store.savePending(null);
      this.pending = null;
      this.state = { kind: "unavailable", reason: outcome.status === "denied" ? "Sign-in was denied." : "The sign-in code expired." };
      await this.changed();
      return null;
    }
    this.identity = await completeEnrollment(this.client, this.options.store, this.issuer, pending, outcome.emailHint);
    this.pending = null;
    this.state = { kind: "ready", accountHint: this.identity.emailHint, label: this.identity.label };
    await this.changed();
    return null;
  }

  async cancelSignIn(): Promise<void> {
    await this.options.store.savePending(null);
    this.pending = null;
    this.state = { kind: "signed_out" };
    await this.changed();
  }

  async signOut(): Promise<void> {
    const identity = this.identity;
    if (!identity) return;
    try {
      await this.client.signOut();
    } catch (error) {
      if (!(error instanceof OperatorWireError) || error.retryable) throw error;
    }
    await this.forget({ kind: "signed_out" });
  }

  private async forget(next: InboxState): Promise<void> {
    if (this.identity) await this.options.store.wipe(this.identity.accountId);
    this.identity = null;
    this.client.credential = null;
    this.machinesCache = null;
    this.presenceSnapshot = null;
    this.presenceError = null;
    this.presenceFetchedAt = -Infinity;
    this.monitoringDecisions.clear();
    this.state = next;
    await this.changed();
  }

  // ---------------------------------------------------------------- replay

  canManageMonitoring(): boolean {
    return this.identity?.grant.capabilities.includes("account:manage") ?? false;
  }

  /** Snapshot polling is independent of hub reachability and feed replay. */
  async refreshPresence(force = false): Promise<void> {
    const identity = this.identity;
    if (!identity || (!force && this.now() - this.presenceFetchedAt < 30_000)) return;
    if (this.presenceRequest) {
      await this.presenceRequest;
      // A request started before a successful consent change may return
      // the old generation. A forced refresh must read after that change.
      if (force && this.identity === identity) await this.refreshPresence(true);
      return;
    }
    const work = async () => {
      try {
        const response = await this.client.request({ method: "GET", path: "/api/operator/machine-presence", auth: "grant" });
        const snapshot = parsePresenceSnapshot(response);
        if (this.identity !== identity) return;
        this.presenceSnapshot = snapshot;
        this.presenceError = null;
      } catch (error) {
        if (this.identity !== identity) return;
        if (error instanceof OperatorWireError && ["grant_revoked", "grant_expired", "grant_unknown"].includes(error.code)) {
          await this.forget({ kind: "revoked", reason: "This device was signed out of the operator inbox." });
          return;
        }
        this.presenceError = "Can't refresh machine status. Any last report below is from an earlier check.";
      }
      if (this.identity !== identity) return;
      this.presenceFetchedAt = this.now();
      await this.emit();
    };
    this.presenceRequest = work();
    try { await this.presenceRequest; } finally { this.presenceRequest = null; }
  }

  /** Keep an unanswered decision ID/body for an explicit retry (§17.2). */
  async setMonitoring(machineId: string, enabled: boolean): Promise<void> {
    const identity = this.identity;
    if (!identity || !this.canManageMonitoring()) throw new Error("This browser cannot change account monitoring.");
    const machine = this.presenceSnapshot?.machines.find((machine) => machine.machineId === machineId);
    if (!machine || machine.monitoring === "not_capable") throw new Error("Monitoring is unavailable for this machine.");
    let decision = this.monitoringDecisions.get(machineId);
    if (!decision || decision.enabled !== enabled) {
      const bytes = crypto.getRandomValues(new Uint8Array(16));
      const decisionId = btoa(String.fromCharCode(...bytes)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
      decision = { enabled, decisionId, generation: machine.monitoringGeneration };
      this.monitoringDecisions.set(machineId, decision);
    }
    try {
      const response = await this.client.request({
        method: "PUT", path: `/api/operator/machines/${encodeURIComponent(machineId)}/monitoring`, auth: "grant",
        body: { wire_version: 1, enabled, decision_id: decision.decisionId, expected_monitoring_generation: decision.generation },
      });
      if (this.identity !== identity) return;
      const monitoring = record(response.monitoring, "monitoring receipt");
      if (response.wire_version !== 1 || response.machine_id !== machineId || monitoring.decision_id !== decision.decisionId || monitoring.state !== (enabled ? "enabled" : "disabled")) {
        throw new Error("The monitoring decision could not be verified.");
      }
      this.monitoringDecisions.delete(machineId);
      await this.refreshPresence(true);
    } catch (error) {
      if (this.identity === identity && error instanceof OperatorWireError && !error.retryable) {
        this.monitoringDecisions.delete(machineId);
        await this.refreshPresence(true);
      }
      throw error;
    }
  }

  async machines(force = false): Promise<InboxMachine[]> {
    if (!this.identity) return [];
    if (!force && this.machinesCache && this.now() - this.machinesCache.at < PRINCIPALS_TTL_MS) return this.machinesCache.machines;
    const principals = await this.client.principals();
    const machines = (Array.isArray(principals.machines) ? principals.machines : []).map((entry) => {
      const machine = record(entry, "machine");
      return {
        machineId: String(machine.machine_id ?? ""),
        hubId: String(machine.hub_id ?? ""),
        label: typeof machine.label === "string" ? machine.label : null,
        status: String(machine.status ?? ""),
        projects: Array.isArray(machine.projects) ? machine.projects.filter((p): p is string => typeof p === "string") : [],
      };
    });
    this.machinesCache = { at: this.now(), machines };
    return machines;
  }

  private context(identity: InboxIdentity): ReplayContext & CommandContext {
    const refreshKeys = async () => {
      await refreshEpochKeys(this.client, this.options.store, this.issuer, identity);
    };
    return {
      client: this.client,
      store: this.options.store,
      issuer: this.issuer,
      identity,
      refreshKeys,
      machines: async (): Promise<MachineDirectory> => {
        const known = new Set((await this.machines(true)).map((machine) => machine.machineId));
        return { has: (id) => known.has(id) };
      },
    };
  }

  private async withReplayLock<T>(work: () => Promise<T>): Promise<T | null> {
    const locks = this.options.locks;
    if (!locks) return work();
    let result: T | null = null;
    await locks.request(LOCK_NAME, { ifAvailable: true }, async (lock) => {
      if (!lock) return; // another tab of this profile is replaying
      result = await work();
    });
    return result;
  }

  /**
   * One replay round plus command sync. Returns the delay before the next
   * round in ms. Errors map to state, never to an empty "caught up".
   */
  async runOnce(): Promise<number> {
    const identity = this.identity;
    if (!identity) return 30_000;
    // Presence is a separate cloud resource: a feed outage must not prevent
    // the phone seeing machine status or a device revocation.
    await this.refreshPresence();
    if (this.identity !== identity) return 30_000;
    try {
      const outcome = await this.withReplayLock(() => replayRound(this.context(identity)));
      if (this.identity !== identity) return 30_000;
      if (outcome?.kind === "generation_changed") {
        this.generationWarning = "The account inbox was reset. Earlier history on this device is from the previous inbox.";
        identity.feedGeneration = outcome.feedGeneration;
        await this.options.store.saveIdentity(identity);
        await refreshEpochKeys(this.client, this.options.store, this.issuer, identity);
        await this.changed();
        return 0;
      }
      if (outcome?.kind === "cursor_ahead") {
        this.state = { kind: "unavailable", reason: "The cloud inbox reported fewer messages than this device holds; replay is paused." };
        await this.changed();
        return 60_000;
      }
      const before = JSON.stringify((await this.options.store.commands(identity.accountId)).map((command) => [command.commandId, command.state]));
      const synced = await syncCommands(this.context(identity)).catch(() => null);
      if (synced && JSON.stringify(synced.map((command) => [command.commandId, command.state])) !== before) await this.changed();
      // Machine labels and command bindings for the inbox view (cached 5 min).
      const hadMachines = this.machinesCache !== null;
      await this.machines().catch(() => undefined);
      if (!hadMachines && this.machinesCache) await this.changed();
      if (this.identity !== identity) return 30_000;
      if (this.state.kind !== "ready") this.state = { kind: "ready", accountHint: identity.emailHint, label: identity.label };
      if (outcome && (outcome.stored.length > 0 || outcome.kind === "more")) await this.changed();
      if (!outcome) return 5_000;
      return outcome.kind === "more" ? 0 : Math.max(outcome.pollAfterMs, 5_000);
    } catch (error) {
      if (error instanceof OperatorWireError && ["grant_revoked", "grant_expired", "grant_unknown"].includes(error.code)) {
        await this.forget({ kind: "revoked", reason: "This device was signed out of the operator inbox." });
        return 30_000;
      }
      if (error instanceof EpochKeyError || error instanceof ReplayPageError) {
        this.state = { kind: "unavailable", reason: "The cloud inbox sent data this device could not verify; nothing was stored." };
        await this.changed();
        return 60_000;
      }
      const retryAfter = error instanceof OperatorWireError ? error.retryAfterMs : null;
      return retryAfter ?? 15_000;
    }
  }

  dismissGenerationWarning(): void {
    this.generationWarning = null;
    void this.emit();
  }

  // ---------------------------------------------------------------- commands

  async queueMessage(target: CommandTarget, body: string): Promise<QueuedCommand> {
    const identity = this.identity;
    if (!identity) throw new Error("not signed in to the operator inbox");
    const command = await queueOfflineMessage(this.context(identity), target, body);
    await this.changed();
    return command;
  }

  async cancelMessage(commandId: string): Promise<QueuedCommand | null> {
    const identity = this.identity;
    if (!identity) return null;
    const command = await cancelOfflineMessage(this.context(identity), commandId);
    await this.changed();
    return command;
  }

  /** Command scopes this device holds (for offering offline replies). */
  commandScopes() {
    return this.identity?.grant.scopes ?? [];
  }
}

/** Drive `runOnce` on its own schedule until `signal` aborts. */
export function startInboxLoop(controller: OperatorInboxController, signal: AbortSignal): void {
  let timer: ReturnType<typeof setTimeout> | null = null;
  const tick = async () => {
    if (signal.aborted) return;
    const delay = await controller.runOnce();
    if (!signal.aborted) timer = setTimeout(() => void tick(), delay);
  };
  signal.addEventListener("abort", () => {
    if (timer) clearTimeout(timer);
  });
  void tick();
}
