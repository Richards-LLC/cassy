import "./styles.css";
import { HubConnectionSupervisor, type ConnectionState, type HubMachineInfo } from "./connection";
import { ensureMachineConnection, replaceMachineConnection } from "./connection-lifecycle";
import { createDeviceKey } from "./dpop";
import { consumePairingFragment } from "./fragment";
import { createPairingDraft, updatePairingDraft } from "./pairing-draft";
import { bindPairingDialogCancel } from "./pairing-dialog";
import { pairingCleanupFailureUpdate, pairingStorageClearFailureMessage } from "./pairing-cleanup";
import { exchangePendingPairing, PairingCleanupError, PairingExchangeError } from "./pairing-exchange";
import { PairingOperationCoordinator, commitPairingResult } from "./pairing-operation";
import { pendingPairingStoreFor, type PendingPairing, type PendingRelayRequest } from "./pending-pairing";
import { DEFAULT_PAIRING_SCOPES, PairingRelayError, acknowledgePairing, createPairingRequest, pairingRelayOrigin, pollPairingRequest } from "./pairing-relay";
import { attentionStore, catalog } from "./storage";
import { createTerminalSurface, type TerminalSurface } from "./terminal";
import type { AttentionItem, HubSession, LeaseState, PaneInfo, Scope, SessionState, StoredMachine } from "./types";

const pendingPairingStore = pendingPairingStoreFor(window);
const relayOrigin = pairingRelayOrigin(document.querySelector<HTMLMetaElement>('meta[name="cas-pairing-relay-origin"]')?.content ?? null);
let pendingPairing: PendingPairing | null = consumePairingFragment(window.location, window.history, pendingPairingStore);
pendingPairing ??= pendingPairingStore.load();
const pairingOperations = new PairingOperationCoordinator();
const app = document.querySelector<HTMLDivElement>("#app")!;
const machines = new Map<string, StoredMachine>();
const sessions = new Map<string, HubSession[]>();
const connections = new Map<string, HubConnectionSupervisor>();
const connectionStates = new Map<string, ConnectionState>();
const machineInfo = new Map<string, HubMachineInfo | undefined>();
const statuses = new Map<string, Record<string, unknown>>();
const leases = new Map<string, LeaseState>();
const surfaces = new Map<string, TerminalSurface>();
const sessionStates = new Map<string, SessionState>();
const paneBuffers = new Map<string, number[]>();
const selectedPanes = new Map<string, string>();
const leaseHeartbeats = new Map<string, number>();
const leaseExpiryTimers = new Map<string, number>();
let attention: AttentionItem[] = [];
let selectedMachineId: string | undefined;
let selectedSession: string | undefined;
let pairingStatus = pendingPairing?.kind === "relay-request" ? "Waiting for a machine to claim the code…" : "";
let pairingPollTimer: number | undefined;
let pairingCountdownTimer: number | undefined;
let pairingCreateInFlight = false;
let pairingExchangeInFlight = false;
let pairingDraft = createPairingDraft(location.origin);

function sessionKey(machineId: string, session: string): string { return `${machineId}:${session}`; }
function paneKey(machineId: string, session: string, pane: string): string { return `${machineId}:${session}:${pane}`; }
function activeConnection(): HubConnectionSupervisor | undefined { return selectedMachineId ? connections.get(selectedMachineId) : undefined; }

async function boot(): Promise<void> {
  const stored = await catalog.recoverPending();
  for (const machine of stored.machines) machines.set(machine.id, machine);
  if (stored.pendingCleanup > 0) {
    pairingStatus = "A canceled credential remains blocked while durable local cleanup is pending.";
  }
  attention = (await attentionStore.list()).toSorted((a, b) => b.createdAt.localeCompare(a.createdAt));
  selectedMachineId = machines.keys().next().value;
  render();
  for (const machine of machines.values()) ensureConnection(machine);
  resumePairingPoll();
}

function createConnection(machine: StoredMachine): HubConnectionSupervisor {
  return new HubConnectionSupervisor(machine, {
    onState: (state, detail) => {
      connectionStates.set(machine.id, state);
      if (state === "reconnecting" || state === "auth-blocked" || state === "offline") invalidateMachineLeases(machine.id);
      if (state === "auth-blocked") void addAttention(machine, undefined, "auth_loss", detail ?? "Authentication blocked");
      if (state === "reconnecting") void addAttention(machine, undefined, "hub_disconnected", detail ?? "Hub disconnected");
      render();
    },
    onMachineInfo: (info) => { machineInfo.set(machine.id, info); render(); },
    onSessions: (items) => { sessions.set(machine.id, items); render(); },
    onMachineEvent: (event) => {
      const kind = String(event.kind ?? "hub_event");
      if (["daemon_disconnected", "pane_exited", "session_removed"].includes(kind)) {
        void addAttention(machine, event.session as string | undefined, kind, event.diagnostic ? `Daemon ended: ${JSON.stringify(event.diagnostic)}` : kind.replaceAll("_", " "));
      }
      if (selectedMachineId === machine.id && selectedSession) void loadStatus(machine.id, selectedSession);
      if (selectedMachineId === machine.id && selectedSession) void loadLease(machine.id, selectedSession);
    },
    onSessionState: (session, state, scrollback) => void renderSessionState(machine.id, session, state, scrollback),
    onOutput: (session, pane, data) => {
      const key = paneKey(machine.id, session, pane);
      const buffered = [...(paneBuffers.get(key) ?? []), ...data];
      paneBuffers.set(key, buffered.slice(-2_000_000));
      surfaces.get(key)?.write(data);
    },
    onSocketError: (session, detail) => void addAttention(machine, session, "session_transport", detail),
  });
}

function ensureConnection(machine: StoredMachine): HubConnectionSupervisor {
  return ensureMachineConnection(machine, connections, createConnection);
}

async function addAttention(machine: StoredMachine, session: string | undefined, kind: string, message: string): Promise<void> {
  const id = `${machine.id}:${session ?? "machine"}:${kind}:${message}`;
  if (attention.some((item) => item.id === id && !item.acknowledgedAt)) return;
  const item: AttentionItem = { id, machineId: machine.id, machineLabel: machine.label, session, kind, message, createdAt: new Date().toISOString() };
  attention = [item, ...attention];
  await attentionStore.put(item);
  render();
}

async function acknowledgeAttention(id: string): Promise<void> {
  const item = attention.find((candidate) => candidate.id === id);
  if (!item) return;
  item.acknowledgedAt = new Date().toISOString();
  await attentionStore.put(item);
  render();
}

async function pairMachine(form: HTMLFormElement): Promise<boolean> {
  const invitation = pendingPairing?.kind === "invitation" ? pendingPairing : null;
  if (!invitation) throw new Error("Create a pairing request or open a one-time pairing link first.");
  const values = new FormData(form);
  pairingDraft = updatePairingDraft(pairingDraft, values.entries(), !invitation.hubUrl);
  const operation = pairingOperations.begin();
  pairingExchangeInFlight = true;
  pairingStatus = "Creating this browser credential… Cancel stops local installation.";
  render();
  let machine: StoredMachine;
  try {
    machine = await exchangePendingPairing({
      invitation,
      controllerOrigin: location.origin,
      legacyHubUrl: invitation.hubUrl ? undefined : String(values.get("url")),
      machineLabel: String(values.get("label")),
      deviceLabel: String(values.get("device")),
      operatorLabel: String(values.get("operator")),
      requestedScopes: values.getAll("scope") as Scope[],
      fetcher: fetch,
      createKey: createDeviceKey,
      installationGeneration: operation.generation,
      stagePersisted: (candidate, identity) => catalog.stage(candidate, identity, operation.signal),
      activatePersisted: (identity, signal) => catalog.activate(identity, signal),
      rollbackPersisted: (identity) => catalog.rollback(identity),
      acknowledge: relayOrigin ? (relay, signal) => acknowledgePairing(fetch, relayOrigin, relay, signal) : undefined,
      signal: operation.signal,
      isCurrent: () => pairingOperations.isCurrent(operation),
    });
  } catch (error) {
    if (error instanceof PairingCleanupError) {
      const update = pairingCleanupFailureUpdate({
        coordinator: pairingOperations,
        operation,
        expectedPending: invitation,
        current: { pendingPairing, pairingDraft, exchangeInFlight: pairingExchangeInFlight, status: pairingStatus },
        cleanupMessage: error.message,
        resetDraft: () => createPairingDraft(location.origin),
      });
      if (!update) return false;
      const cleared = pendingPairingStore.clear();
      pendingPairing = update.pendingPairing;
      pairingDraft = update.pairingDraft;
      pairingExchangeInFlight = update.exchangeInFlight;
      pairingStatus = cleared.failClosed
        ? `${update.status}${cleared.persistentRemovalFailed ? " Browser storage removal was denied; the cancelled request is durably blocked." : ""}`
        : `${update.status} Browser storage could not durably block the cancelled request.`;
      render(false);
      throw error;
    }
    if (!pairingOperations.isCurrent(operation)) {
      if (!pendingPairing) {
        pairingStatus = "Pairing cancelled after durable local cleanup completed.";
        render(false);
      }
      return false;
    }
    pairingExchangeInFlight = false;
    if (error instanceof PairingExchangeError) {
      pairingOperations.invalidate();
      const cleared = pendingPairingStore.clear();
      pendingPairing = null;
      pairingDraft = createPairingDraft(location.origin);
      pairingStatus = pairingStorageClearFailureMessage(error.message, cleared);
      render(false);
    } else {
      render();
    }
    throw error;
  } finally {
    pairingOperations.finish(operation);
  }
  if (!pairingOperations.isCurrent(operation)) return false;
  pairingExchangeInFlight = false;
  pairingOperations.invalidate();
  pendingPairingStore.clear();
  pendingPairing = null;
  stopPairingTimers();
  pairingDraft = createPairingDraft(location.origin);
  machines.set(machine.id, machine);
  selectedMachineId = machine.id;
  replaceMachineConnection(machine, connections, connectionStates, createConnection);
  render(false);
  return true;
}

async function startRelayPairing(email: string): Promise<boolean> {
  if (!relayOrigin) throw new PairingRelayError("relay_unavailable", "Page-initiated pairing is unavailable in this deployment.");
  if (pairingCreateInFlight) return false;
  const generation = pairingOperations.replace();
  stopPairingTimers();
  pendingPairingStore.clear();
  pendingPairing = null;
  pairingCreateInFlight = true;
  pairingDraft.email = email;
  pairingStatus = "Creating a pairing code…";
  render();
  const operation = pairingOperations.begin(generation);
  let created: PendingRelayRequest | undefined;
  try {
    const committed = await commitPairingResult(
      pairingOperations,
      operation,
      createPairingRequest(fetch, relayOrigin, location.origin, DEFAULT_PAIRING_SCOPES, email || undefined, operation.signal),
      (value) => { created = value; },
    );
    if (!committed || !created) return false;
  } catch (error) {
    if (!pairingOperations.isCurrent(operation)) return false;
    pairingCreateInFlight = false;
    throw error;
  }
  pairingCreateInFlight = false;
  pendingPairing = created;
  pendingPairingStore.save(created);
  pairingStatus = "Waiting for a machine to claim the code…";
  render();
  resumePairingPoll();
  return true;
}

function resumePairingPoll(): void {
  if (!relayOrigin || pairingPollTimer !== undefined || pendingPairing?.kind !== "relay-request") return;
  const request = pendingPairing;
  pairingPollTimer = window.setTimeout(() => void pollRelay(request), request.interval * 1000);
}

async function pollRelay(request: PendingRelayRequest): Promise<void> {
  pairingPollTimer = undefined;
  if (pendingPairing?.kind !== "relay-request" || pendingPairing.pairingRequestId !== request.pairingRequestId) return;
  const operation = pairingOperations.begin();
  try {
    if (!relayOrigin) throw new PairingRelayError("relay_unavailable", "Page-initiated pairing is unavailable in this deployment.");
    const result = await pollPairingRequest(fetch, relayOrigin, request, operation.signal);
    if (!pairingOperations.isCurrent(operation) || pendingPairing?.kind !== "relay-request" || pendingPairing.pairingRequestId !== request.pairingRequestId) return;
    if (result.kind === "authorized") {
      pendingPairing = result.invitation;
      pendingPairingStore.save(result.invitation);
      pairingStatus = "Machine authorized. Confirm the exact details and finish pairing.";
      render();
      return;
    }
    request.interval = result.interval;
    if (result.kind !== "slow-down") request.expiresAt = result.expiresAt;
    pendingPairingStore.save(request);
    pairingStatus = result.kind === "claimed" ? "Machine claimed the code. Waiting for local approval…" : result.kind === "slow-down" ? `Polling slowed to ${result.interval} seconds.` : "Waiting for a machine to claim the code…";
    render();
    resumePairingPoll();
  } catch (error) {
    if (!pairingOperations.isCurrent(operation) || pendingPairing?.kind !== "relay-request" || pendingPairing.pairingRequestId !== request.pairingRequestId) return;
    const terminal = error instanceof PairingRelayError && ["request_mismatch", "expired_request"].includes(error.code);
    if (terminal) {
      pairingOperations.invalidate();
      pendingPairingStore.clear();
      pendingPairing = null;
    }
    pairingStatus = error instanceof PairingRelayError ? error.message : "The pairing service is unavailable. Retrying without discarding this request.";
    render();
    if (!terminal) resumePairingPoll();
  } finally {
    pairingOperations.finish(operation);
  }
}

function cancelPendingPairing(): void {
  const verifiesCleanup = pairingExchangeInFlight;
  document.querySelector<HTMLDialogElement>("#pair-dialog")?.close();
  pairingOperations.invalidate();
  const cleared = pendingPairingStore.clear();
  pendingPairing = null;
  pairingCreateInFlight = false;
  pairingExchangeInFlight = false;
  pairingDraft = createPairingDraft(location.origin);
  pairingStatus = !cleared.failClosed
    ? "Pairing cancellation could not durably block the request; keep this page open and retry after storage access is restored."
    : verifiesCleanup
      ? "Cancelling pairing and verifying durable local cleanup…"
      : cleared.persistentRemovalFailed
        ? "Pairing cancelled. Browser storage removal was denied, so the request was durably blocked."
        : "Pairing cancelled.";
  stopPairingTimers();
  render(false);
}

function stopPairingTimers(): void {
  if (pairingPollTimer !== undefined) window.clearTimeout(pairingPollTimer);
  if (pairingCountdownTimer !== undefined) window.clearInterval(pairingCountdownTimer);
  pairingPollTimer = undefined;
  pairingCountdownTimer = undefined;
}

function syncPairingCountdown(): void {
  if (pairingCountdownTimer !== undefined) window.clearInterval(pairingCountdownTimer);
  pairingCountdownTimer = undefined;
  const expiresAt = pendingPairing?.expiresAt;
  if (!expiresAt) return;
  const update = () => {
    const output = document.querySelector<HTMLElement>("#pair-countdown");
    const remaining = Math.max(0, Date.parse(expiresAt) - Date.now());
    if (output) output.textContent = `${Math.floor(remaining / 60_000)}:${String(Math.floor(remaining / 1000) % 60).padStart(2, "0")}`;
    if (remaining === 0) {
      pairingOperations.invalidate();
      pendingPairingStore.clear();
      pendingPairing = null;
      pairingCreateInFlight = false;
      pairingExchangeInFlight = false;
      pairingDraft = createPairingDraft(location.origin);
      pairingStatus = "This pairing request has expired.";
      stopPairingTimers();
      render(false);
    }
  };
  update();
  pairingCountdownTimer = window.setInterval(update, 1000);
}

async function openSession(machineId: string, session: string): Promise<void> {
  selectedMachineId = machineId;
  selectedSession = session;
  render();
  await Promise.all([loadStatus(machineId, session), loadLease(machineId, session)]);
  await connections.get(machineId)?.attach(session);
}

async function loadStatus(machineId: string, session: string): Promise<void> {
  const connection = connections.get(machineId);
  const machine = machines.get(machineId);
  if (!connection || !machine) return;
  try {
    const status = await connection.status(session);
    statuses.set(sessionKey(machineId, session), status);
    const tasks = [...((status.tasks_in_progress as any[]) ?? []), ...((status.tasks_ready as any[]) ?? [])];
    for (const task of tasks) {
      if (["blocked", "awaiting_merge", "awaitingmerge"].includes(String(task.status))) {
        await addAttention(machine, session, String(task.status), `${task.id}: ${task.title}`);
      }
    }
    render();
  } catch { /* connection supervisor owns transport/auth reporting */ }
}

async function loadLease(machineId: string, session: string): Promise<void> {
  try {
    const state = await connections.get(machineId)?.lease(session);
    if (state) {
      const key = sessionKey(machineId, session);
      const becameController = state.held_by_me && !leases.get(key)?.held_by_me;
      leases.set(key, state);
      if (becameController) resizeControlledPanes(machineId, session);
      const expiryTimer = leaseExpiryTimers.get(key);
      if (expiryTimer !== undefined) window.clearTimeout(expiryTimer);
      if (state.expires_at) {
        const delay = Math.max(0, new Date(state.expires_at).getTime() - Date.now() + 100);
        leaseExpiryTimers.set(key, window.setTimeout(() => void loadLease(machineId, session), delay));
      }
      if (state.held_by_me) startLeaseHeartbeat(machineId, session);
    }
    render();
  } catch { /* legacy hub may not expose lease status */ }
}

function resizeControlledPanes(machineId: string, session: string): void {
  const state = sessionStates.get(sessionKey(machineId, session));
  if (!state) return;
  for (const pane of state.panes.filter((candidate) => candidate.kind !== "Director")) {
    const surface = surfaces.get(paneKey(machineId, session, pane.id));
    if (surface) sendControl(machineId, session, { ResizePane: { pane_id: pane.id, cols: surface.cols, rows: surface.rows } });
  }
}

async function renderSessionState(machineId: string, session: string, state: SessionState, scrollback?: Record<string, number[][]>): Promise<void> {
  sessionStates.set(sessionKey(machineId, session), state);
  if (scrollback) {
    for (const [pane, chunks] of Object.entries(scrollback)) {
      paneBuffers.set(paneKey(machineId, session, pane), chunks.flat().slice(-2_000_000));
    }
  }
  if (selectedMachineId !== machineId || selectedSession !== session) return;
  const grid = document.querySelector<HTMLElement>("#pane-grid");
  if (!grid) return;
  const active = new Set(state.panes.filter((pane) => pane.kind !== "Director").map((pane) => pane.id));
  const selectedKey = sessionKey(machineId, session);
  const selectedPane = selectedPanes.get(selectedKey);
  if (!selectedPane || !active.has(selectedPane)) {
    const fallback = state.panes.find((pane) => pane.kind !== "Director" && pane.focused)
      ?? state.panes.find((pane) => pane.kind !== "Director");
    if (fallback) selectedPanes.set(selectedKey, fallback.id);
  }
  for (const [key, surface] of surfaces) {
    if (key.startsWith(`${machineId}:${session}:`) && !active.has(key.split(":").at(-1)!)) { surface.dispose(); surfaces.delete(key); }
  }
  for (const pane of state.panes.filter((candidate) => candidate.kind !== "Director")) {
    const key = paneKey(machineId, session, pane.id);
    let mount = grid.querySelector<HTMLElement>(`[data-pane-id="${CSS.escape(pane.id)}"] .terminal-mount`);
    if (!mount) {
      const card = document.createElement("section");
      card.className = "pane";
      card.dataset.paneId = pane.id;
      if (selectedPanes.get(selectedKey) === pane.id) card.classList.add("selected");
      card.onclick = () => {
        selectedPanes.set(selectedKey, pane.id);
        for (const sibling of grid.querySelectorAll(".pane.selected")) sibling.classList.remove("selected");
        card.classList.add("selected");
        surfaces.get(key)?.focus();
      };
      const title = document.createElement("header");
      title.textContent = `${pane.title || pane.id} · ${pane.kind.toLowerCase()}${pane.exited ? " · exited" : ""}`;
      mount = document.createElement("div"); mount.className = "terminal-mount";
      card.append(title, mount); grid.append(card);
    }
    const existingSurface = surfaces.get(key);
    if (existingSurface && (existingSurface.element !== mount || !existingSurface.element.isConnected)) {
      existingSurface.dispose();
      surfaces.delete(key);
    }
    if (!surfaces.has(key)) {
      const surface = await createTerminalSurface(mount, {
        onData: (data) => { if (canControl(machineId, session, "pane-input")) sendControl(machineId, session, { Input: { pane_id: pane.id, data: [...data] } }); },
        onResize: (cols, rows) => { if (canControl(machineId, session, "pane-input")) sendControl(machineId, session, { ResizePane: { pane_id: pane.id, cols, rows } }); },
      });
      const currentMount = document.querySelector<HTMLElement>(`[data-pane-id="${CSS.escape(pane.id)}"] .terminal-mount`);
      if (selectedMachineId !== machineId || selectedSession !== session || !mount.isConnected || currentMount !== mount) {
        surface.dispose();
        continue;
      }
      surfaces.set(key, surface);
      const buffered = paneBuffers.get(key);
      if (buffered) surface.write(new Uint8Array(buffered));
    }
  }
}

function hubSupports(machineId: string, capability: string): boolean {
  return machineInfo.get(machineId)?.capabilities.includes(capability) === true;
}

function canControl(machineId: string, session: string, scope: Scope): boolean {
  return hubSupports(machineId, "daemon_attach") && machines.get(machineId)?.scopes.includes(scope) === true && leases.get(sessionKey(machineId, session))?.held_by_me === true;
}

function sendControl(machineId: string, session: string, message: unknown): void {
  if (!connections.get(machineId)?.send(session, message)) toast("Terminal is reconnecting");
}

function startLeaseHeartbeat(machineId: string, session: string): void {
  const key = sessionKey(machineId, session);
  if (leaseHeartbeats.has(key)) return;
  leaseHeartbeats.set(key, window.setInterval(async () => {
    if (!leases.get(key)?.held_by_me) return;
    try {
      const state = await connections.get(machineId)?.acquireLease(session);
      if (state) leases.set(key, state);
    } catch {
      invalidateMachineLeases(machineId);
      render();
    }
  }, 10_000));
}

function invalidateMachineLeases(machineId: string): void {
  for (const [key, timer] of leaseHeartbeats) {
    if (key.startsWith(`${machineId}:`)) { window.clearInterval(timer); leaseHeartbeats.delete(key); }
  }
  for (const [key, lease] of leases) if (key.startsWith(`${machineId}:`)) leases.set(key, { ...lease, held_by_me: false });
}

function toast(message: string): void {
  const output = document.querySelector<HTMLElement>("#toast");
  if (!output) return;
  output.textContent = message;
  output.classList.add("visible");
  window.setTimeout(() => output.classList.remove("visible"), 3200);
}

function pairingDetails(origin: string, scopes: readonly Scope[]): string {
  return `<dl class="pair-details"><div><dt>Commander origin</dt><dd>${escapeHtml(origin)}</dd></div><div><dt>Scopes</dt><dd>${scopes.map(escapeHtml).join(", ")}</dd></div></dl>`;
}

function pairDialogMarkup(): string {
  if (pendingPairing?.kind === "relay-request") {
    return `<dialog id="pair-dialog"><section class="pair-flow"><h2>Pair this machine</h2><p>Run <code>cas hub authorize ${escapeHtml(pendingPairing.userCode)}</code> on the machine you want to pair.</p><div class="pair-code" aria-label="Pairing code">${escapeHtml(pendingPairing.userCode)}</div><p>Expires in <strong id="pair-countdown">10:00</strong></p>${pairingDetails(pendingPairing.controllerOrigin, pendingPairing.requestedScopes)}<p class="pair-status" role="status">${escapeHtml(pairingStatus)}</p><div class="dialog-actions"><button id="pair-cancel" type="button">Cancel</button></div></section></dialog>`;
  }
  if (pendingPairing?.kind === "invitation") {
    const relay = Boolean(pendingPairing.relay);
    const hubUrl = pendingPairing.hubUrl;
    const origin = pendingPairing.controllerOrigin;
    const scopes = pendingPairing.scopes;
    return `<dialog id="pair-dialog"><form id="pair-form"><h2>${relay ? "Machine authorized" : "Pair a machine"}</h2><p>${relay ? "Verify the machine details, then create this browser's device credential." : "One-time invitation ready. Confirm the target hub."}</p>${relay && hubUrl && origin && scopes ? `<dl class="pair-details"><div><dt>Machine</dt><dd>${escapeHtml(pendingPairing.machineLabel ?? pendingPairing.hubId)}</dd></div><div><dt>Hub</dt><dd>${escapeHtml(hubUrl)}</dd></div><div><dt>Commander origin</dt><dd>${escapeHtml(origin)}</dd></div><div><dt>Granted scopes</dt><dd>${scopes.map(escapeHtml).join(", ")}</dd></div></dl><p>Invitation expires in <strong id="pair-countdown">10:00</strong></p>` : `<label>Hub URL<input name="url" type="url" required value="${escapeAttr(pairingDraft.hubUrl)}"></label><label>Machine label<input name="label" required placeholder="Studio Mac" value="${escapeAttr(pairingDraft.machineLabel)}"></label><fieldset><legend>Scopes requested</legend>${scopeChecks(pairingDraft.scopes)}</fieldset>`}<label>Device label<input name="device" required value="${escapeAttr(pairingDraft.deviceLabel)}"></label><label>Operator label<input name="operator" required placeholder="Your name" value="${escapeAttr(pairingDraft.operatorLabel)}"></label>${pairingStatus ? `<p class="pair-status" role="status">${escapeHtml(pairingStatus)}</p>` : ""}<div class="dialog-actions"><button id="pair-cancel" type="button">Cancel</button><button type="submit" class="primary" ${pairingExchangeInFlight ? "disabled" : ""}>${pairingExchangeInFlight ? "Pairing…" : "Pair"}</button></div></form></dialog>`;
  }
  const relayAction = relayOrigin
    ? `<button id="pair-create" type="button" class="primary" ${pairingCreateInFlight ? "disabled" : ""}>${pairingCreateInFlight ? "Creating…" : "Create pairing code"}</button>`
    : '<p class="pairing-disabled-reason">Page-initiated pairing is unavailable because this Commander build has no reviewed relay origin.</p>';
  return `<dialog id="pair-dialog"><section class="pair-flow"><h2>Pair this machine</h2><p>Create a ten-minute code, then approve the exact origin and read-only scopes on the target machine.</p>${pairingDetails(location.origin, DEFAULT_PAIRING_SCOPES)}<label>Email code (optional)<input id="pair-email" type="email" autocomplete="email" placeholder="operator@example.com" value="${escapeAttr(pairingDraft.email)}"></label>${pairingStatus ? `<p class="pair-status" role="status">${escapeHtml(pairingStatus)}</p>` : ""}<div class="dialog-actions"><button id="pair-close" type="button">${pairingCreateInFlight ? "Cancel" : "Close"}</button>${pendingPairing ? "" : '<p class="pairing-disabled-reason">Pair is disabled until you open a pairing URL generated by <code>cas hub pair</code> on the machine.</p>'}<button type="button" ${pendingPairing ? "" : "disabled"}>Pair</button>${relayAction}</div></section></dialog>`;
}

function capturePairingDraft(): void {
  const email = document.querySelector<HTMLInputElement>("#pair-email");
  if (email) pairingDraft.email = email.value;
  const form = document.querySelector<HTMLFormElement>("#pair-form");
  if (form) pairingDraft = updatePairingDraft(pairingDraft, new FormData(form).entries(), pendingPairing?.kind === "invitation" && !pendingPairing.hubUrl);
}

function render(captureDraft = true): void {
  if (captureDraft) capturePairingDraft();
  const selected = selectedMachineId ? machines.get(selectedMachineId) : undefined;
  const machineSessions = selected ? sessions.get(selected.id) ?? [] : [];
  const lease = selected && selectedSession ? leases.get(sessionKey(selected.id, selectedSession)) : undefined;
  const status = selected && selectedSession ? statuses.get(sessionKey(selected.id, selectedSession)) : undefined;
  const compatibility = selected ? compatibilityWarning(selected.id) : undefined;
  const terminalSessionKey = selected && selectedSession ? sessionKey(selected.id, selectedSession) : undefined;
  const currentGrid = document.querySelector<HTMLElement>("#pane-grid");
  const pairDialogWasOpen = document.querySelector<HTMLDialogElement>("#pair-dialog")?.open === true;
  const preservedGrid = terminalSessionKey && currentGrid?.dataset.sessionKey === terminalSessionKey ? currentGrid : undefined;
  if (preservedGrid) {
    preservedGrid.remove();
  } else {
    for (const surface of surfaces.values()) surface.dispose();
    surfaces.clear();
  }
  app.innerHTML = `
    <div class="shell">
      <aside class="machines"><div class="brand"><span class="pulse"></span><strong>Commander</strong></div><button id="pair-toggle" class="primary">Pair this machine</button><nav id="machine-list"></nav></aside>
      <aside class="sessions"><h2>${escapeHtml(selected?.label ?? "Machines")}</h2><div class="connection ${connectionStates.get(selected?.id ?? "") ?? "idle"}">${connectionStates.get(selected?.id ?? "") ?? "select a machine"}</div>${compatibility ? `<div class="compatibility-warning" role="alert">${escapeHtml(compatibility)}</div>` : ""}${selected ? '<button id="remove-machine" class="remove-machine">Remove</button>' : ""}<nav id="session-list"></nav></aside>
      <main><header class="toolbar"><div><h1>${escapeHtml(selectedSession ?? "Fleet overview")}</h1><p>${lease?.held_by_me ? "You control this session" : lease?.controller_label ? `Observed · controlled by ${escapeHtml(lease.controller_label)}` : "Observer mode"}</p></div><div class="actions"><button id="lease" ${!selected || !selectedSession || !hubSupports(selected.id, "daemon_attach") || (!lease?.held_by_me && !selected.scopes.includes("pane-input") && !selected.scopes.includes("hub-admin")) ? "disabled" : ""}>${lease?.held_by_me ? "Release control" : lease?.controller_label && selected?.scopes.includes("hub-admin") ? "Force takeover" : "Take control"}</button><button id="interrupt" class="danger" ${!selected || !selectedSession || !canControl(selected.id, selectedSession, "pane-interrupt") ? "disabled" : ""}>Interrupt</button></div></header><section id="pane-grid" class="pane-grid"${terminalSessionKey ? ` data-session-key="${escapeAttr(terminalSessionKey)}"` : ""}><div class="empty">${selectedSession ? "Connecting to terminal…" : "Choose a live session to open its panes."}</div></section></main>
      <aside class="context"><section><h2>Attention <span class="badge">${attention.filter((item) => !item.acknowledgedAt).length}</span></h2><div id="attention-list"></div></section><section><h2>Workers & tasks</h2><div id="status-view"></div></section><section class="message"><h2>Message supervisor</h2><textarea id="message-text" placeholder="Send an attributed semantic message"></textarea><button id="message-send" ${!selected || !selectedSession || !canControl(selected.id, selectedSession, "message-send") ? "disabled" : ""}>Send message</button></section></aside>
    </div>${pairDialogMarkup()}<div id="toast" role="status"></div>`;
  if (preservedGrid) document.querySelector<HTMLElement>("#pane-grid")!.replaceWith(preservedGrid);
  const machineList = document.querySelector("#machine-list")!;
  for (const machine of machines.values()) machineList.append(machineButton(machine));
  const sessionList = document.querySelector("#session-list")!;
  for (const session of machineSessions) sessionList.append(sessionButton(selected!.id, session));
  renderAttention(); renderStatus(status);
  bindEvents(selected, lease);
  if (pairDialogWasOpen) document.querySelector<HTMLDialogElement>("#pair-dialog")?.showModal();
  if (selected && selectedSession) {
    const state = sessionStates.get(sessionKey(selected.id, selectedSession));
    if (state) queueMicrotask(() => void renderSessionState(selected.id, selectedSession!, state));
  }
  syncPairingCountdown();
}

function compatibilityWarning(machineId: string): string | undefined {
  const info = machineInfo.get(machineId);
  if (!info) return "Compatibility check unavailable: this hub may be older or newer. Read-only discovery may work, but controls stay disabled until it reports capabilities.";
  const missing = ["session_index", "daemon_attach", "machine_events"].filter((capability) => !info.capabilities.includes(capability));
  if (info.schema_version !== 1 || missing.length > 0) {
    return `Hub ${info.version} is version-skewed (schema ${info.schema_version}; missing ${missing.join(", ") || "no required capabilities"}). Upgrade or use a compatible Commander build; unsupported controls are disabled.`;
  }
  return undefined;
}

function machineButton(machine: StoredMachine): HTMLButtonElement {
  const button = document.createElement("button"); button.className = `nav-item ${machine.id === selectedMachineId ? "active" : ""}`;
  button.textContent = `${machine.label} · ${connectionStates.get(machine.id) ?? "idle"}`;
  button.onclick = () => { selectedMachineId = machine.id; selectedSession = undefined; render(); };
  return button;
}

function sessionButton(machineId: string, session: HubSession): HTMLButtonElement {
  const button = document.createElement("button"); button.className = `nav-item ${session.name === selectedSession ? "active" : ""}`;
  button.innerHTML = `<span>${escapeHtml(session.name)}</span><small>${escapeHtml(session.supervisor)} · ${session.workers.length} workers · ${session.liveness}</small>`;
  button.onclick = () => void openSession(machineId, session.name);
  return button;
}

function renderAttention(): void {
  const container = document.querySelector("#attention-list")!;
  for (const item of attention.filter((candidate) => !candidate.acknowledgedAt).slice(0, 8)) {
    const card = document.createElement("article"); card.className = "attention-item";
    const text = document.createElement("button"); text.className = "attention-open"; text.textContent = `${item.machineLabel}${item.session ? ` / ${item.session}` : ""}: ${item.message}`;
    text.onclick = () => { selectedMachineId = item.machineId; if (item.session) void openSession(item.machineId, item.session); };
    const ack = document.createElement("button"); ack.className = "ack"; ack.textContent = "Acknowledge"; ack.onclick = () => void acknowledgeAttention(item.id);
    card.append(text, ack); container.append(card);
  }
}

function renderStatus(status?: Record<string, unknown>): void {
  const container = document.querySelector("#status-view")!;
  if (!status) { container.textContent = "Open a session for push-refreshed status."; return; }
  const agents = (status.agents as any[]) ?? [];
  const tasks = [...((status.tasks_in_progress as any[]) ?? []), ...((status.tasks_ready as any[]) ?? [])];
  for (const agent of agents) { const row = document.createElement("article"); row.className = "status-row"; row.textContent = `${agent.name} · ${agent.status}${agent.current_task ? ` · ${agent.current_task}` : ""}${agent.latest_activity?.summary ? ` — ${agent.latest_activity.summary}` : ""}`; container.append(row); }
  for (const task of tasks) { const row = document.createElement("article"); row.className = "status-row"; row.textContent = `${task.id} · ${task.status} · ${task.title}`; container.append(row); }
}

function bindEvents(selected: StoredMachine | undefined, lease: LeaseState | undefined): void {
  document.querySelector<HTMLButtonElement>("#pair-toggle")!.onclick = () => (document.querySelector<HTMLDialogElement>("#pair-dialog")!).showModal();
  const pairForm = document.querySelector<HTMLFormElement>("#pair-form");
  const pairCancel = document.querySelector<HTMLButtonElement>("#pair-cancel");
  const pairClose = document.querySelector<HTMLButtonElement>("#pair-close");
  const pairCreate = document.querySelector<HTMLButtonElement>("#pair-create");
  const pairDialog = document.querySelector<HTMLDialogElement>("#pair-dialog");
  if (pairDialog) bindPairingDialogCancel(
    pairDialog,
    () => ({
      createInFlight: pairingCreateInFlight,
      exchangeInFlight: pairingExchangeInFlight,
      hasPendingPairing: pendingPairing !== null,
    }),
    cancelPendingPairing,
  );
  if (pairCancel) pairCancel.onclick = cancelPendingPairing;
  if (pairClose) pairClose.onclick = pairingCreateInFlight ? cancelPendingPairing : () => (document.querySelector<HTMLDialogElement>("#pair-dialog")!).close();
  if (pairCreate) pairCreate.onclick = () => {
    pairCreate.disabled = true;
    const email = document.querySelector<HTMLInputElement>("#pair-email")?.value.trim() ?? "";
    void startRelayPairing(email).then((created) => {
      if (!created) return;
      const dialog = document.querySelector<HTMLDialogElement>("#pair-dialog");
      if (dialog && !dialog.open) dialog.showModal();
    }).catch((error) => {
      pairingStatus = error instanceof PairingRelayError ? error.message : "The pairing service is unavailable.";
      render();
      const dialog = document.querySelector<HTMLDialogElement>("#pair-dialog");
      if (dialog && !dialog.open) dialog.showModal();
    });
  };
  if (pairForm) pairForm.onsubmit = (event) => { event.preventDefault(); void pairMachine(pairForm).then((paired) => { if (paired) document.querySelector<HTMLDialogElement>("#pair-dialog")?.close(); }).catch((error) => toast(error instanceof Error ? error.message : "Pairing failed")); };
  const remove = document.querySelector<HTMLButtonElement>("#remove-machine");
  if (remove && selected) remove.onclick = async () => {
    connections.get(selected.id)?.stop();
    connections.delete(selected.id); machines.delete(selected.id); sessions.delete(selected.id);
    await catalog.remove(selected.id);
    selectedMachineId = machines.keys().next().value; selectedSession = undefined;
    render();
  };
  document.querySelector<HTMLButtonElement>("#lease")!.onclick = async () => {
    if (!selected || !selectedSession) return;
    if (lease?.held_by_me) {
      await connections.get(selected.id)?.releaseLease(selectedSession);
      invalidateMachineLeases(selected.id);
    } else {
      await connections.get(selected.id)?.acquireLease(selectedSession, Boolean(lease?.controller_label && selected.scopes.includes("hub-admin")));
    }
    await loadLease(selected.id, selectedSession);
  };
  document.querySelector<HTMLButtonElement>("#interrupt")!.onclick = () => {
    if (!selected || !selectedSession) return;
    const pane = selectedPanes.get(sessionKey(selected.id, selectedSession));
    if (pane) sendControl(selected.id, selectedSession, { InterruptPane: { pane_id: pane } });
  };
  document.querySelector<HTMLButtonElement>("#message-send")!.onclick = () => {
    if (!selected || !selectedSession) return;
    const text = document.querySelector<HTMLTextAreaElement>("#message-text")!.value.trim();
    if (!text) return;
    const supervisor = sessions.get(selected.id)?.find((item) => item.name === selectedSession)?.supervisor ?? "supervisor";
    sendControl(selected.id, selectedSession, { SendMessage: { target: supervisor, text, summary: "Commander message", urgent: false, attribution: { device_id: null, credential_id: null, device_label: null, operator_label: null, controller_origin: null, request_id: null } } });
  };
}

function scopeChecks(selectedScopes: readonly Scope[]): string {
  const scopes: Scope[] = ["machine-read", "session-read", "pane-read", "pane-input", "message-send", "pane-interrupt"];
  return scopes.map((scope) => `<label class="scope"><input type="checkbox" name="scope" value="${scope}" ${selectedScopes.includes(scope) ? "checked" : ""}>${scope.replaceAll("-", ":")}</label>`).join("");
}

function escapeHtml(value: string): string { const span = document.createElement("span"); span.textContent = value; return span.innerHTML; }
function escapeAttr(value: string): string { return escapeHtml(value).replaceAll('"', "&quot;"); }

void boot();
