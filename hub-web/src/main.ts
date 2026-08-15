import "./styles.css";
import { attentionCounts, attentionUrl, createAttentionItem, machineEventAttention, type AttentionAction, type AttentionContent } from "./attention";
import { renderAttentionCounts, renderAttentionPanel } from "./attention-view";
import { HubConnectionSupervisor, type ConnectionState, type HubMachineInfo } from "./connection";
import { elapsedSeconds, type AttachSnapshot } from "./connection-state";
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
import { loadPaneLayout, movePane, normalizePaneLayout, orderedPaneIds, promotePane, savePaneLayout, type PaneLayout, type PaneLayoutStorage } from "./pane-layout";
import type { AttentionItem, HubSession, LeaseState, PaneInfo, Scope, SessionState, StoredMachine } from "./types";

const pendingPairingStore = pendingPairingStoreFor(window);
const relayOrigin = pairingRelayOrigin(document.querySelector<HTMLMetaElement>('meta[name="cas-pairing-relay-origin"]')?.content ?? null);
let pendingPairing: PendingPairing | null = consumePairingFragment(window.location, window.history, pendingPairingStore);
pendingPairing ??= pendingPairingStore.load();
const pairingOperations = new PairingOperationCoordinator();
const app = document.querySelector<HTMLDivElement>("#app")!;
const rootStyles = getComputedStyle(document.documentElement);
document.querySelector<HTMLMetaElement>('meta[name="theme-color"]')?.setAttribute(
  "content",
  rootStyles.getPropertyValue("--bg-root").trim(),
);
const machines = new Map<string, StoredMachine>();
const sessions = new Map<string, HubSession[]>();
const connections = new Map<string, HubConnectionSupervisor>();
const connectionStates = new Map<string, ConnectionState>();
const attachStates = new Map<string, AttachSnapshot>();
const connectionLatency = new Map<string, number>();
const machineInfo = new Map<string, HubMachineInfo | undefined>();
const statuses = new Map<string, Record<string, unknown>>();
const leases = new Map<string, LeaseState>();
const surfaces = new Map<string, TerminalSurface>();
const sessionStates = new Map<string, SessionState>();
const paneBuffers = new Map<string, number[]>();
const paneLastActivity = new Map<string, number>();
const authoritativeSessions = new Set<string>();
const paneKeyframesReady = new Set<string>();
const selectedPanes = new Map<string, string>();
const collapsedWorkerPanes = new Set<string>();
const leaseHeartbeats = new Map<string, number>();
const leaseExpiryTimers = new Map<string, number>();
// A live Claude/Ink proof after cas-9a29 decides whether one-row PTYs are safe.
// Until then collapsed phone rows preserve their last real terminal geometry.
const mobileCollapsedPaneGeometry = "freeze";
let attention: AttentionItem[] = [];
const newCriticalAttentionIds = new Set<string>();
let selectedMachineId: string | undefined;
let selectedSession: string | undefined;
let pairingStatus = pendingPairing?.kind === "relay-request" ? "Waiting for a machine to claim the code…" : "";
let pairingPollTimer: number | undefined;
let pairingCountdownTimer: number | undefined;
let pairingCreateInFlight = false;
let pairingExchangeInFlight = false;
let pairingDraft = createPairingDraft(location.origin);
let machineDrawerOpen = false;
let attentionPanelCollapsed = window.matchMedia("(max-width: 850px)").matches;
let activeContextTab: "attention" | "status" = "attention";

function sessionKey(machineId: string, session: string): string { return `${machineId}:${session}`; }
function paneKey(machineId: string, session: string, pane: string): string { return `${machineId}:${session}:${pane}`; }
function activeConnection(): HubConnectionSupervisor | undefined { return selectedMachineId ? connections.get(selectedMachineId) : undefined; }

function lastActivityLabel(timestamp: number | undefined): string {
  if (!timestamp) return "waiting";
  const seconds = Math.max(0, Math.round((Date.now() - timestamp) / 1000));
  if (seconds < 5) return "active now";
  if (seconds < 60) return `${seconds}s ago`;
  return `${Math.floor(seconds / 60)}m ago`;
}

function paneLayoutStorage(): PaneLayoutStorage | undefined {
  try { return window.localStorage; } catch { return undefined; }
}

function layoutForPanes(key: string, panes: readonly PaneInfo[], fallbackPrimaryPaneId: string | undefined): PaneLayout | undefined {
  const activePaneIds = panes.filter((pane) => pane.kind !== "Director").map((pane) => pane.id);
  const storage = paneLayoutStorage();
  return storage
    ? loadPaneLayout(storage, key, activePaneIds, fallbackPrimaryPaneId)
    : normalizePaneLayout(activePaneIds, undefined, fallbackPrimaryPaneId);
}

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
    onState: (state) => {
      connectionStates.set(machine.id, state);
      if (state.phase === "failed" || state.phase === "backoff") invalidateMachineLeases(machine.id);
      if (state.authFailure) void addAttention(machine, undefined, "auth_loss", state.reason ?? "Authentication blocked");
      if (state.phase === "backoff") void addAttention(machine, undefined, "hub_disconnected", state.reason ?? "Hub disconnected");
      render();
    },
    onAttachState: (session, state) => {
      attachStates.set(sessionKey(machine.id, session), state);
      if (selectedMachineId === machine.id && selectedSession === session) render();
    },
    onLatency: (latencyMs) => {
      connectionLatency.set(machine.id, latencyMs);
      const output = document.querySelector<HTMLElement>(`[data-machine-latency="${CSS.escape(machine.id)}"]`);
      if (output) output.textContent = `${latencyMs}ms`;
    },
    onAuthFailure: (kind, detail) => {
      pairingStatus = kind === "expired"
        ? "Credential refresh failed. Re-pair this machine to continue."
        : `${detail}. Re-pair in Commander; no browser reset is required.`;
      render();
    },
    onCredentialRefreshed: async (refreshed) => { machines.set(refreshed.id, refreshed); await catalog.put(refreshed); },
    onMachineInfo: (info) => { machineInfo.set(machine.id, info); render(); },
    onSessions: (items) => { sessions.set(machine.id, items); render(); },
    onMachineEvent: (event) => {
      const kind = String(event.kind ?? "hub_event");
      if (["daemon_disconnected", "pane_exited", "session_removed"].includes(kind)) {
        const content = machineEventAttention(kind, event.diagnostic);
        void addAttention(machine, event.session as string | undefined, kind, { ...content, payload: event });
      }
      if (selectedMachineId === machine.id && selectedSession) void loadStatus(machine.id, selectedSession);
      if (selectedMachineId === machine.id && selectedSession) void loadLease(machine.id, selectedSession);
    },
    onSessionState: (session, state, scrollback, authoritativeKeyframes) => {
      void renderSessionState(machine.id, session, state, scrollback, authoritativeKeyframes);
    },
    onPaneKeyframe: (session, pane, data) => {
      const key = paneKey(machine.id, session, pane);
      paneKeyframesReady.add(key);
      paneBuffers.set(key, [...data]);
      surfaces.get(key)?.write(data);
    },
    onOutput: (session, pane, data) => {
      const key = paneKey(machine.id, session, pane);
      paneLastActivity.set(key, Date.now());
      if (authoritativeSessions.has(sessionKey(machine.id, session)) && !paneKeyframesReady.has(key)) return;
      const buffered = [...(paneBuffers.get(key) ?? []), ...data];
      paneBuffers.set(key, buffered.slice(-2_000_000));
      surfaces.get(key)?.write(data);
      const activity = document.querySelector<HTMLElement>(`[data-pane-id="${CSS.escape(pane)}"] .pane-last-activity`);
      if (activity && selectedMachineId === machine.id && selectedSession === session) activity.textContent = "active now";
    },
    onSocketError: (session, detail) => {
      renderTerminalFailure(machine.id, session, detail);
      void addAttention(machine, session, "session_transport", { headline: "Terminal transport problem", detail, severity: "critical", action: "view_pane", payload: detail });
    },
  });
}

function ensureConnection(machine: StoredMachine): HubConnectionSupervisor {
  return ensureMachineConnection(machine, connections, createConnection);
}

async function addAttention(machine: StoredMachine, session: string | undefined, kind: string, content: string | AttentionContent): Promise<void> {
  const createdAt = new Date().toISOString();
  const item = createAttentionItem({
    id: `${machine.id}:${session ?? "machine"}:${kind}:${createdAt}:${crypto.randomUUID()}`,
    machineId: machine.id,
    machineLabel: machine.label,
    session,
    kind,
    createdAt,
  }, content);
  if (item.severity === "critical") newCriticalAttentionIds.add(item.id);
  attention = [item, ...attention];
  await attentionStore.put(item);
  render();
  newCriticalAttentionIds.delete(item.id);
}

async function acknowledgeAttentionGroup(items: AttentionItem[]): Promise<void> {
  const acknowledgedAt = new Date().toISOString();
  const pending = items.filter((item) => !item.acknowledgedAt);
  for (const item of pending) item.acknowledgedAt = acknowledgedAt;
  await Promise.all(pending.map((item) => attentionStore.put(item)));
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
      fetcher: window.fetch.bind(window),
      createKey: createDeviceKey,
      installationGeneration: operation.generation,
      stagePersisted: (candidate, identity) => catalog.stage(candidate, identity, operation.signal),
      activatePersisted: (identity, signal) => catalog.activate(identity, signal),
      rollbackPersisted: (identity) => catalog.rollback(identity),
      acknowledge: relayOrigin ? (relay, signal) => acknowledgePairing(window.fetch.bind(window), relayOrigin, relay, signal) : undefined,
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
      createPairingRequest(window.fetch.bind(window), relayOrigin, location.origin, DEFAULT_PAIRING_SCOPES, email || undefined, operation.signal),
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
    const result = await pollPairingRequest(window.fetch.bind(window), relayOrigin, request, operation.signal);
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
  renderTerminalConnecting(machineId, session);
  await Promise.all([loadStatus(machineId, session), loadLease(machineId, session)]);
  await connections.get(machineId)?.attach(session);
}

function renderTerminalConnecting(machineId: string, session: string): void {
  if (selectedMachineId !== machineId || selectedSession !== session) return;
  const grid = document.querySelector<HTMLElement>("#pane-grid");
  if (grid?.dataset.sessionKey !== sessionKey(machineId, session)) return;
  const placeholder = grid.querySelector<HTMLElement>(".empty");
  if (placeholder) {
    placeholder.classList.remove("terminal-state");
    placeholder.textContent = "Attaching terminal (3s deadline). Existing pane output stays visible while Commander retries.";
  }
}

function renderTerminalFailure(machineId: string, session: string, detail: string): void {
  if (selectedMachineId !== machineId || selectedSession !== session) return;
  const grid = document.querySelector<HTMLElement>("#pane-grid");
  if (grid?.dataset.sessionKey !== sessionKey(machineId, session)) return;
  const placeholder = grid.querySelector<HTMLElement>(".empty");
  if (!placeholder) return;
  const message = document.createElement("p");
  message.textContent = `Terminal unavailable: ${detail}`;
  const retry = document.createElement("button");
  retry.className = "primary retry-terminal";
  retry.textContent = "Try again";
  retry.onclick = () => {
    renderTerminalConnecting(machineId, session);
    void connections.get(machineId)?.attach(session);
  };
  placeholder.classList.add("terminal-state");
  placeholder.replaceChildren(message, retry);
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
        const awaitingMerge = ["awaiting_merge", "awaitingmerge"].includes(String(task.status).toLowerCase());
        const alreadyQueued = attention.some((item) => !item.acknowledgedAt && item.ticketId === String(task.id) && item.kind === String(task.status));
        if (alreadyQueued) continue;
        await addAttention(machine, session, String(task.status), {
          headline: String(task.title),
          severity: awaitingMerge ? "warning" : "info",
          action: awaitingMerge ? "open_pr" : "none",
          ticketId: String(task.id),
          payload: task,
        });
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
      const previousLease = leases.get(key);
      const becameGeometryOwner =
        (state.held_by_me && !previousLease?.held_by_me) ||
        (!state.controller_label && Boolean(previousLease?.controller_label));
      leases.set(key, state);
      if (becameGeometryOwner) resizeViewablePanes(machineId, session);
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

function resizeViewablePanes(machineId: string, session: string): void {
  if (!canResizePanes(machineId, session)) return;
  const state = sessionStates.get(sessionKey(machineId, session));
  if (!state) return;
  for (const pane of state.panes.filter((candidate) => candidate.kind !== "Director")) {
    const surface = surfaces.get(paneKey(machineId, session, pane.id));
    if (surface) sendControl(machineId, session, { ResizePane: { pane_id: pane.id, cols: surface.cols, rows: surface.rows } });
  }
}

async function renderSessionState(machineId: string, session: string, state: SessionState, scrollback?: Record<string, number[][]>, authoritativeKeyframes?: boolean): Promise<void> {
  const selectedKey = sessionKey(machineId, session);
  sessionStates.set(selectedKey, state);
  if (authoritativeKeyframes === true) {
    authoritativeSessions.add(selectedKey);
    for (const pane of state.panes) {
      const key = paneKey(machineId, session, pane.id);
      paneBuffers.delete(key);
      paneKeyframesReady.delete(key);
    }
  } else if (authoritativeKeyframes === false) {
    authoritativeSessions.delete(selectedKey);
  }
  if (scrollback) {
    for (const [pane, chunks] of Object.entries(scrollback)) {
      paneBuffers.set(paneKey(machineId, session, pane), chunks.flat().slice(-2_000_000));
    }
  }
  if (selectedMachineId !== machineId || selectedSession !== session) return;
  const grid = document.querySelector<HTMLElement>("#pane-grid");
  if (!grid) return;
  grid.querySelector(".empty")?.remove();
  const visiblePanes = state.panes.filter((pane) => pane.kind !== "Director");
  const active = new Set(visiblePanes.map((pane) => pane.id));
  const selectedPane = selectedPanes.get(selectedKey);
  if (!selectedPane || !active.has(selectedPane)) {
    const fallback = visiblePanes.find((pane) => pane.focused) ?? visiblePanes[0];
    if (fallback) selectedPanes.set(selectedKey, fallback.id);
  }
  const defaultPrimaryPaneId = visiblePanes.find((pane) => pane.kind === "Supervisor")?.id
    ?? selectedPanes.get(selectedKey)
    ?? visiblePanes[0]?.id;
  const layout = layoutForPanes(selectedKey, visiblePanes, defaultPrimaryPaneId);
  if (!layout) return;
  grid.classList.add("pane-layout");
  grid.classList.toggle("single-pane", visiblePanes.length === 1);
  grid.dataset.secondaryPaneGeometry = mobileCollapsedPaneGeometry;
  let primarySlot = grid.querySelector<HTMLElement>(".primary-pane-slot");
  let secondaryStrip = grid.querySelector<HTMLElement>(".secondary-pane-strip");
  if (!primarySlot || !secondaryStrip) {
    primarySlot = document.createElement("div"); primarySlot.className = "primary-pane-slot";
    secondaryStrip = document.createElement("div"); secondaryStrip.className = "secondary-pane-strip";
    grid.replaceChildren(primarySlot, secondaryStrip);
  }
  for (const [key, surface] of surfaces) {
    if (key.startsWith(`${machineId}:${session}:`) && !active.has(key.split(":").at(-1)!)) { surface.dispose(); surfaces.delete(key); }
  }
  const panesById = new Map(visiblePanes.map((pane) => [pane.id, pane]));
  for (const paneId of orderedPaneIds(layout)) {
    const pane = panesById.get(paneId);
    if (!pane) continue;
    const key = paneKey(machineId, session, pane.id);
    let card = grid.querySelector<HTMLElement>(`[data-pane-id="${CSS.escape(pane.id)}"]`);
    let mount = card?.querySelector<HTMLElement>(".terminal-mount");
    if (!card || !mount) {
      card = document.createElement("section");
      card.className = "pane";
      card.dataset.paneId = pane.id;
      card.dataset.paneRole = pane.kind.toLowerCase();
      if (selectedPanes.get(selectedKey) === pane.id) card.classList.add("selected");
      card.onclick = () => {
        selectedPanes.set(selectedKey, pane.id);
        for (const sibling of grid.querySelectorAll(".pane.selected")) sibling.classList.remove("selected");
        card?.classList.add("selected");
        surfaces.get(key)?.focus();
      };
      const title = document.createElement("header"); title.className = "pane-header";
      const statusDot = document.createElement("span"); statusDot.className = `pane-status-dot ${pane.exited ? "exited" : "live"}`;
      const label = document.createElement("span"); label.className = "pane-title"; label.textContent = pane.title || pane.id;
      const role = document.createElement("span"); role.className = "pane-role"; role.textContent = pane.kind.toLowerCase();
      const activity = document.createElement("span"); activity.className = "pane-last-activity"; activity.textContent = lastActivityLabel(paneLastActivity.get(key));
      const controls = document.createElement("div"); controls.className = "pane-layout-controls";
      const button = (label: string, className: string, action: () => void) => {
        const control = document.createElement("button"); control.type = "button"; control.className = className; control.textContent = label;
        control.setAttribute("aria-label", label);
        control.onclick = (event) => { event.stopPropagation(); action(); };
        return control;
      };
      const updateLayout = (change: (current: PaneLayout) => PaneLayout) => {
        const current = layoutForPanes(selectedKey, visiblePanes, selectedPanes.get(selectedKey));
        if (!current) return;
        const next = change(current);
        const storage = paneLayoutStorage();
        if (storage) savePaneLayout(storage, selectedKey, next);
        void renderSessionState(machineId, session, state);
      };
      controls.append(
        button("Make primary", "make-primary", () => updateLayout((current) => promotePane(current, pane.id))),
        button("Move earlier", "move-earlier", () => updateLayout((current) => movePane(current, pane.id, -1))),
        button("Move later", "move-later", () => updateLayout((current) => movePane(current, pane.id, 1))),
      );
      title.append(statusDot, label, role, activity, controls);
      if (pane.kind !== "Supervisor") {
        title.title = "Click to collapse or expand this worker";
        title.tabIndex = 0;
        title.setAttribute("role", "button");
        title.onclick = (event) => {
          event.stopPropagation();
          selectedPanes.set(selectedKey, pane.id);
          for (const sibling of grid.querySelectorAll(".pane.selected")) sibling.classList.remove("selected");
          card?.classList.add("selected");
          if (collapsedWorkerPanes.has(key)) collapsedWorkerPanes.delete(key);
          else collapsedWorkerPanes.add(key);
          card?.classList.toggle("collapsed", collapsedWorkerPanes.has(key));
          if (!collapsedWorkerPanes.has(key)) queueMicrotask(() => surfaces.get(key)?.focus());
        };
        title.onkeydown = (event) => {
          if (event.key !== "Enter" && event.key !== " ") return;
          event.preventDefault();
          title.click();
        };
      }
      mount = document.createElement("div"); mount.className = "terminal-mount";
      card.append(title, mount);
    }
    if (!card || !mount) continue;
    const position = orderedPaneIds(layout).indexOf(pane.id);
    card.classList.toggle("primary", pane.id === layout.primaryPaneId);
    card.classList.toggle("collapsed", pane.kind !== "Supervisor" && collapsedWorkerPanes.has(key));
    card.querySelector<HTMLElement>(".pane-status-dot")!.className = `pane-status-dot ${pane.exited ? "exited" : "live"}`;
    card.querySelector<HTMLElement>(".pane-title")!.textContent = pane.title || pane.id;
    card.querySelector<HTMLElement>(".pane-last-activity")!.textContent = lastActivityLabel(paneLastActivity.get(key));
    const makePrimary = card.querySelector<HTMLButtonElement>(".make-primary");
    const moveEarlier = card.querySelector<HTMLButtonElement>(".move-earlier");
    const moveLater = card.querySelector<HTMLButtonElement>(".move-later");
    if (makePrimary) makePrimary.disabled = pane.id === layout.primaryPaneId;
    if (moveEarlier) moveEarlier.disabled = pane.id === layout.primaryPaneId || position <= 1;
    if (moveLater) moveLater.disabled = pane.id === layout.primaryPaneId || position === layout.paneIds.length - 1;
    (pane.id === layout.primaryPaneId ? primarySlot : secondaryStrip).append(card);
    const collapsedOnPhone = window.matchMedia("(max-width: 850px)").matches
      && pane.id !== layout.primaryPaneId;
    const existingSurface = surfaces.get(key);
    if (existingSurface && (collapsedOnPhone || existingSurface.element !== mount || !existingSurface.element.isConnected)) {
      existingSurface.dispose();
      surfaces.delete(key);
    }
    if (collapsedOnPhone) continue;
    if (!surfaces.has(key)) {
      const surface = await createTerminalSurface(mount, {
        onData: (data) => { if (canControl(machineId, session, "pane-input")) sendControl(machineId, session, { Input: { pane_id: pane.id, data: [...data] } }); },
        onResize: (cols, rows) => { if (canResizePanes(machineId, session)) sendControl(machineId, session, { ResizePane: { pane_id: pane.id, cols, rows } }); },
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
    if (authoritativeSessions.has(selectedKey) && !paneKeyframesReady.has(key)) {
      connections.get(machineId)?.requestPaneKeyframe(session, pane.id);
    }
  }
}

function hubSupports(machineId: string, capability: string): boolean {
  return machineInfo.get(machineId)?.capabilities.includes(capability) === true;
}

function canControl(machineId: string, session: string, scope: Scope): boolean {
  return hubSupports(machineId, "daemon_attach") && machines.get(machineId)?.scopes.includes(scope) === true && leases.get(sessionKey(machineId, session))?.held_by_me === true;
}

function canResizePanes(machineId: string, session: string): boolean {
  if (!hubSupports(machineId, "daemon_attach") || machines.get(machineId)?.scopes.includes("pane-read") !== true) return false;
  const lease = leases.get(sessionKey(machineId, session));
  return !lease?.controller_label || lease.held_by_me;
}

function controlDisabledReason(machine: StoredMachine | undefined, session: string | undefined, lease: LeaseState | undefined): string | undefined {
  if (!machine) return "Choose a paired machine, then a live session, to use its controls.";
  if (!session) return "Choose a live session to use its controls.";
  if (!hubSupports(machine.id, "daemon_attach")) return "This hub does not support Commander control. Upgrade the hub, then reconnect this machine.";
  const missingScopes = ["pane-input", "message-send", "pane-interrupt"] as const;
  if (missingScopes.some((scope) => !machine.scopes.includes(scope))) {
    return `This credential can only observe from ${location.origin}. Pair ${machine.label} again from this Commander origin and approve control access on the machine. Pairings are specific to each Commander origin.`;
  }
  if (lease?.held_by_me) return undefined;
  if (lease?.controller_label) return `${lease.controller_label} currently controls this session. Wait for it to be released or use an administrator credential to take over.`;
  return "Take control to enable terminal input, messages, and interrupts.";
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

function connectionLabel(state: ConnectionState | undefined): string {
  if (!state) return "idle";
  if (state.phase === "live") return state.degraded ? `degraded · ${state.missedHeartbeats} missed` : `live · ${state.latencyMs ?? 0}ms`;
  if (state.phase === "backoff") return `retrying ${state.stage} in ${Math.ceil((state.retryInMs ?? 0) / 1000)}s`;
  if (state.phase === "failed") return state.reason ?? `failed during ${state.stage}`;
  return `${state.phase} · ${elapsedSeconds(state)}s`;
}

function connectionClass(state: ConnectionState | undefined): string { return state?.degraded ? "degraded" : state?.phase ?? "idle"; }

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
  return `<dialog id="pair-dialog"><section class="pair-flow"><h2>Pair this machine</h2><p>Create a ten-minute code, then verify the exact Commander origin and approve the requested read and control scopes on the target machine.</p>${pairingDetails(location.origin, DEFAULT_PAIRING_SCOPES)}<label>Email code (optional)<input id="pair-email" type="email" autocomplete="email" placeholder="operator@example.com" value="${escapeAttr(pairingDraft.email)}"></label>${pairingStatus ? `<p class="pair-status" role="status">${escapeHtml(pairingStatus)}</p>` : ""}<div class="dialog-actions"><button id="pair-close" type="button">${pairingCreateInFlight ? "Cancel" : "Close"}</button>${pendingPairing ? "" : '<p class="pairing-disabled-reason">Pair is disabled until you open a pairing URL generated by <code>cas hub pair</code> on the machine.</p>'}<button type="button" ${pendingPairing ? "" : "disabled"}>Pair</button>${relayAction}</div></section></dialog>`;
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
  const lease = selected && selectedSession ? leases.get(sessionKey(selected.id, selectedSession)) : undefined;
  const status = selected && selectedSession ? statuses.get(sessionKey(selected.id, selectedSession)) : undefined;
  const compatibility = selected ? compatibilityWarning(selected.id) : undefined;
  const machineConnectionSnapshot = selected ? connectionStates.get(selected.id) : undefined;
  const terminalAttachSnapshot = selected && selectedSession ? attachStates.get(sessionKey(selected.id, selectedSession)) : undefined;
  const connectionSnapshot = terminalAttachSnapshot ?? machineConnectionSnapshot;
  const needsRepair = connectionSnapshot?.stage === "auth" && connectionSnapshot.phase === "failed";
  const controlReason = controlDisabledReason(selected, selectedSession, lease);
  const terminalSessionKey = selected && selectedSession ? sessionKey(selected.id, selectedSession) : undefined;
  const connectionState = connectionClass(connectionSnapshot);
  const connectionText = selected ? connectionLabel(connectionSnapshot) : "idle";
  const latency = machineConnectionSnapshot?.latencyMs ?? (selected ? connectionLatency.get(selected.id) : undefined);
  const counts = attentionCounts(attention);
  const mode = lease?.held_by_me ? "CONTROL" : "OBSERVER";
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
    <div class="shell${attentionPanelCollapsed ? " attention-collapsed" : ""}">
      <aside class="machine-navigation${machineDrawerOpen ? " drawer-open" : ""}" aria-label="Machines and sessions">
        <div class="machine-rail">
          <button id="machine-drawer-toggle" class="rail-control commander-mark" type="button" aria-label="Open machines and sessions" aria-expanded="${machineDrawerOpen}">C</button>
          <nav id="machine-rail-list" aria-label="Machines"></nav>
          <button id="pair-toggle" class="rail-control pair-machine" type="button" aria-label="Pair a machine" title="Pair a machine">+</button>
        </div>
        <div class="machine-drawer">
          <header class="drawer-header"><strong>Machines</strong><button id="machine-drawer-close" type="button" aria-label="Close machines and sessions">×</button></header>
          ${compatibility ? `<div class="compatibility-warning" role="alert">${escapeHtml(compatibility)}</div>` : ""}
          <nav id="machine-tree" aria-label="Machine sessions"></nav>
          ${selected ? '<button id="remove-machine" class="remove-machine">Remove selected machine</button>' : ""}
        </div>
      </aside>
      <main>
        <header class="session-header">
          <h1 class="${selectedSession ? "toolbar-session-title" : ""}">${escapeHtml(selectedSession ?? "Fleet overview")}</h1>
          <span class="machine-chip">${escapeHtml(selected?.label ?? "No machine")}</span>
          <span class="mode-badge ${mode.toLowerCase()}">${mode}</span>
          <span class="connection-summary ${connectionState}" title="${escapeAttr(compatibility ?? connectionText)}"><span class="connection-dot"></span><span data-machine-latency="${escapeAttr(selected?.id ?? "")}">${latency === undefined ? escapeHtml(connectionText) : `${latency}ms`}</span></span>
          <div class="actions"><button id="lease" title="${escapeAttr(controlReason ?? (lease?.held_by_me ? "Release control" : "Take control"))}" ${!selected || !selectedSession || !hubSupports(selected.id, "daemon_attach") || (!lease?.held_by_me && !selected.scopes.includes("pane-input") && !selected.scopes.includes("hub-admin")) ? "disabled" : ""}>${lease?.held_by_me ? "Release control" : lease?.controller_label && selected?.scopes.includes("hub-admin") ? "Force takeover" : "Take control"}</button><button id="interrupt" class="danger" title="${escapeAttr(controlReason ?? "Interrupt selected pane")}" ${!selected || !selectedSession || !canControl(selected.id, selectedSession, "pane-interrupt") ? "disabled" : ""}>Interrupt</button></div>
        </header>
        ${connectionSnapshot?.degraded ? '<section class="compatibility-warning" role="alert">Connection degraded: two heartbeats missed. Reconnecting after four.</section>' : ""}
        ${connectionSnapshot && connectionSnapshot.phase !== "live" && selected ? `<section class="connection-detail" role="status"><span>${escapeHtml(connectionText)}</span><button id="retry-connection" type="button">Retry now</button><button id="diagnose-connection" type="button">Diagnose</button>${needsRepair ? '<button id="repair-machine" class="primary" type="button">Re-pair</button>' : ""}<pre id="diagnostic-output" hidden></pre></section>` : ""}
        <section id="pane-grid" class="pane-grid"${terminalSessionKey ? ` data-session-key="${escapeAttr(terminalSessionKey)}"` : ""}><div class="empty">${selectedSession ? "Connecting to terminal…" : "Choose a live session to open its panes."}</div></section>
      </main>
      <aside class="context-panel${attentionPanelCollapsed ? " collapsed" : ""}" aria-label="Attention, workers, and tasks">
        <div class="attention-rail">
          <button id="attention-panel-toggle" class="rail-control" type="button" aria-label="${attentionPanelCollapsed ? "Expand" : "Collapse"} attention panel" aria-expanded="${!attentionPanelCollapsed}">${attentionPanelCollapsed ? "‹" : "›"}</button>
          <button id="attention-rail-counts" class="attention-rail-counts" type="button" data-open-context="attention" aria-label="Open attention"></button>
        </div>
        <div class="context-body">
          <div class="context-tabs" role="tablist" aria-label="Operations panel">
            <button type="button" role="tab" data-context-tab="attention" aria-selected="${activeContextTab === "attention"}">Attention</button>
            <button type="button" role="tab" data-context-tab="status" aria-selected="${activeContextTab === "status"}">Workers &amp; Tasks</button>
          </div>
          <section id="attention-panel" class="context-tab" data-context-content="attention" ${activeContextTab === "attention" ? "" : "hidden"}></section>
          <section class="context-tab status-context" data-context-content="status" ${activeContextTab === "status" ? "" : "hidden"}><div id="status-view"></div><div class="message"><h2>Message supervisor</h2><textarea id="message-text" placeholder="Send an attributed semantic message"></textarea>${controlReason ? `<p class="control-disabled-reason" role="note">${escapeHtml(controlReason)}</p>` : ""}<button id="message-send" ${!selected || !selectedSession || !canControl(selected.id, selectedSession, "message-send") ? "disabled" : ""}>Send message</button></div></section>
        </div>
      </aside>
    </div>${pairDialogMarkup()}<div id="toast" role="status"></div>`;
  if (preservedGrid) document.querySelector<HTMLElement>("#pane-grid")!.replaceWith(preservedGrid);
  const machineRail = document.querySelector("#machine-rail-list")!;
  const machineTree = document.querySelector("#machine-tree")!;
  for (const machine of machines.values()) {
    machineRail.append(machineRailButton(machine));
    machineTree.append(machineTreeGroup(machine));
  }
  document.querySelector("#attention-rail-counts")?.append(renderAttentionCounts(counts, true));
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

function machineInitials(label: string): string {
  const words = label.trim().split(/\s+/).filter(Boolean);
  return (words.length > 1 ? `${words[0][0]}${words[1][0]}` : words[0]?.slice(0, 2) ?? "?").toUpperCase();
}

function selectMachine(machine: StoredMachine): void {
  selectedMachineId = machine.id;
  selectedSession = undefined;
  machineDrawerOpen = true;
  render();
}

function machineRailButton(machine: StoredMachine): HTMLButtonElement {
  const snapshot = connectionStates.get(machine.id);
  const state = connectionClass(snapshot);
  const button = document.createElement("button");
  button.className = `machine-icon ${machine.id === selectedMachineId ? "active" : ""}`;
  button.type = "button";
  button.innerHTML = `<span>${escapeHtml(machineInitials(machine.label))}</span><i class="machine-state ${state}"></i>`;
  button.title = `${machine.label} · ${connectionLabel(snapshot)}`;
  button.setAttribute("aria-label", `${machine.label}, ${connectionLabel(snapshot)}`);
  button.onclick = () => selectMachine(machine);
  return button;
}

function machineTreeGroup(machine: StoredMachine): HTMLElement {
  const group = document.createElement("section");
  group.className = `machine-group ${machine.id === selectedMachineId ? "active" : ""}`;
  const machineRow = document.createElement("button");
  machineRow.className = "machine-row";
  machineRow.type = "button";
  const snapshot = connectionStates.get(machine.id);
  const state = connectionClass(snapshot);
  machineRow.innerHTML = `<span class="machine-state ${state}"></span><strong>${escapeHtml(machine.label)}</strong><small>${escapeHtml(connectionLabel(snapshot))}</small>`;
  machineRow.onclick = () => selectMachine(machine);
  group.append(machineRow);
  if (machine.id === selectedMachineId) {
    const sessionList = document.createElement("div");
    sessionList.className = "session-tree";
    for (const session of sessions.get(machine.id) ?? []) sessionList.append(sessionButton(machine.id, session));
    if (!sessionList.childElementCount) {
      const empty = document.createElement("p"); empty.className = "drawer-empty"; empty.textContent = "No live sessions."; sessionList.append(empty);
    }
    group.append(sessionList);
  }
  return group;
}

function sessionButton(machineId: string, session: HubSession): HTMLButtonElement {
  const button = document.createElement("button"); button.className = `nav-item ${session.name === selectedSession ? "active" : ""}`;
  button.innerHTML = `<span class="session-name">${escapeHtml(session.name)}</span><small class="session-meta">${escapeHtml(session.supervisor)} · ${session.workers.length} workers · ${session.liveness}</small>`;
  button.onclick = () => { machineDrawerOpen = false; void openSession(machineId, session.name); };
  return button;
}

function renderAttention(): void {
  const container = document.querySelector<HTMLElement>("#attention-panel");
  if (!container) return;
  renderAttentionPanel(container, attention, {
    dismiss: acknowledgeAttentionGroup,
    act: performAttentionAction,
    copy: async (payload) => {
      await navigator.clipboard.writeText(payload);
      toast("Event payload copied");
    },
  }, { animateIds: newCriticalAttentionIds });
}

async function performAttentionAction(item: AttentionItem, action: AttentionAction): Promise<void> {
  if (action === "repair") {
    document.querySelector<HTMLDialogElement>("#pair-dialog")?.showModal();
    return;
  }
  if (action === "open_pr") {
    const url = attentionUrl(item);
    if (url) {
      window.open(url, "_blank", "noopener,noreferrer");
      return;
    }
  }
  selectedMachineId = item.machineId;
  if (item.session) await openSession(item.machineId, item.session);
  else render();
}

function renderStatus(status?: Record<string, unknown>): void {
  const container = document.querySelector("#status-view")!;
  if (!status) { container.textContent = "Open a session for push-refreshed status."; return; }
  const agents = (status.agents as any[]) ?? [];
  const tasks = [...((status.tasks_in_progress as any[]) ?? []), ...((status.tasks_ready as any[]) ?? [])];
  const identifier = (value: unknown): HTMLSpanElement => {
    const span = document.createElement("span");
    span.className = "status-identifier";
    span.textContent = String(value);
    return span;
  };
  for (const agent of agents) {
    const row = document.createElement("article"); row.className = "status-row";
    row.append(identifier(agent.name), " · ", identifier(agent.status));
    if (agent.current_task) row.append(" · ", identifier(agent.current_task));
    if (agent.latest_activity?.summary) row.append(` — ${agent.latest_activity.summary}`);
    container.append(row);
  }
  for (const task of tasks) {
    const row = document.createElement("article"); row.className = "status-row";
    row.append(identifier(task.id), " · ", identifier(task.status), ` · ${task.title}`);
    container.append(row);
  }
}

function bindEvents(selected: StoredMachine | undefined, lease: LeaseState | undefined): void {
  document.querySelector<HTMLButtonElement>("#pair-toggle")!.onclick = () => (document.querySelector<HTMLDialogElement>("#pair-dialog")!).showModal();
  document.querySelector<HTMLButtonElement>("#machine-drawer-toggle")!.onclick = () => { machineDrawerOpen = !machineDrawerOpen; render(); };
  document.querySelector<HTMLButtonElement>("#machine-drawer-close")!.onclick = () => { machineDrawerOpen = false; render(); };
  document.querySelector<HTMLButtonElement>("#attention-panel-toggle")!.onclick = () => { attentionPanelCollapsed = !attentionPanelCollapsed; render(); };
  for (const button of document.querySelectorAll<HTMLButtonElement>("[data-open-context]")) {
    button.onclick = () => { activeContextTab = "attention"; attentionPanelCollapsed = false; render(); };
  }
  for (const tab of document.querySelectorAll<HTMLButtonElement>("[data-context-tab]")) {
    tab.onclick = () => { activeContextTab = tab.dataset.contextTab === "status" ? "status" : "attention"; render(); };
  }
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
  const retry = document.querySelector<HTMLButtonElement>("#retry-connection");
  if (retry && selected) retry.onclick = () => connections.get(selected.id)?.retry();
  const diagnose = document.querySelector<HTMLButtonElement>("#diagnose-connection");
  if (diagnose && selected) diagnose.onclick = () => void connections.get(selected.id)?.diagnose().then((result) => {
    const output = document.querySelector<HTMLElement>("#diagnostic-output");
    if (output) { output.hidden = false; output.textContent = JSON.stringify(result, null, 2); }
  }).catch((error) => toast(error instanceof Error ? error.message : "Diagnosis failed"));
  const repair = document.querySelector<HTMLButtonElement>("#repair-machine");
  if (repair) repair.onclick = () => { pairingStatus = `Re-pair ${selected?.label ?? "this machine"} with a short-lived code.`; document.querySelector<HTMLDialogElement>("#pair-dialog")?.showModal(); };
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
      await connections.get(selected.id)?.requestControl(selectedSession, Boolean(lease?.controller_label && selected.scopes.includes("hub-admin")));
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
