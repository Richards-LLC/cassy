import "./styles.css";
import { applyAttentionEnrichment, attentionCounts, attentionSummary, attentionUrl, createAttentionItem, dismissableInfoItems, machineEventAttention, mergeAttentionItem, type AttentionAction, type AttentionContent, type AttentionEnrichment } from "./attention";
import { cycleAttentionGroup, renderAttentionCounts, renderAttentionPanel, renderAttentionSummary } from "./attention-view";
import { HubConnectionSupervisor, type ConnectionState, type HubMachineInfo } from "./connection";
import { attachElapsedSeconds, elapsedSeconds, type AttachSnapshot } from "./connection-state";
import { connectingView, disconnectedView, shouldRetainDisconnectedFrame } from "./connection-state-view";
import { ensureMachineConnection, replaceMachineConnection } from "./connection-lifecycle";
import { createDeviceKey } from "./dpop";
import { readPairingFragment, watchPairingFragment } from "./fragment";
import { createPairingDraft, updatePairingDraft } from "./pairing-draft";
import { bindPairingDialogCancel } from "./pairing-dialog";
import { EXPIRED_PAIRING_INVITATION_MESSAGE, INVALID_PAIRING_LINK_MESSAGE, cancellationOutcome, pairingCleanupFailureUpdate, pairingStorageClearFailureMessage } from "./pairing-cleanup";
import { exchangePendingPairing, PairingCleanupError, PairingExchangeError, PairingStorageError } from "./pairing-exchange";
import { PairingOperationCoordinator, commitPairingResult } from "./pairing-operation";
import { LATE_ROLLBACK_FAILURE_MESSAGE, PairingCancellationTracker, cleanupRetryOutcome } from "./pairing-cancellation";
import { PAIRING_SCOPES, pairCommand, preselectedScopes, scopeChoices, scopeLabel, ungrantedScopes } from "./pairing-scopes";
import { pendingPairingStoreFor, type PendingPairing, type PendingRelayRequest } from "./pending-pairing";
import { DEFAULT_PAIRING_SCOPES, PairingRelayError, acknowledgePairing, createPairingRequest, pairingRelayOrigin, pollPairingRequest } from "./pairing-relay";
import { browserSupport, unsupportedBrowserNotice } from "./browser-support";
import { attentionStore, catalog } from "./storage";
import { createTerminalSurface, type TerminalSurface } from "./terminal";
import { absoluteTimestamp, relativeTimestamp } from "./time";
import { loadPaneLayout, movePane, normalizePaneLayout, orderedPaneIds, promotePane, savePaneLayout, type PaneLayout, type PaneLayoutStorage } from "./pane-layout";
import { detectSpeechInput, SpeechDictationController, type SpeechInputCapability, type SpeechInputState } from "./speech-input";
import { backLabel, clearStoredSelection, forgetMachine, goBackSelection, loadStoredSelection, previousSelection, restorableSession, saveStoredSelection, selectSelection, sessionPickerEntries, sessionPickerMeta, workerCountLabel, type SelectionState, type SelectionStorage, type SessionSelection } from "./session-selection";
import { composerFocusWinner, planSupervisorSend, sendsOnEnter, supervisorMessage, supervisorTarget } from "./supervisor-message";
import { COMPACT_MEDIA_QUERY, PHONE_MEDIA_QUERY } from "./viewport";
import { defaultTranscriptView, loadTranscriptView, saveTranscriptView, type TranscriptViewMode } from "./transcript";
import { TranscriptView } from "./transcript-view";
import { applyLiveRegions, type LiveRegionView } from "./live-regions";
import { DeferredRenderScheduler } from "./deferred-render";
import { FleetBoardRenderer } from "./fleet-board";
import { isEditableElement, renderDecision, shellSignature } from "./render-model";
import type { AttentionItem, HubSession, LeaseState, PaneInfo, Scope, SessionCardSummary, SessionState, StoredMachine } from "./types";

const pendingPairingStore = pendingPairingStoreFor(window);
const relayOrigin = pairingRelayOrigin(document.querySelector<HTMLMetaElement>('meta[name="cas-pairing-relay-origin"]')?.content ?? null);
const arrivedFragment = readPairingFragment(window.location, window.history, pendingPairingStore);
let pendingPairing: PendingPairing | null = arrivedFragment.kind === "fragment" ? arrivedFragment.fragment : null;
// Opening the link is the operator's "yes"; making them hunt for Pair a machine
// afterwards is how a one-time invitation gets left unused on a phone. A broken
// or expired link is the same "yes" with nothing usable behind it, so it opens
// the dialog too — on the sentence that says so, never on the token (F6).
let pairDialogAutoOpen = pendingPairing !== null || arrivedFragment.kind === "invalid";
let pairingArrivalNotice = arrivedFragment.kind === "invalid" ? INVALID_PAIRING_LINK_MESSAGE : "";
if (!pendingPairing) {
  const stored = pendingPairingStore.loadOutcome();
  if (stored.kind === "pending") pendingPairing = stored.value;
  if (stored.kind === "expired" && !pairingArrivalNotice) {
    pairingArrivalNotice = EXPIRED_PAIRING_INVITATION_MESSAGE;
    pairDialogAutoOpen = true;
  }
}
const pairingOperations = new PairingOperationCoordinator();
// Which cancellation, if any, owns the "could not finish cancelling" step.
const pairingCancellations = new PairingCancellationTracker();
const app = document.querySelector<HTMLDivElement>("#app")!;
const rootStyles = getComputedStyle(document.documentElement);
document.querySelector<HTMLMetaElement>('meta[name="theme-color"]')?.setAttribute(
  "content",
  rootStyles.getPropertyValue("--bg-root").trim(),
);
const machines = new Map<string, StoredMachine>();
let machineCatalogLoaded = false;
const sessions = new Map<string, HubSession[]>();
const connections = new Map<string, HubConnectionSupervisor>();
const connectionStates = new Map<string, ConnectionState>();
const lastLiveAt = new Map<string, number>();
const attachStates = new Map<string, AttachSnapshot>();
const machineInfo = new Map<string, HubMachineInfo | undefined>();
const statuses = new Map<string, Record<string, unknown>>();
const leases = new Map<string, LeaseState>();
const surfaces = new Map<string, TerminalSurface>();
const transcripts = new Map<string, TranscriptView>();
// The shell is rebuilt only when its own inputs changed. A hub heartbeat
// carries none of them, so it can no longer replace the composer mid-sentence.
let lastShellSignature: string | undefined;
let lastPairingView: string | undefined;
// setTimeout, not queueMicrotask: the click event a pointerup is about to
// produce is dispatched in the same task, so only a macrotask lands after it.
const deferredRender = new DeferredRenderScheduler({
  render: () => render(),
  afterGesture: (run) => window.setTimeout(run, 0),
});
let lastRailSignature: string | undefined;
const sessionStates = new Map<string, SessionState>();
// Shared data source for session drawers, status rows, pane tooltips, and the
// Cmd+K integration lane. Values are produced once by the daemon.
export const sessionSummaries = new Map<string, SessionCardSummary>();
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
const reclassifiedAttentionIds = new Set<string>();
let selectedMachineId: string | undefined;
let selectedSession: string | undefined;
// One machine runs several sessions, so where the operator is standing is a
// (machine, session) pair with a trail behind it. selectedMachineId and
// selectedSession stay as the render-facing view of selection.current.
let selection: SelectionState = { history: [] };
// The last session from the previous visit, held until this machine's hub
// confirms it still exists.
let restoreTarget: SessionSelection | undefined;
let sessionPickerOpen = false;
let pairingStatus = pendingPairing?.kind === "relay-request" ? "Waiting for a machine to claim the code…" : pairingArrivalNotice;
// Cancellation whose durable cleanup did not complete: the dialog stays on a
// "could not finish cancelling" step with a retry until storage cooperates (F2).
let pairingCleanupFailed = false;
// The generation of the exchange Cancel would invalidate, so a late rollback
// result can be matched to the cancellation that caused it.
let exchangeOperationGeneration: number | undefined;
let pairingPollTimer: number | undefined;
let pairingCountdownTimer: number | undefined;
let connectionViewTicker: number | undefined;
let pairingCreateInFlight = false;
let pairingExchangeInFlight = false;
let pairingDraft = createPairingDraft(location.origin, preselectedScopes(pendingPairing));
let machineDrawerOpen = false;
let attentionPanelCollapsed = window.matchMedia(PHONE_MEDIA_QUERY).matches;
let activeContextTab: "attention" | "status" = "attention";
let commandPaletteOpen = false;
let speechCapability: SpeechInputCapability | undefined;
let speechDetectionStarted = false;
let speechController: SpeechDictationController | undefined;
let speechInputState: SpeechInputState = "idle";
let speechInputDetail = "";
let messageDelivery: { session: string; target: string } | undefined;
// Why a send did not happen has to survive the render that follows it, and has
// to sit beside the composer: a toast is gone before a phone operator has
// finished reading it, and a disabled button says nothing at all.
let messageStatus: { session: string | undefined; text: string; tone: "info" | "error" } | undefined;

// An engine cannot gain an API mid-session, so this is probed once. Saying so
// in one line beats a "Connecting…" spinner that can never finish
// (report cas-b652, defect D3).
const browserNotice = unsupportedBrowserNotice(browserSupport());

// One phone definition shared by the stylesheet, layout state, pane mounting
// and pane tapping — see viewport.ts. Rotation must not put the CSS and this
// logic in different modes, which a width-only breakpoint guaranteed it would.
function phoneLayout(): boolean { return window.matchMedia(PHONE_MEDIA_QUERY).matches; }


// The compact breakpoint from DESIGN.md, which is also where a mount stops
// being able to measure a usable agent-TUI grid.
function compactViewport(): boolean { return window.matchMedia(COMPACT_MEDIA_QUERY).matches; }

/**
 * Columns handed to the PTY on a compact viewport. A 395px mount measures ~46
 * columns, and an 80-column agent TUI redrawn at that width loses its hanging
 * indents before Commander ever sees the bytes; the floor is what the reflowed
 * transcript then reads back. Desktop keeps the mount's own measurement.
 */
const COMPACT_MINIMUM_COLUMNS = 80;

function paneViewMode(selectedKey: string): TranscriptViewMode {
  const storage = paneLayoutStorage();
  const stored = storage ? loadTranscriptView(storage, selectedKey) : undefined;
  return stored ?? defaultTranscriptView(window.innerWidth);
}

function setPaneViewMode(selectedKey: string, view: TranscriptViewMode): void {
  const storage = paneLayoutStorage();
  if (storage) saveTranscriptView(storage, selectedKey, view);
  const state = sessionStates.get(selectedKey);
  const [machineId, session] = [selectedMachineId, selectedSession];
  if (state && machineId && session) void renderSessionState(machineId, session, state);
}

/**
 * Applies the reading view to one mounted pane: the transcript owns the mount
 * while it is active, and the grid keeps rendering underneath it only when it
 * is the thing on screen.
 */
function releaseSurface(key: string, surface: TerminalSurface): void {
  transcripts.get(key)?.dispose();
  transcripts.delete(key);
  surface.dispose();
  surfaces.delete(key);
}

function applyPaneView(key: string, mount: HTMLElement, surface: TerminalSurface, view: TranscriptViewMode): void {
  surface.setMinimumColumns(compactViewport() ? COMPACT_MINIMUM_COLUMNS : 0);
  const active = view === "transcript";
  mount.classList.toggle("transcript-active", active);
  surface.setCanvasPainting(!active);
  let transcript = transcripts.get(key);
  if (active && !transcript) {
    transcript = new TranscriptView(document, surface.transcript);
    transcripts.set(key, transcript);
  }
  if (!active) {
    transcript?.dispose();
    transcripts.delete(key);
    return;
  }
  if (transcript && transcript.element.parentElement !== mount) mount.append(transcript.element);
  transcript?.update();
}

function sessionKey(machineId: string, session: string): string { return `${machineId}:${session}`; }
function paneKey(machineId: string, session: string, pane: string): string { return `${machineId}:${session}:${pane}`; }
function activeConnection(): HubConnectionSupervisor | undefined { return selectedMachineId ? connections.get(selectedMachineId) : undefined; }

function selectionStorage(): SelectionStorage | undefined {
  try { return window.localStorage; } catch { return undefined; }
}

function applySelection(next: SessionSelection | undefined): void {
  selectedMachineId = next?.machineId;
  selectedSession = next?.session;
}

/**
 * Every deliberate move — a machine, a session, an attention jump — goes
 * through here, so the back control and the restored-on-reopen session are
 * always describing the same trail.
 */
function commitSelection(next: SessionSelection): void {
  restoreTarget = undefined;
  selection = selectSelection(selection, next);
  applySelection(next);
  saveStoredSelection(selectionStorage(), next);
}

function goBack(): void {
  const previous = previousSelection(selection);
  if (!previous) return;
  selection = goBackSelection(selection);
  applySelection(previous);
  saveStoredSelection(selectionStorage(), previous);
  if (previous.session) void attachSelectedSession(previous.machineId, previous.session);
  else render();
}

function lastActivityLabel(timestamp: number | undefined): string {
  return relativeTimestamp(timestamp);
}

function updatePaneActivity(element: HTMLElement, timestamp: number | undefined): void {
  element.textContent = lastActivityLabel(timestamp);
  element.title = timestamp === undefined ? "No activity received" : absoluteTimestamp(timestamp);
}

function focusPane(machineId: string, session: string, paneId: string): void {
  const selectedKey = sessionKey(machineId, session);
  const key = paneKey(machineId, session, paneId);
  selectedPanes.set(selectedKey, paneId);
  collapsedWorkerPanes.delete(key);
  const grid = document.querySelector<HTMLElement>("#pane-grid");
  const phone = phoneLayout();
  for (const pane of grid?.querySelectorAll<HTMLElement>(".pane") ?? []) {
    const selected = pane.dataset.paneId === paneId;
    pane.classList.toggle("selected", selected);
    // A phone worker has no mounted terminal until it is promoted, so expanding
    // it here would only open an empty well.
    if (selected && !(phone && !pane.classList.contains("primary"))) pane.classList.remove("collapsed");
  }
  surfaces.get(key)?.focus();
}

function activePaneContext(): { machineId: string; session: string; paneId: string; surface: TerminalSurface } | undefined {
  if (!selectedMachineId || !selectedSession) return undefined;
  const paneId = selectedPanes.get(sessionKey(selectedMachineId, selectedSession));
  const surface = paneId ? surfaces.get(paneKey(selectedMachineId, selectedSession, paneId)) : undefined;
  return paneId && surface ? { machineId: selectedMachineId, session: selectedSession, paneId, surface } : undefined;
}

function openTerminalSearch(): void {
  const active = activePaneContext();
  if (!active) return;
  const pane = document.querySelector<HTMLElement>(`[data-pane-id="${CSS.escape(active.paneId)}"]`);
  if (!pane) return;
  pane.querySelector<HTMLElement>(".terminal-search")?.remove();
  const form = document.createElement("form");
  form.className = "terminal-search";
  form.setAttribute("role", "search");
  const input = document.createElement("input");
  input.type = "search";
  input.placeholder = "Find in terminal";
  input.setAttribute("aria-label", "Find in focused terminal");
  const result = document.createElement("span");
  result.setAttribute("role", "status");
  const close = document.createElement("button");
  close.type = "button";
  close.textContent = "×";
  close.setAttribute("aria-label", "Close terminal search");
  const closeSearch = () => { form.remove(); active.surface.focus(); };
  close.onclick = closeSearch;
  form.onsubmit = (event) => {
    event.preventDefault();
    result.textContent = active.surface.search(input.value) ? "Match selected" : "No match";
  };
  form.onkeydown = (event) => {
    if (event.key !== "Escape") return;
    event.preventDefault();
    closeSearch();
  };
  form.append(input, result, close);
  pane.append(form);
  input.focus();
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
  machineCatalogLoaded = true;
  if (stored.pendingCleanup > 0) {
    pairingStatus = "A canceled credential remains blocked while durable local cleanup is pending.";
  }
  attention = (await attentionStore.list()).toSorted((a, b) => b.createdAt.localeCompare(a.createdAt));
  // Reopening on "No session open" throws away the one thing the operator was
  // looking at. The machine is restored immediately; the session waits for the
  // hub to confirm it is still running.
  const lastSelection = loadStoredSelection(selectionStorage());
  const restoredMachineId = lastSelection && machines.has(lastSelection.machineId) ? lastSelection.machineId : undefined;
  selectedMachineId = restoredMachineId ?? machines.keys().next().value;
  if (selectedMachineId) selection = { current: { machineId: selectedMachineId }, history: [] };
  restoreTarget = restoredMachineId && lastSelection?.session ? lastSelection : undefined;
  render();
  for (const machine of machines.values()) ensureConnection(machine);
  // An invitation that arrived in the URL is the whole reason this page was
  // opened. Rendering the same empty state behind it leaves the operator
  // guessing that they must tap "Pair a machine" again.
  if (pendingPairing) openPairDialog();
  resumePairingPoll();
}

function openPairDialog(): void {
  const dialog = document.querySelector<HTMLDialogElement>("#pair-dialog");
  if (dialog && !dialog.open) dialog.showModal();
}

// Android hands a pairing URL that differs only by #fragment to the tab that is
// already open, so boot-time consumption alone drops the invitation in silence.
watchPairingFragment(window, pendingPairingStore, (fragment) => {
  // A new link is a new flow: whatever an earlier cancellation still owed the
  // dialog no longer applies to it, and the fresh invitation takes the store.
  pairingCancellations.supersede();
  pairingCleanupFailed = false;
  pendingPairing = fragment;
  pairingStatus = "";
  render();
  openPairDialog();
}, () => {
  if (pendingPairing) return;
  pairingStatus = INVALID_PAIRING_LINK_MESSAGE;
  render();
  openPairDialog();
});

function createConnection(machine: StoredMachine): HubConnectionSupervisor {
  return new HubConnectionSupervisor(machine, {
    onState: (state) => {
      connectionStates.set(machine.id, state);
      // Anchor staleness to the last live moment: retry transitions rewrite
      // snapshot.since, which would report a ten-minute outage as "just now".
      if (state.phase === "live") lastLiveAt.set(machine.id, Date.now());
      if (state.phase === "failed" || state.phase === "backoff") invalidateMachineLeases(machine.id);
      // One outage is one problem. A stable fingerprint per machine and kind
      // collapses every retry into a single card with a repeat count instead of
      // burying the feed under a card for each attempt.
      if (state.authFailure) {
        void addAttention(machine, undefined, "auth_loss", {
          headline: state.authFailure === "needs-pairing" ? "Machine needs pairing" : "Authentication blocked",
          detail: state.reason ?? "Authentication blocked",
          severity: "critical",
          action: "repair",
          fingerprint: `${machine.id}:auth_loss`,
        });
      }
      if (state.phase === "backoff") {
        void addAttention(machine, undefined, "hub_disconnected", {
          headline: "Reconnecting to hub",
          detail: state.reason ?? "Hub disconnected",
          severity: "warning",
          action: "retry",
          fingerprint: `${machine.id}:hub_disconnected`,
        });
      }
      render();
    },
    onAttachState: (session, state) => {
      attachStates.set(sessionKey(machine.id, session), state);
      if (selectedMachineId === machine.id && selectedSession === session) render();
    },
    onAuthFailure: (kind, detail) => {
      if (kind === "expired") return;
      pairingStatus = `${detail}. Re-pair in Cassy Commander; no browser reset is required.`;
      render();
    },
    onCredentialRefreshed: async (refreshed) => { machines.set(refreshed.id, refreshed); await catalog.put(refreshed); },
    onMachineInfo: (info) => { machineInfo.set(machine.id, info); render(); },
    onSessions: (items) => { sessions.set(machine.id, items); restoreLastSession(machine.id, items); render(); },
    onMachineEvent: (event) => {
      const kind = String(event.kind ?? "hub_event");
      if (["daemon_disconnected", "daemon_error", "pane_exited", "session_removed"].includes(kind) || event.enrichment !== undefined) {
        void upsertMachineEventAttention(machine, event);
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
    onPaneSize: (session, pane, cols, rows, authority) => {
      applyPaneAuthority(machine.id, session, pane, cols, rows, authority);
    },
    onFlowControlReset: (session) => {
      const prefix = `${sessionKey(machine.id, session)}:`;
      for (const key of paneKeyframesReady) {
        if (key.startsWith(prefix)) paneKeyframesReady.delete(key);
      }
    },
    onOutput: (session, pane, data) => {
      const key = paneKey(machine.id, session, pane);
      paneLastActivity.set(key, Date.now());
      if (authoritativeSessions.has(sessionKey(machine.id, session)) && !paneKeyframesReady.has(key)) return;
      const buffered = [...(paneBuffers.get(key) ?? []), ...data];
      paneBuffers.set(key, buffered.slice(-2_000_000));
      surfaces.get(key)?.write(data);
      const activity = document.querySelector<HTMLElement>(`[data-pane-id="${CSS.escape(pane)}"] .pane-last-activity`);
      if (activity && selectedMachineId === machine.id && selectedSession === session) updatePaneActivity(activity, paneLastActivity.get(key));
    },
    onSessionSummary: (session, summary) => {
      sessionSummaries.set(sessionKey(machine.id, session), summary);
      if (selectedMachineId === machine.id && selectedSession === session) {
        const state = sessionStates.get(sessionKey(machine.id, session));
        if (state) void renderSessionState(machine.id, session, state);
      }
      render();
    },
    onSocketError: (session, detail) => {
      renderTerminalFailure(machine.id, session, detail);
      void addAttention(machine, session, "session_transport", { headline: "Terminal transport problem", detail, severity: "critical", action: "view_pane", payload: detail, fingerprint: `${machine.id}:${session}:session_transport` });
    },
  });
}

function attentionEnrichment(value: unknown): AttentionEnrichment | undefined {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return undefined;
  const candidate = value as Record<string, unknown>;
  if (!["critical", "warning", "info"].includes(String(candidate.severity))) return undefined;
  if (!["repair", "view_pane", "retry", "open_pr", "none"].includes(String(candidate.action))) return undefined;
  if (typeof candidate.summary !== "string" || typeof candidate.fingerprint !== "string") return undefined;
  if (candidate.detail !== undefined && candidate.detail !== null && typeof candidate.detail !== "string") return undefined;
  return candidate as unknown as AttentionEnrichment;
}

async function upsertMachineEventAttention(machine: StoredMachine, event: Record<string, unknown>): Promise<void> {
  const kind = String(event.kind ?? "hub_event");
  const session = typeof event.session === "string" ? event.session : undefined;
  const sequence = typeof event.sequence === "number" || typeof event.sequence === "string"
    ? String(event.sequence)
    : crypto.randomUUID();
  const id = `${machine.id}:event:${sequence}`;
  const existing = attention.find((item) => item.id === id);
  const payload = event.payload ?? event.diagnostic ?? event;
  const pending = event.enrichment_pending === true;
  const provisional = existing ?? createAttentionItem({
    id,
    machineId: machine.id,
    machineLabel: machine.label,
    session,
    kind,
    createdAt: typeof event.at === "string" ? event.at : new Date().toISOString(),
  }, machineEventAttention(kind, payload, pending));
  const enriched = attentionEnrichment(event.enrichment);
  const next = enriched
    ? applyAttentionEnrichment(provisional, enriched, typeof event.enriched_at === "string" ? event.enriched_at : undefined)
    : { ...provisional, enrichmentPending: pending };
  const wasCritical = existing?.severity === "critical";
  if (existing && existing.severity !== next.severity) reclassifiedAttentionIds.add(id);
  if (!wasCritical && next.severity === "critical") newCriticalAttentionIds.add(id);
  attention = existing
    ? attention.map((item) => item.id === id ? next : item)
    : [next, ...attention];
  await attentionStore.put(next);
  render();
  newCriticalAttentionIds.delete(id);
  reclassifiedAttentionIds.delete(id);
}

/**
 * The session list arrives after boot, so restore is claimed here rather than
 * guessed at boot: a session that ended between visits simply never matches,
 * and any selection the operator makes first cancels the restore.
 */
function restoreLastSession(machineId: string, items: readonly HubSession[]): void {
  if (selectedSession !== undefined) return;
  const session = restorableSession(restoreTarget, machineId, items);
  if (!session) return;
  restoreTarget = undefined;
  void openSession(machineId, session);
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
  // One recurring failure is one entry: a retry loop used to write a row per
  // attempt for the same outage (cas-b652 D3).
  const merge = mergeAttentionItem(attention, item);
  if (merge.stored.severity === "critical" && !merge.repeat) newCriticalAttentionIds.add(merge.stored.id);
  attention = merge.items;
  await attentionStore.put(merge.stored);
  render();
  newCriticalAttentionIds.delete(merge.stored.id);
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
  exchangeOperationGeneration = operation.generation;
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
      // The relay form has no scope boxes, so its invitation's own scopes stand.
      requestedScopes: form.querySelector('input[name="scope"]') ? values.getAll("scope") as Scope[] : undefined,
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
      if (!update) {
        // Cancel invalidated this operation and its rollback then rejected. The
        // cancellation that did so still owns the dialog unless something newer
        // started; then the staged row stays quarantined for boot-time recovery
        // and the newer flow is left alone.
        if (pairingCancellations.ownsOperation(operation.generation)) {
          const cleared = pendingPairingStore.clear();
          pairingCleanupFailed = true;
          pairingStatus = cleared.failClosed
            ? LATE_ROLLBACK_FAILURE_MESSAGE
            : `${LATE_ROLLBACK_FAILURE_MESSAGE} Browser storage also refused to record the cancellation.`;
          render(false);
          openPairDialog();
        }
        return false;
      }
      const cleared = pendingPairingStore.clear();
      pendingPairing = update.pendingPairing;
      pairingDraft = update.pairingDraft;
      pairingExchangeInFlight = update.exchangeInFlight;
      pairingStatus = cleared.failClosed
        ? `${update.status}${cleared.persistentRemovalFailed ? " Browser storage removal was denied; the cancelled request is durably blocked." : ""}`
        : `${update.status} Browser storage could not durably block the cancelled request.`;
      pairingCleanupFailed = true;
      render(false);
      openPairDialog();
      throw error;
    }
    if (!pairingOperations.isCurrent(operation)) {
      // Only the cancellation that ended this operation may close the dialog;
      // a replacement flow that started since owns it now.
      if (!pendingPairing && !pairingCleanupFailed && pairingCancellations.ownsOperation(operation.generation)) {
        // The cancellation the operator asked for has now been verified: the
        // dialog that said "verifying" can close, and the page says so.
        pairingStatus = "Pairing cancelled.";
        finishCancelledPairing();
      }
      return false;
    }
    pairingExchangeInFlight = false;
    if (error instanceof PairingStorageError) {
      // The hub consumed the invitation and recorded the device; only this
      // browser's copy failed. Say exactly that and point at a fresh invitation
      // instead of "expired or already used" (F3).
      pairingOperations.invalidate();
      const cleared = pendingPairingStore.clear();
      pendingPairing = null;
      pairingDraft = createPairingDraft(location.origin);
      pairingStatus = pairingStorageClearFailureMessage(error.message, cleared);
      render(false);
      throw error;
    }
    if (error instanceof PairingExchangeError && error.recoverable) {
      // Nothing reached the machine, so the invitation is still good: say what
      // happened and leave Pair usable instead of sending the operator back to
      // a terminal for a fresh link.
      pairingStatus = error.message;
      render(false);
      throw error;
    }
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
    if (exchangeOperationGeneration === operation.generation) exchangeOperationGeneration = undefined;
  }
  if (!pairingOperations.isCurrent(operation)) return false;
  pairingExchangeInFlight = false;
  pairingCancellations.supersede();
  pairingOperations.invalidate();
  pendingPairingStore.clear();
  pendingPairing = null;
  stopPairingTimers();
  pairingDraft = createPairingDraft(location.origin);
  machines.set(machine.id, machine);
  commitSelection({ machineId: machine.id });
  replaceMachineConnection(machine, connections, connectionStates, createConnection);
  render(false);
  return true;
}

async function startRelayPairing(email: string): Promise<boolean> {
  if (!relayOrigin) throw new PairingRelayError("relay_unavailable", "Page-initiated pairing is unavailable in this deployment.");
  if (pairingCreateInFlight) return false;
  const generation = pairingOperations.replace();
  pairingCancellations.supersede();
  pairingCleanupFailed = false;
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
      pairingStatus = "Approved. Checking that this device can reach the machine…";
      render();
      const hubUrl = result.invitation.hubUrl;
      if (hubUrl) {
        try {
          await fetch(new URL("/v1/health", hubUrl), {
            method: "GET",
            mode: "no-cors",
            cache: "no-store",
            credentials: "omit",
            signal: AbortSignal.timeout(3_000),
          });
          if (!pairingOperations.isCurrent(operation) || pendingPairing?.kind !== "invitation") return;
          pairingStatus = "Machine authorized. Confirm the exact details and finish pairing.";
        } catch {
          if (!pairingOperations.isCurrent(operation) || pendingPairing?.kind !== "invitation") return;
          const machine = result.invitation.machineLabel ?? result.invitation.hubId;
          pairingStatus = `Approved — but this device can't reach ${machine}'s hub. Check that Tailscale (VPN) is connected on this device and that Private DNS or secure DNS isn't overriding it, then try a fresh code.`;
        }
        render();
      }
      return;
    }
    request.interval = result.interval;
    if (result.kind !== "slow-down") request.expiresAt = result.expiresAt;
    pendingPairingStore.save(request);
    pairingStatus = result.kind === "claimed" ? "The machine has the code. Approve the request in its terminal to finish." : result.kind === "slow-down" ? `Checking every ${result.interval} seconds.` : "Waiting for a machine to claim the code…";
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
  pairingCancellations.begin(verifiesCleanup ? exchangeOperationGeneration : undefined);
  pairingOperations.invalidate();
  const cleared = pendingPairingStore.clear();
  pendingPairing = null;
  pairingCreateInFlight = false;
  pairingExchangeInFlight = false;
  pairingDraft = createPairingDraft(location.origin);
  stopPairingTimers();
  // Cancel discards the invitation either way. The dialog only closes once the
  // page can say the cancellation is durable; a warning behind a closed dialog
  // was a warning nobody saw (F2).
  const outcome = cancellationOutcome(cleared, verifiesCleanup);
  pairingStatus = outcome.status;
  pairingCleanupFailed = outcome.cleanupFailed;
  if (outcome.cleanupFailed || outcome.verifying) {
    render(false);
    openPairDialog();
    return;
  }
  finishCancelledPairing();
}

/** A cancellation the page can vouch for: close the dialog and say so. */
function finishCancelledPairing(): void {
  pairingCleanupFailed = false;
  pairingCancellations.supersede();
  document.querySelector<HTMLDialogElement>("#pair-dialog")?.close();
  render(false);
  toast(pairingStatus || "Pairing cancelled.");
}

/**
 * Retry the durable part of a cancellation. It never resumes the discarded
 * invitation: the persistent store is cleared again and the catalog's pending
 * rollback is re-checked, and only a fail-closed result ends the step. One
 * retry runs at a time, a rejection lands in the dialog, and a result is
 * applied only while the cancellation that started it still owns the dialog.
 */
async function retryPairingCleanup(): Promise<void> {
  const ticket = pairingCancellations.beginRetry();
  if (!ticket) return;
  pairingStatus = "Retrying cleanup…";
  render(false);
  const cleared = pendingPairingStore.clear();
  let recovery: { pendingCleanup?: number; failed?: boolean };
  try {
    recovery = { pendingCleanup: (await catalog.recoverPending()).pendingCleanup };
  } catch {
    recovery = { failed: true };
  }
  if (!pairingCancellations.finishRetry(ticket)) return;
  const outcome = cleanupRetryOutcome(cleared, recovery);
  pairingStatus = outcome.status;
  if (outcome.done) {
    finishCancelledPairing();
    return;
  }
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
  commitSelection({ machineId, session });
  await attachSelectedSession(machineId, session);
}

/** Paints and attaches an already-committed selection; back reuses it so
 * returning somewhere never records a new step forward. */
async function attachSelectedSession(machineId: string, session: string): Promise<void> {
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
    placeholder.textContent = `Connecting to ${session}…`;
  }
}

function clearDisconnectedState(grid: HTMLElement): void {
  grid.classList.remove("terminal-disconnected");
  grid.querySelector(".terminal-disconnected-banner")?.remove();
}

function openConnectionLog(machineId: string): void {
  let dialog = document.querySelector<HTMLDialogElement>("#connection-log");
  if (!dialog) {
    dialog = document.createElement("dialog");
    dialog.id = "connection-log";
    dialog.className = "connection-log";
    dialog.innerHTML = '<header><h2>Connection log</h2><form method="dialog"><button type="submit" aria-label="Close connection log">×</button></form></header><pre>Running diagnostics…</pre>';
    document.body.append(dialog);
  }
  const output = dialog.querySelector("pre")!;
  output.textContent = "Running diagnostics…";
  dialog.showModal();
  void connections.get(machineId)?.diagnose().then((result) => {
    output.textContent = JSON.stringify(result, null, 2);
  }).catch((error) => {
    output.textContent = error instanceof Error ? error.message : "Diagnosis failed";
  });
}

function renderConnectionSurface(machineId: string, session: string, snapshot: ConnectionState, now = Date.now()): void {
  if (selectedMachineId !== machineId || selectedSession !== session) return;
  const grid = document.querySelector<HTMLElement>("#pane-grid");
  if (grid?.dataset.sessionKey !== sessionKey(machineId, session)) return;
  const hasLastFrame = grid.querySelector(".pane") !== null;
  if (hasLastFrame && shouldRetainDisconnectedFrame(snapshot)) {
    const view = disconnectedView(snapshot, now);
    let banner = grid.querySelector<HTMLElement>(".terminal-disconnected-banner");
    if (!banner) {
      banner = document.createElement("div");
      banner.className = "terminal-disconnected-banner";
      banner.setAttribute("role", "status");
      grid.prepend(banner);
    }
    // A fatal failure is not reconnecting, so the banner must not claim it is.
    banner.textContent = snapshot.fatal === true
      ? snapshot.reason ?? "This browser cannot reconnect to the terminal."
      : `Disconnected ${view.elapsedSeconds}s ago — ${view.retryLabel} (attempt ${view.attempt})`;
    grid.classList.add("terminal-disconnected");
    return;
  }
  clearDisconnectedState(grid);
  if (snapshot.phase === "live") return;
  const placeholder = grid.querySelector<HTMLElement>(".empty");
  if (!placeholder) return;
  const view = connectingView(snapshot, now);
  const fatal = snapshot.fatal === true;
  placeholder.className = `empty terminal-state terminal-connecting${fatal ? " terminal-connect-failed" : ""}`;
  const title = document.createElement("p");
  title.className = "terminal-connecting-title";
  // A spinner and a rising counter over a failure that will never resolve is
  // the D3 overlay: it reads as progress. State the outcome instead.
  title.textContent = fatal ? `Cannot connect to ${session}` : `Connecting to ${session}…`;
  if (fatal) {
    placeholder.replaceChildren(title);
  } else {
    const spinner = document.createElement("span");
    spinner.className = "connection-spinner";
    spinner.setAttribute("aria-hidden", "true");
    const elapsed = document.createElement("time");
    elapsed.className = "terminal-connecting-elapsed";
    elapsed.textContent = view.elapsedLabel;
    placeholder.replaceChildren(spinner, title, elapsed);
  }
  if (view.step) {
    const step = document.createElement("p");
    step.className = "terminal-connecting-step";
    step.textContent = view.step;
    placeholder.append(step);
  }
  if (view.actionsAvailable) {
    const actions = document.createElement("div");
    actions.className = "terminal-connecting-actions";
    const retry = document.createElement("button");
    retry.type = "button";
    retry.textContent = "Retry";
    retry.onclick = () => { void connections.get(machineId)?.attach(session); };
    const diagnose = document.createElement("button");
    diagnose.type = "button";
    diagnose.textContent = "Diagnose";
    diagnose.onclick = () => openConnectionLog(machineId);
    actions.append(retry, diagnose);
    if (snapshot.authFailure === "revoked" || snapshot.authFailure === "scope-mismatch" || snapshot.authFailure === "needs-pairing") {
      const repair = document.createElement("button");
      repair.type = "button";
      repair.textContent = "Re-pair";
      repair.onclick = () => document.querySelector<HTMLDialogElement>("#pair-dialog")?.showModal();
      actions.append(repair);
    }
    placeholder.append(actions);
  }
}

function syncConnectionViewTicker(): void {
  if (connectionViewTicker !== undefined) window.clearInterval(connectionViewTicker);
  connectionViewTicker = undefined;
  const connection = activeConnection();
  if (!selectedMachineId || !selectedSession || !connection) return;
  const snapshot = connection.attachSnapshot(selectedSession) ?? connection.snapshot();
  if (snapshot.phase === "live" && !snapshot.degraded) return;
  // Nothing about a fatal state changes with time; a 1Hz repaint of it is noise.
  if (snapshot.fatal === true) return;
  const machineId = selectedMachineId;
  const session = selectedSession;
  connectionViewTicker = window.setInterval(() => {
    const current = connection.attachSnapshot(session) ?? connection.snapshot();
    renderConnectionSurface(machineId, session, current);
  }, 1_000);
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

/**
 * The pane geometry the daemon says is authoritative, per pane (cas-37f8).
 * `local` means the operator's dashboard owns the PTY: this viewer renders
 * that size and must stop asking for its own.
 */
const paneAuthority = new Map<string, { cols: number; rows: number; local: boolean }>();

function applyPaneAuthority(
  machineId: string,
  session: string,
  paneId: string,
  cols: number,
  rows: number,
  authority: string,
): void {
  const key = paneKey(machineId, session, paneId);
  const local = authority === "LocalDashboard";
  paneAuthority.set(key, { cols, rows, local });
  surfaces.get(key)?.setAuthoritativeSize(local ? { cols, rows } : null);
}

/** A viewer whose pane is owned by the local dashboard never asks again. */
function ownsPaneGeometry(machineId: string, session: string, paneId: string): boolean {
  return paneAuthority.get(paneKey(machineId, session, paneId))?.local !== true;
}

function requestPaneSize(machineId: string, session: string, paneId: string, cols: number, rows: number): void {
  if (!canResizePanes(machineId, session)) return;
  if (!ownsPaneGeometry(machineId, session, paneId)) return;
  sendControl(machineId, session, { ResizePane: { pane_id: paneId, cols, rows } });
}

function resizeViewablePanes(machineId: string, session: string): void {
  if (!canResizePanes(machineId, session)) return;
  const state = sessionStates.get(sessionKey(machineId, session));
  if (!state) return;
  for (const pane of state.panes.filter((candidate) => candidate.kind !== "Director")) {
    const surface = surfaces.get(paneKey(machineId, session, pane.id));
    if (surface) requestPaneSize(machineId, session, pane.id, surface.cols, surface.rows);
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
  const visiblePanes = state.panes.filter((pane) => pane.kind !== "Director");
  const active = new Set(visiblePanes.map((pane) => pane.id));
  if (visiblePanes.length === 0) {
    for (const [key, surface] of surfaces) {
      if (!key.startsWith(`${machineId}:${session}:`)) continue;
      releaseSurface(key, surface);
    }
    const empty = document.createElement("div");
    empty.className = "empty empty-pane-slot";
    const emptyTitle = document.createElement("p");
    emptyTitle.className = "empty-title";
    emptyTitle.textContent = "No panes in this session yet";
    const emptyHint = document.createElement("p");
    emptyHint.className = "empty-hint";
    emptyHint.textContent = "Terminals appear here as soon as the session starts one.";
    empty.replaceChildren(emptyTitle, emptyHint);
    grid.classList.remove("pane-layout", "single-pane");
    grid.replaceChildren(empty);
    return;
  }
  grid.querySelector(".empty")?.remove();
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
    if (key.startsWith(`${machineId}:${session}:`) && !active.has(key.split(":").at(-1)!)) releaseSurface(key, surface);
  }
  const panesById = new Map(visiblePanes.map((pane) => [pane.id, pane]));
  // Re-inserting a card blurs whatever it contains, so panes are only moved when
  // their slot or their position actually changed. A five-second heartbeat render
  // must not close a phone keyboard mid-command.
  const slotPositions = new Map<HTMLElement, number>();
  const placePane = (slot: HTMLElement, card: HTMLElement): void => {
    const index = slotPositions.get(slot) ?? 0;
    slotPositions.set(slot, index + 1);
    if (slot.children[index] === card) return;
    slot.insertBefore(card, slot.children[index] ?? null);
  };
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
        focusPane(machineId, session, pane.id);
      };
      const title = document.createElement("header"); title.className = "pane-header";
      const statusDot = document.createElement("span"); statusDot.className = `pane-status-dot ${pane.exited ? "exited" : "live"}`;
      const label = document.createElement("span"); label.className = "pane-title"; label.textContent = pane.title || pane.id;
      const role = document.createElement("span"); role.className = "pane-role"; role.textContent = pane.kind.toLowerCase();
      const activity = document.createElement("span"); activity.className = "pane-last-activity"; updatePaneActivity(activity, paneLastActivity.get(key));
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
        button("Show terminal", "pane-view-toggle", () => {
          setPaneViewMode(selectedKey, paneViewMode(selectedKey) === "transcript" ? "terminal" : "transcript");
        }),
        button("Find", "pane-search", () => { focusPane(machineId, session, pane.id); openTerminalSearch(); }),
        button("Make primary", "make-primary", () => updateLayout((current) => promotePane(current, pane.id))),
        button("Move earlier", "move-earlier", () => updateLayout((current) => movePane(current, pane.id, -1))),
        button("Move later", "move-later", () => updateLayout((current) => movePane(current, pane.id, 1))),
      );
      title.append(statusDot, label, role, activity, controls);
      title.title = sessionSummaries.get(selectedKey)?.title ?? "";
      title.onclick = (event) => {
        event.stopPropagation();
        // On a phone only the primary pane mounts a terminal, so a tap opens the
        // tapped pane as primary rather than toggling an empty well.
        if (phoneLayout() && !card?.classList.contains("primary")) {
          focusPane(machineId, session, pane.id);
          updateLayout((current) => promotePane(current, pane.id));
          return;
        }
        if (pane.kind === "Supervisor") return;
        const wasCollapsed = collapsedWorkerPanes.has(key);
        focusPane(machineId, session, pane.id);
        if (!wasCollapsed) collapsedWorkerPanes.add(key);
        card?.classList.toggle("collapsed", collapsedWorkerPanes.has(key));
        if (!collapsedWorkerPanes.has(key)) queueMicrotask(() => surfaces.get(key)?.focus());
      };
      title.onkeydown = (event) => {
        if (event.key !== "Enter" && event.key !== " ") return;
        event.preventDefault();
        title.click();
      };
      mount = document.createElement("div"); mount.className = "terminal-mount";
      card.append(title, mount);
    }
    if (!card || !mount) continue;
    const position = orderedPaneIds(layout).indexOf(pane.id);
    const phone = phoneLayout();
    const secondaryOnPhone = phone && pane.id !== layout.primaryPaneId;
    card.classList.toggle("primary", pane.id === layout.primaryPaneId);
    card.classList.toggle("selected", selectedPanes.get(selectedKey) === pane.id);
    // A secondary phone pane carries no terminal, so it reads as one compact row
    // instead of an empty well the size of a third of the screen.
    card.classList.toggle("collapsed", secondaryOnPhone || (pane.kind !== "Supervisor" && collapsedWorkerPanes.has(key)));
    card.querySelector<HTMLElement>(".pane-status-dot")!.className = `pane-status-dot ${pane.exited ? "exited" : "live"}`;
    card.querySelector<HTMLElement>(".pane-title")!.textContent = pane.title || pane.id;
    const paneHeader = card.querySelector<HTMLElement>(".pane-header");
    if (paneHeader) {
      const hint = secondaryOnPhone
        ? "Tap to open this pane"
        : pane.kind === "Supervisor" ? undefined : "Click to collapse or expand this worker";
      paneHeader.title = [sessionSummaries.get(selectedKey)?.title, hint].filter(Boolean).join(" · ");
      if (hint) {
        paneHeader.tabIndex = 0;
        paneHeader.setAttribute("role", "button");
        paneHeader.setAttribute("aria-label", `${pane.title || pane.id}: ${hint}`);
      } else {
        paneHeader.removeAttribute("tabindex");
        paneHeader.removeAttribute("role");
        paneHeader.removeAttribute("aria-label");
      }
    }
    updatePaneActivity(card.querySelector<HTMLElement>(".pane-last-activity")!, paneLastActivity.get(key));
    const makePrimary = card.querySelector<HTMLButtonElement>(".make-primary");
    const moveEarlier = card.querySelector<HTMLButtonElement>(".move-earlier");
    const moveLater = card.querySelector<HTMLButtonElement>(".move-later");
    if (makePrimary) makePrimary.disabled = pane.id === layout.primaryPaneId;
    if (moveEarlier) moveEarlier.disabled = pane.id === layout.primaryPaneId || position <= 1;
    if (moveLater) moveLater.disabled = pane.id === layout.primaryPaneId || position === layout.paneIds.length - 1;
    const paneView = paneViewMode(selectedKey);
    const viewToggle = card.querySelector<HTMLButtonElement>(".pane-view-toggle");
    if (viewToggle) {
      const label = paneView === "transcript" ? "Show terminal" : "Show transcript";
      viewToggle.textContent = label;
      viewToggle.setAttribute("aria-label", label);
      viewToggle.title = paneView === "transcript"
        ? "Show the true terminal grid"
        : "Read this pane as reflowed text";
      viewToggle.dataset.view = paneView;
    }
    placePane(pane.id === layout.primaryPaneId ? primarySlot : secondaryStrip, card);
    const collapsedOnPhone = phoneLayout() && secondaryOnPhone;
    const existingSurface = surfaces.get(key);
    existingSurface?.setControlMode(leases.get(selectedKey)?.held_by_me === true);
    if (existingSurface && (collapsedOnPhone || existingSurface.element !== mount || !existingSurface.element.isConnected)) {
      releaseSurface(key, existingSurface);
    }
    if (collapsedOnPhone) continue;
    if (!surfaces.has(key)) {
      const surface = await createTerminalSurface(mount, {
        onData: (data) => { if (canControl(machineId, session, "pane-input")) sendControl(machineId, session, { Input: { pane_id: pane.id, data: [...data] } }); },
        onResize: (cols, rows) => requestPaneSize(machineId, session, pane.id, cols, rows),
        // The transcript is a reading of the same frame the grid just rendered,
        // so it follows the emulator's own tick instead of polling it.
        onRender: () => transcripts.get(key)?.update(),
      });
      const currentMount = document.querySelector<HTMLElement>(`[data-pane-id="${CSS.escape(pane.id)}"] .terminal-mount`);
      if (selectedMachineId !== machineId || selectedSession !== session || !mount.isConnected || currentMount !== mount) {
        surface.dispose();
        continue;
      }
      surfaces.set(key, surface);
      surface.setControlMode(leases.get(selectedKey)?.held_by_me === true);
      // The floor goes in before the replay: scrollback written at the mount's
      // own narrow grid would only have to be reflowed again.
      surface.setMinimumColumns(compactViewport() ? COMPACT_MINIMUM_COLUMNS : 0);
      // A pane the operator's dashboard already claimed is pinned before the
      // replay too, so the buffer is never written at a grid that is about to
      // change (cas-37f8).
      const authority = paneAuthority.get(key);
      if (authority?.local) surface.setAuthoritativeSize({ cols: authority.cols, rows: authority.rows });
      const buffered = paneBuffers.get(key);
      if (buffered) surface.write(new Uint8Array(buffered));
    }
    const mounted = surfaces.get(key);
    if (mounted) applyPaneView(key, mount, mounted, paneView);
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
  if (!hubSupports(machine.id, "daemon_attach")) return "This hub does not support Cassy Commander control. Upgrade the hub, then reconnect this machine.";
  const missingScopes = ["pane-input", "message-send", "pane-interrupt"] as const;
  if (missingScopes.some((scope) => !machine.scopes.includes(scope))) {
    return `Relay pairing granted read-only scopes for ${location.origin}. Run cas hub pair --origin ${location.origin}, open the new pairing URL here, and approve control access on ${machine.label}. Pairings are specific to each Cassy Commander origin.`;
  }
  if (lease?.held_by_me) return undefined;
  if (lease?.controller_label) return `${lease.controller_label} currently controls this session. Wait for it to be released or use an administrator credential to take over.`;
  return "Take control to enable terminal input, messages, and interrupts.";
}

function takeControlDisabledReason(machine: StoredMachine | undefined, session: string | undefined, lease: LeaseState | undefined): string | undefined {
  const reason = controlDisabledReason(machine, session, lease);
  if (!machine || !session || !hubSupports(machine.id, "daemon_attach")) return reason;
  if (!["pane-input", "message-send", "pane-interrupt"].every((scope) => machine.scopes.includes(scope as Scope))) return reason;
  if (!lease?.held_by_me && lease?.controller_label && !machine.scopes.includes("hub-admin")) return reason;
  return undefined;
}

function sendControl(machineId: string, session: string, message: unknown): boolean {
  if (connections.get(machineId)?.send(session, message)) return true;
  toast("Terminal is reconnecting");
  return false;
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
  let held = false;
  for (const [key, lease] of leases) {
    if (!key.startsWith(`${machineId}:`)) continue;
    held ||= lease.held_by_me;
    // The controller identity was learned over the connection that just died.
    // Keeping it told the operator that another controller — in fact this very
    // browser — was holding the session against them.
    leases.set(key, { ...lease, held_by_me: false, controller_label: undefined, controller_device_id: undefined });
  }
  // Control disappearing in silence invites typing into a terminal that is no
  // longer listening.
  if (held) toast("Control released — the hub connection dropped");
}

let toastTimer: number | undefined;

/**
 * The toast lives on document.body, not inside the rendered shell: every render
 * replaces app.innerHTML, and a confirmation that a heartbeat can delete a
 * moment after it appears is not a confirmation.
 */
function toast(message: string): void {
  let output = document.querySelector<HTMLElement>("#toast");
  if (!output) {
    output = document.createElement("div");
    output.id = "toast";
    output.setAttribute("role", "status");
    document.body.append(output);
  }
  output.textContent = message;
  output.classList.add("visible");
  if (toastTimer !== undefined) window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => output.classList.remove("visible"), 3200);
}

function connectionLabel(state: ConnectionState | AttachSnapshot | undefined): string {
  if (!state) return "idle";
  if (state.phase === "live") {
    if (state.degraded) return `degraded · ${state.missedHeartbeats} missed`;
    return state.latencyMs === undefined ? "live" : `live · ${state.latencyMs}ms`;
  }
  if (state.phase === "backoff") return `retrying ${state.stage} in ${Math.ceil((state.retryInMs ?? 0) / 1000)}s`;
  if (state.phase === "failed") return state.reason ?? `failed during ${state.stage}`;
  const elapsed = "session" in state ? attachElapsedSeconds(state) : elapsedSeconds(state);
  return `${state.phase} · ${elapsed}s`;
}

function connectionClass(state: ConnectionState | undefined): string { return state?.degraded ? "degraded" : state?.phase ?? "idle"; }

function pairingDetails(origin: string, scopes: readonly Scope[]): string {
  return `<dl class="pair-details"><div><dt>Cassy Commander origin</dt><dd>${escapeHtml(origin)}</dd></div><div><dt>Scopes</dt><dd>${scopes.map(scopeLabel).map(escapeHtml).join(", ")}</dd></div></dl>`;
}

function pairStatusMarkup(): string {
  return `<p class="pair-status" role="status"${pairingStatus ? "" : " hidden"}>${escapeHtml(pairingStatus)}</p>`;
}

function pairDialogMarkup(): string {
  if (pairingCleanupFailed) {
    // Cancel already discarded the invitation; this step exists because the
    // page cannot yet prove a reload will not see it again. There is no way
    // back to the invitation from here, only forward through the cleanup.
    return `<dialog id="pair-dialog"><section class="pair-flow pair-cleanup" tabindex="-1" autofocus aria-labelledby="pair-cleanup-title"><h2 id="pair-cleanup-title">Could not finish cancelling</h2><p>Pairing was cancelled on this page and the discarded invitation cannot be resumed here. Browser storage refused to record the cancellation, so a reload could still see it.</p><p>Keep this page open and retry once browser storage is available. Cancelling here blocks this browser only; copies of the link elsewhere remain subject to the machine's own expiry.</p>${pairStatusMarkup()}<div class="dialog-actions"><button id="pair-close" type="button" data-role="cleanup">Close</button><button id="pair-cleanup-retry" type="button" class="primary">Retry cleanup</button></div></section></dialog>`;
  }
  if (pendingPairing?.kind === "relay-request") {
    return `<dialog id="pair-dialog"><section class="pair-flow"><h2>Pair this machine</h2><p>Run <code>cas hub authorize ${escapeHtml(pendingPairing.userCode)}</code> on the machine you want to pair, then approve the request it prints.</p><div class="pair-code" aria-label="Pairing code">${escapeHtml(pendingPairing.userCode)}</div><div class="pair-code-actions"><button id="pair-copy" type="button" data-pair-command="cas hub authorize ${escapeAttr(pendingPairing.userCode)}">Copy command</button></div><p>Expires in <strong id="pair-countdown">10:00</strong></p>${pairingDetails(pendingPairing.controllerOrigin, pendingPairing.requestedScopes)}${pairStatusMarkup()}<div class="dialog-actions"><button id="pair-cancel" type="button">Cancel</button></div></section></dialog>`;
  }
  if (pendingPairing?.kind === "invitation") {
    const relay = Boolean(pendingPairing.relay);
    const hubUrl = pendingPairing.hubUrl;
    const origin = pendingPairing.controllerOrigin;
    const invitationScopes = pendingPairing.scopes;
    return `<dialog id="pair-dialog"><form id="pair-form"><h2>${relay ? "Machine authorized" : "Pair a machine"}</h2><p>${relay ? "Verify the machine details, then create this browser's device credential." : "One-time invitation ready. Confirm the target hub."}</p>${relay && hubUrl && origin && invitationScopes ? `<dl class="pair-details"><div><dt>Machine</dt><dd>${escapeHtml(pendingPairing.machineLabel ?? pendingPairing.hubId)}</dd></div><div><dt>Hub</dt><dd>${escapeHtml(hubUrl)}</dd></div><div><dt>Cassy Commander origin</dt><dd>${escapeHtml(origin)}</dd></div><div><dt>Granted scopes</dt><dd>${invitationScopes.map(scopeLabel).map(escapeHtml).join(", ")}</dd></div></dl><p>Invitation expires in <strong id="pair-countdown">10:00</strong></p>` : `<label>Hub URL<input name="url" type="url" required autofocus value="${escapeAttr(pairingDraft.hubUrl)}"></label><label>Machine label<input name="label" required placeholder="Studio Mac" value="${escapeAttr(pairingDraft.machineLabel)}"></label><fieldset><legend>Scopes requested</legend>${scopeChecks(pairingDraft.scopes, invitationScopes)}</fieldset>${scopeCeilingHint(invitationScopes)}`}<label>Device label<input name="device" required autofocus value="${escapeAttr(pairingDraft.deviceLabel)}"></label><label>Operator label<input name="operator" required placeholder="Your name" value="${escapeAttr(pairingDraft.operatorLabel)}"></label>${pairStatusMarkup()}<div class="dialog-actions"><button id="pair-cancel" type="button">Cancel</button><button type="submit" class="primary" ${pairingExchangeInFlight ? "disabled" : ""}>${pairingExchangeInFlight ? "Pairing…" : "Pair"}</button></div></form></dialog>`;
  }
  const relayAction = relayOrigin
    ? `<button id="pair-create" type="button" class="primary" ${pairingCreateInFlight ? "disabled" : ""}>${pairingCreateInFlight ? "Creating…" : "Create pairing code"}</button>`
    : '<p class="pairing-disabled-reason">Page-initiated pairing is unavailable because this Cassy Commander build has no reviewed relay origin.</p>';
  return `<dialog id="pair-dialog"><section class="pair-flow" tabindex="-1" autofocus><h2>Pair this machine</h2><p>Create a ten-minute code, then verify the exact Cassy Commander origin and approve the requested read and control scopes on the target machine.</p>${pairingDetails(location.origin, DEFAULT_PAIRING_SCOPES)}<label>Email code (optional)<input id="pair-email" type="email" autocomplete="email" placeholder="operator@example.com" value="${escapeAttr(pairingDraft.email)}"></label>${pairStatusMarkup()}<div class="dialog-actions"><button id="pair-close" type="button">${pairingCreateInFlight ? "Cancel" : "Close"}</button>${pendingPairing ? "" : '<p class="pairing-disabled-reason">Pair is disabled until you open a pairing URL generated by <code>cas hub pair</code> on the machine.</p>'}<button type="button" ${pendingPairing ? "" : "disabled"}>Pair</button>${relayAction}</div></section></dialog>`;
}

// A phone sentence takes longer to type than the heartbeat render interval, so
// the composer draft survives re-render exactly like the pairing draft does.
let messageDraft = "";
let messageDraftSelection = 0;

function captureMessageDraft(): void {
  const composer = document.querySelector<HTMLTextAreaElement>("#message-text");
  if (!composer) return;
  messageDraft = composer.value;
  messageDraftSelection = composer.selectionStart ?? composer.value.length;
}

function restoreMessageDraft(): void {
  const composer = document.querySelector<HTMLTextAreaElement>("#message-text");
  if (!composer || !messageDraft) return;
  composer.value = messageDraft;
  const caret = Math.min(messageDraftSelection, messageDraft.length);
  composer.setSelectionRange(caret, caret);
}

function speechStatusText(): string {
  if (speechInputState === "listening") return "Listening… speak a short message, then review it before sending.";
  if (speechInputState === "error") return speechInputDetail;
  if (speechCapability?.mode === "local") return "On-device voice ready — tap the mic, review, then send.";
  return "Voice ready — tap the mic, review, then send.";
}

function syncSpeechComposer(): void {
  const mic = document.querySelector<HTMLButtonElement>("#message-mic");
  const status = document.querySelector<HTMLElement>("#speech-status");
  if (!mic || !status) return;
  const available = speechCapability !== undefined && speechCapability.mode !== "typing";
  mic.hidden = !available;
  status.hidden = !available && speechInputDetail.length === 0;
  mic.classList.toggle("listening", speechInputState === "listening");
  mic.setAttribute("aria-pressed", String(speechInputState === "listening"));
  mic.querySelector<HTMLElement>("[data-mic-label]")!.textContent = speechInputState === "listening" ? "Stop listening" : "Tap to talk";
  status.textContent = speechStatusText();
}

function createSpeechController(capability: SpeechInputCapability): SpeechDictationController {
  return new SpeechDictationController(capability, {
    read: () => document.querySelector<HTMLTextAreaElement>("#message-text")?.value ?? messageDraft,
    write: (value) => {
      messageDraft = value;
      messageDraftSelection = value.length;
      messageDelivery = undefined;
      const composer = document.querySelector<HTMLTextAreaElement>("#message-text");
      if (!composer) return;
      composer.value = value;
      composer.setSelectionRange(value.length, value.length);
      const delivery = document.querySelector<HTMLElement>("#message-delivery");
      if (delivery) delivery.hidden = true;
    },
    state: (next, detail = "") => {
      speechInputState = next;
      speechInputDetail = detail;
      syncSpeechComposer();
    },
    permissionDenied: () => {
      speechCapability = { mode: "typing", language: capability.language };
      speechController = undefined;
      speechInputState = "idle";
      speechInputDetail = "Mic permission was not granted. Type your message instead.";
      syncSpeechComposer();
    },
  });
}

function bindSpeechComposer(): void {
  const composer = document.querySelector<HTMLTextAreaElement>("#message-text");
  const keyboard = document.querySelector<HTMLButtonElement>("#message-keyboard");
  const mic = document.querySelector<HTMLButtonElement>("#message-mic");
  if (!composer || !keyboard || !mic) return;
  composer.oninput = () => {
    messageDraft = composer.value;
    messageDraftSelection = composer.selectionStart ?? composer.value.length;
    messageDelivery = undefined;
    const delivery = document.querySelector<HTMLElement>("#message-delivery");
    if (delivery) delivery.hidden = true;
  };
  // Enter sends. Without this the composer looked functional and delivered
  // nothing: the keypress only added a newline, in observe and control mode
  // alike.
  composer.onkeydown = (event) => {
    if (!sendsOnEnter(event)) return;
    event.preventDefault();
    void submitSupervisorMessage();
  };
  keyboard.onclick = () => composer.focus();
  mic.onclick = () => speechController?.toggle();
  syncSpeechComposer();
  if (speechDetectionStarted) return;
  speechDetectionStarted = true;
  void detectSpeechInput().then((capability) => {
    speechCapability = capability;
    speechController = capability.mode === "typing" ? undefined : createSpeechController(capability);
    syncSpeechComposer();
  });
}

function openSupervisorComposer(): void {
  activeContextTab = "status";
  attentionPanelCollapsed = false;
  render();
  queueMicrotask(() => {
    const composer = document.querySelector<HTMLTextAreaElement>("#message-text");
    composer?.scrollIntoView({ block: "nearest" });
    // Voice is one labelled tap away; focus belongs in the field that accepts text.
    composer?.focus();
  });
}

/**
 * The composer's own status line. The hub can refuse a supervisor message for
 * reasons the operator cannot see — a device paired without message:send, a
 * session someone else controls, a transport that is reconnecting — and each of
 * those used to look identical to a Send button that does nothing.
 */
function showComposerStatus(text: string, tone: "info" | "error"): void {
  messageStatus = { session: selectedSession, text, tone };
  // A stale "Message sent" beside a refusal reads as a contradiction.
  messageDelivery = undefined;
  const delivery = document.querySelector<HTMLElement>("#message-delivery");
  if (delivery) delivery.hidden = true;
  const status = document.querySelector<HTMLElement>("#message-status");
  if (!status) return;
  status.hidden = false;
  status.textContent = text;
  status.classList.toggle("error", tone === "error");
}

function clearComposerStatus(): void {
  messageStatus = undefined;
  const status = document.querySelector<HTMLElement>("#message-status");
  if (!status) return;
  status.hidden = true;
  status.textContent = "";
  status.classList.remove("error");
}

function supervisorSendContext(text: string): Parameters<typeof planSupervisorSend>[0] {
  const machine = selectedMachineId ? machines.get(selectedMachineId) : undefined;
  const lease = machine && selectedSession ? leases.get(sessionKey(machine.id, selectedSession)) : undefined;
  const session = machine && selectedSession ? sessions.get(machine.id)?.find((item) => item.name === selectedSession) : undefined;
  return {
    text,
    machineLabel: machine?.label,
    session: selectedSession,
    supervisor: supervisorTarget(session),
    daemonAttach: machine ? hubSupports(machine.id, "daemon_attach") : false,
    scopes: machine?.scopes ?? [],
    leaseHeldByMe: lease?.held_by_me === true,
    leaseControllerLabel: lease?.held_by_me ? undefined : lease?.controller_label,
    commanderOrigin: location.origin,
  };
}

/**
 * The hub only accepts a supervisor message from the device holding the session
 * lease (hub/server.rs handle_client_message). Observing operators were left
 * with a dead button; taking the lease is the step they would otherwise perform
 * by hand, and it succeeds only when no one else controls the session.
 */
async function takeControlForMessage(machine: StoredMachine, session: string): Promise<boolean> {
  try {
    await connections.get(machine.id)?.requestControl(session, false);
  } catch {
    return false;
  }
  await loadLease(machine.id, session);
  return leases.get(sessionKey(machine.id, session))?.held_by_me === true;
}

function deliverSupervisorMessage(machine: StoredMachine, session: string, supervisor: string, text: string): void {
  const sent = sendControl(machine.id, session, supervisorMessage(supervisor, text));
  // Without an outcome the operator cannot tell a sent message from a lost
  // one, and the natural response is to send it a second time.
  if (!sent) {
    showComposerStatus("The hub connection is reconnecting, so this message was not delivered. Try again once the session is live.", "error");
    return;
  }
  clearComposerStatus();
  const composer = document.querySelector<HTMLTextAreaElement>("#message-text");
  if (composer) composer.value = "";
  messageDraft = "";
  messageDraftSelection = 0;
  messageDelivery = { session, target: supervisor };
  const delivery = document.querySelector<HTMLElement>("#message-delivery");
  if (delivery) {
    delivery.hidden = false;
    delivery.textContent = `Message sent to ${supervisor}`;
  }
  toast(`Message sent to ${supervisor}`);
  // A phone operator usually has a second sentence; keep the caret where they
  // left it rather than dropping focus to the page body.
  composer?.focus();
}

async function submitSupervisorMessage(): Promise<void> {
  const composer = document.querySelector<HTMLTextAreaElement>("#message-text");
  if (!composer) return;
  const text = composer.value.trim();
  const plan = planSupervisorSend(supervisorSendContext(text));
  if (plan.kind === "blocked") {
    showComposerStatus(plan.reason, "error");
    composer.focus();
    return;
  }
  const machine = selectedMachineId ? machines.get(selectedMachineId) : undefined;
  const session = selectedSession;
  const supervisor = machine && session ? supervisorTarget(sessions.get(machine.id)?.find((item) => item.name === session)) : undefined;
  if (!machine || !session || !supervisor) return;
  if (plan.kind === "take-control-then-send") {
    showComposerStatus(plan.notice, "info");
    if (!await takeControlForMessage(machine, session)) {
      showComposerStatus(`Could not take control of ${session}, and the hub refuses a message from a device that is only observing. Take control from the header, then send again.`, "error");
      return;
    }
  }
  deliverSupervisorMessage(machine, session, supervisor, text);
}

/**
 * The canvas is the first thing a new operator reads. With nothing paired it has
 * to offer pairing — pointing at a session list that cannot exist yet is a dead
 * end, not an instruction.
 */
function emptyCanvasMarkup(): string {
  if (!machineCatalogLoaded) {
    return '<p class="empty-title">Loading paired machines…</p>';
  }
  if (machines.size === 0) {
    return '<p class="empty-title">No machine paired yet</p><p class="empty-hint">Pair the machine your sessions run on. You will get a code to approve there.</p><button id="empty-pair" class="primary" type="button">Pair a machine</button>';
  }
  return '<p class="empty-title">No session open</p><p class="empty-hint">Pick a session to attach its supervisor and workers.</p><button id="open-machines" class="primary" type="button">Open machines</button>';
}

function capturePairingDraft(): void {
  const email = document.querySelector<HTMLInputElement>("#pair-email");
  if (email) pairingDraft.email = email.value;
  const form = document.querySelector<HTMLFormElement>("#pair-form");
  if (form) pairingDraft = updatePairingDraft(pairingDraft, new FormData(form).entries(), pendingPairing?.kind === "invitation" && !pendingPairing.hubUrl);
}

function render(captureDraft = true): void {
  if (captureDraft) capturePairingDraft();
  captureMessageDraft();
  const composerWasFocused = document.activeElement?.id === "message-text";
  const selected = selectedMachineId ? machines.get(selectedMachineId) : undefined;
  const lease = selected && selectedSession ? leases.get(sessionKey(selected.id, selectedSession)) : undefined;
  const status = selected && selectedSession ? statuses.get(sessionKey(selected.id, selectedSession)) : undefined;
  const compatibility = selected ? compatibilityWarning(selected.id) : undefined;
  const machineConnectionSnapshot = selected ? connectionStates.get(selected.id) : undefined;
  const terminalAttachSnapshot = selected && selectedSession ? attachStates.get(sessionKey(selected.id, selectedSession)) : undefined;
  const connectionSnapshot = terminalAttachSnapshot ?? machineConnectionSnapshot;
  const controlReason = controlDisabledReason(selected, selectedSession, lease);
  const takeControlReason = takeControlDisabledReason(selected, selectedSession, lease);
  const selectedHubSession = selected && selectedSession
    ? sessions.get(selected.id)?.find((item) => item.name === selectedSession)
    : undefined;
  const supervisor = supervisorTarget(selectedHubSession);
  const delivery = selectedSession && messageDelivery?.session === selectedSession ? messageDelivery : undefined;
  // Evaluated with the draft the operator can actually see, so the button's
  // stated reason and the reason a send would print are the same sentence. It
  // never carries `disabled`: a disabled Send button swallows the tap and looks
  // exactly like a broken one.
  const sendPlan = planSupervisorSend(supervisorSendContext(messageDraft));
  const sendReason = sendPlan.kind === "blocked" && sendPlan.block !== "empty" ? sendPlan.reason : undefined;
  const composerStatus = messageStatus?.session === selectedSession ? messageStatus : undefined;
  // A phone has no hover, so a title attribute is an explanation nobody can
  // reach. Unavailable controls stay focusable and say why when tapped.
  const interruptReason = !selected || !selectedSession || !canControl(selected.id, selectedSession, "pane-interrupt")
    ? controlReason ?? "Interrupt is unavailable for this session."
    : undefined;
  // Workers and tasks keep rendering the last snapshot while a hub is
  // unreachable. Presented unlabelled, that reads as current truth.
  const statusIsStale = Boolean(selected) && machineConnectionSnapshot !== undefined
    && (machineConnectionSnapshot.phase !== "live" || machineConnectionSnapshot.degraded);
  const lastLive = selected ? lastLiveAt.get(selected.id) : undefined;
  const staleStatusAge = lastLive === undefined ? undefined : relativeTimestamp(lastLive);
  const staleStatusTail = staleStatusAge === undefined
    ? ""
    : ` Showing the last state received ${staleStatusAge === "now" ? "just now" : `${staleStatusAge} ago`}.`;
  // The sentence, not the element: the element is always in the shell so a
  // heartbeat can fill or empty it without rebuilding the status section.
  const staleStatusText = statusIsStale ? `Not live — reconnecting.${staleStatusTail}` : undefined;
  const terminalSessionKey = selected && selectedSession ? sessionKey(selected.id, selectedSession) : undefined;
  const connectionState = connectionClass(connectionSnapshot);
  const connectionText = selected ? connectionLabel(connectionSnapshot) : "idle";
  const latency = machineConnectionSnapshot?.latencyMs;
  const latencyText = latency === undefined ? "Status unavailable" : `${latency}ms`;
  const counts = attentionCounts(attention);
  const infoItems = dismissableInfoItems(attention);
  // With no paired machine and no event to inspect, the canvas is the only
  // useful surface. The rail's second pairing button and the empty attention
  // well otherwise split a phone into three unrelated empty states.
  const fleetEmpty = machineCatalogLoaded && machines.size === 0 && attention.length === 0;
  const showSessionControls = selected !== undefined && selectedSession !== undefined;
  // With machines paired and nothing open, the canvas is the fleet: every
  // machine and its sessions, one tap from opening. An empty card pointing at a
  // drawer was a detour to the same list.
  const showFleetBoard = machineCatalogLoaded && machines.size > 0 && selectedSession === undefined;
  const mode = lease?.held_by_me ? "CONTROL" : "OBSERVER";
  const controlActionLabel = lease?.held_by_me ? "Release control" : lease?.controller_label && selected?.scopes.includes("hub-admin") ? "Force takeover" : "Take control";
  const machineLabel = selected?.label ?? "No machine";
  const compactMachineLabel = machineLabel.split(/\s+/).filter(Boolean).slice(0, 2).map((part) => part[0]?.toUpperCase()).join("") || "—";
  const controlActionDisabled = takeControlReason !== undefined;
  const sessionCommands = [...machines.values()].flatMap((machine) => (sessions.get(machine.id) ?? []).map((session) => {
    const summary = sessionSummaries.get(sessionKey(machine.id, session.name));
    const searchMetadata = summary ? `${summary.title} ${summary.description} ${summary.phase}` : "";
    const secondary = summary ? `${machine.label} · ${summary.title} · ${summary.phase}` : machine.label;
    return `<button type="button" class="palette-command" data-palette-machine="${escapeAttr(machine.id)}" data-palette-session="${escapeAttr(session.name)}" data-search-text="${escapeAttr(searchMetadata)}"><span>Jump to ${escapeHtml(session.name)}</span><small${summary ? ` title="${escapeAttr(summary.description)}"` : ""}>${escapeHtml(secondary)}</small></button>`;
  })).join("");
  const backTarget = previousSelection(selection);
  const backText = backLabel(backTarget, (machineId) => machines.get(machineId)?.label);
  // The session name is the switch: on a phone it is the only always-visible
  // chrome that can carry one, and the ⌘K palette is hidden below 500px.
  const sessionCount = [...machines.values()].reduce((total, machine) => total + (sessions.get(machine.id)?.length ?? 0), 0);
  const sessionPickerLabel = sessionCount === 0
    ? "Switch session — no sessions listed yet"
    : `Switch session — ${sessionCount} available`;
  const liveRegions: LiveRegionView = {
    ...(selected ? {
      connection: { state: connectionState, title: compatibility ?? connectionText, latencyText },
      mode: { badge: mode, compact: lease?.held_by_me ? "CTL" : "OBS" },
    } : {}),
    ...(showSessionControls ? {
      controlAction: { label: controlActionLabel, ...(takeControlReason ? { disabledReason: takeControlReason } : {}) },
    } : {}),
    ...(interruptReason ? { interruptReason } : {}),
    ...(staleStatusText ? { staleNotice: staleStatusText } : {}),
    ...(controlReason ? { controlReason } : {}),
    ...(sendReason ? { sendReason } : {}),
    ...(composerStatus ? { messageStatus: { text: composerStatus.text, error: composerStatus.tone === "error" } } : {}),
    ...(delivery ? { delivery: `Message sent to ${delivery.target}` } : {}),
    pairing: {
      ...(pairingStatus ? { status: pairingStatus } : {}),
      exchangeInFlight: pairingExchangeInFlight,
      createInFlight: pairingCreateInFlight,
      cleanupRetryInFlight: pairingCancellations.retrying,
    },
  };
  // The pairing dialog's step: which flow, which request, its expiry, and an
  // outstanding cleanup. The status sentence and busy flags are live regions.
  const pairingView = [
    pendingPairing?.kind ?? "",
    pendingPairing?.kind === "relay-request" ? pendingPairing.userCode : pendingPairing?.token ?? "",
    pendingPairing?.expiresAt ?? "",
    pairingCleanupFailed ? "cleanup-failed" : "",
  ].join("|");
  const signature = shellSignature({
    machineId: selectedMachineId,
    session: selectedSession,
    // Label as well as id: a credential refresh can rename a machine, and the
    // header chip and rail read that label.
    machineIds: [...machines.values()].map((machine) => `${machine.id}:${machine.label}`),
    sessionKeys: [...machines.keys()].flatMap((id) => (sessions.get(id) ?? []).map((item) => `${id}/${item.name}`)),
    catalogLoaded: machineCatalogLoaded,
    drawerOpen: machineDrawerOpen,
    attentionCollapsed: attentionPanelCollapsed,
    contextTab: activeContextTab,
    fleetEmpty,
    supervisor,
    backLabel: backTarget ? backText : undefined,
    compatibility,
    leaseHeldByMe: lease?.held_by_me === true,
    leaseController: lease?.controller_label,
    controlDisabled: controlActionDisabled,
    commandPaletteOpen,
    sessionPickerOpen,
    pairingView,
  });
  const active = document.activeElement;
  const composing = isEditableElement(active) && app.contains(active);
  const decision = renderDecision({
    signatureChanged: signature !== lastShellSignature,
    composing,
    pairingStepChanged: pairingView !== lastPairingView,
    focusInPairingDialog: composing && document.querySelector("#pair-dialog")?.contains(active) === true,
  });
  if (decision !== "shell") {
    // A deferred rebuild is owed to a structural change that arrived while the
    // operator was mid-sentence; it runs the moment the field is left.
    if (decision === "defer") deferredRender.defer();
    renderRegions({ selected, session: selectedSession, status, connectionSnapshot, counts, liveRegions });
    return;
  }
  deferredRender.settled();
  const currentGrid = document.querySelector<HTMLElement>("#pane-grid");
  const pairDialogWasOpen = document.querySelector<HTMLDialogElement>("#pair-dialog")?.open === true;
  const preservedGrid = terminalSessionKey && currentGrid?.dataset.sessionKey === terminalSessionKey ? currentGrid : undefined;
  // Moving the live grid through app.innerHTML temporarily detaches its hidden
  // textarea. Remember terminal focus so a heartbeat render cannot dismiss a
  // phone keyboard mid-command.
  const terminalWasFocused = preservedGrid?.contains(document.activeElement) === true
    && document.activeElement?.matches(".t3-ghostty-input") === true;
  if (preservedGrid) {
    preservedGrid.remove();
  } else {
    for (const surface of surfaces.values()) surface.dispose();
    surfaces.clear();
  }
  app.innerHTML = `
    ${browserNotice ? `<p class="browser-unsupported" role="alert">${escapeHtml(browserNotice)}</p>` : ""}
    <div class="shell${browserNotice ? " with-browser-notice" : ""}${machineDrawerOpen ? " drawer-open" : ""}${attentionPanelCollapsed ? " attention-collapsed" : " attention-expanded"}${fleetEmpty ? " fleet-empty" : ""}">
      <aside class="machine-navigation${machineDrawerOpen ? " drawer-open" : ""}" aria-label="Machines and sessions">
        <div class="machine-rail">
          <button id="machine-drawer-toggle" class="rail-control commander-mark" type="button" aria-label="Open machines and sessions" title="Machines and sessions" aria-expanded="${machineDrawerOpen}"><svg class="commander-mark-icon" viewBox="0 0 24 24" aria-hidden="true" focusable="false"><rect x="3" y="4" width="18" height="12" rx="2"></rect><path d="M8 20h8M12 16v4"></path></svg><span class="commander-mark-label">Machines</span></button>
          <nav id="machine-rail-list" aria-label="Machines"></nav>
          <button id="pair-toggle" class="rail-control pair-machine" type="button" aria-label="Pair a machine" title="Pair a machine"><span aria-hidden="true">+</span><span class="pair-machine-label">Pair</span></button>
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
          <div class="session-identity">
            ${backTarget ? `<button id="session-back" class="session-back" type="button" aria-label="${escapeAttr(backText)}" title="${escapeAttr(backText)}"><span aria-hidden="true">‹</span></button>` : ""}
            <h1 class="${selectedSession ? "toolbar-session-title" : ""}"><button id="session-picker-toggle" class="session-picker-toggle" type="button" aria-haspopup="dialog" aria-expanded="${sessionPickerOpen}" aria-label="${escapeAttr(sessionPickerLabel)}" title="${escapeAttr(sessionPickerLabel)}"><span class="session-picker-name">${escapeHtml(selectedSession ?? "Fleet overview")}</span><span class="session-picker-caret" aria-hidden="true">▾</span></button></h1>
          </div>
          ${selected ? `<span class="machine-chip" data-compact-label="${escapeAttr(compactMachineLabel)}" title="${escapeAttr(machineLabel)}">${escapeHtml(machineLabel)}</span><span class="mode-badge ${mode.toLowerCase()}" data-compact-label="${lease?.held_by_me ? "CTL" : "OBS"}">${mode}</span><span class="connection-summary ${connectionState}" title="${escapeAttr(compatibility ?? connectionText)}"><span class="connection-dot"></span><span data-machine-latency="${escapeAttr(selected.id)}">${latencyText}</span></span>` : ""}
          <div class="actions">${sessionCommands ? '<button id="command-palette-toggle" class="command-palette-trigger" type="button" aria-label="Open command palette" title="Command palette (Ctrl or Cmd + K)">⌘K</button>' : ""}${showSessionControls ? `<span class="control-action" title="${escapeAttr(takeControlReason ?? controlActionLabel)}"><button id="lease" data-compact-label="${lease?.held_by_me ? "Rel" : "Ctrl"}" aria-label="${escapeAttr(controlActionLabel)}"${takeControlReason ? ` aria-disabled="true" data-disabled-reason="${escapeAttr(takeControlReason)}" aria-describedby="control-disabled-reason"` : ""}>${controlActionLabel}</button>${takeControlReason ? `<span id="control-disabled-reason" class="sr-only">${escapeHtml(takeControlReason)}</span>` : ""}</span><button id="interrupt" class="danger" data-compact-label="Int" aria-label="Interrupt selected pane" title="${escapeAttr(interruptReason ?? "Interrupt selected pane")}"${interruptReason ? ` aria-disabled="true" data-disabled-reason="${escapeAttr(interruptReason)}"` : ""}>Interrupt</button>` : ""}</div>
        </header>
        <section id="pane-grid" class="pane-grid"${terminalSessionKey ? ` data-session-key="${escapeAttr(terminalSessionKey)}"` : ""}>${selectedSession ? '<div class="empty">Connecting to terminal…</div>' : showFleetBoard ? '<div id="fleet-board" class="fleet-board" aria-label="Fleet"></div>' : `<div class="empty empty-pane-slot">${emptyCanvasMarkup()}</div>`}</section>
        ${supervisor ? `<button id="talk-supervisor" class="talk-supervisor primary" type="button"><span>Talk to supervisor</span><small>${escapeHtml(supervisor)}</small></button>` : ""}
      </main>
      <aside class="context-panel${attentionPanelCollapsed ? " collapsed" : ""}" aria-label="Attention, workers, and tasks">
        <div class="attention-rail">
          <button id="attention-panel-toggle" class="rail-control" type="button" aria-label="${attentionPanelCollapsed ? "Expand" : "Collapse"} attention panel" aria-expanded="${!attentionPanelCollapsed}">${attentionPanelCollapsed ? "‹" : "›"}</button>
          <button id="attention-rail-counts" class="attention-rail-counts" type="button" data-open-context="attention" aria-label="Open attention"></button>
          <button id="mobile-message-toggle" class="mobile-message-toggle" type="button" aria-label="Message supervisor">✉</button>
        </div>
        <div class="context-body">
          <div class="context-tabs" role="tablist" aria-label="Operations panel">
            <button type="button" role="tab" data-context-tab="attention" aria-selected="${activeContextTab === "attention"}">Attention</button>
            <button type="button" role="tab" data-context-tab="status" aria-selected="${activeContextTab === "status"}">Workers &amp; Tasks</button>
            <button id="context-panel-close" class="context-panel-close" type="button" aria-label="Close panel">×</button>
          </div>
          <section id="attention-panel" class="context-tab" data-context-content="attention" ${activeContextTab === "attention" ? "" : "hidden"}></section>
          <section class="context-tab status-context" data-context-content="status" ${activeContextTab === "status" ? "" : "hidden"}><p class="status-stale" role="status" hidden></p><div id="status-view"></div><div class="message"><h2>Talk to ${escapeHtml(supervisor ?? "supervisor")}</h2><textarea id="message-text" placeholder="Speak or type a message, then review it before sending"></textarea><p class="control-disabled-reason" role="note" hidden></p><div class="composer-actions"><button id="message-mic" type="button" hidden aria-label="Start voice input" aria-pressed="false"><span class="mic-mark" aria-hidden="true">●</span><span data-mic-label>Tap to talk</span></button><button id="message-keyboard" type="button">Keyboard</button><button id="message-send" class="primary">Send message</button></div><p id="speech-status" class="composer-status" role="status" hidden></p><p id="message-status" class="message-status" role="status" hidden></p><p id="message-delivery" class="message-delivery" role="status" hidden></p></div></section>
        </div>
      </aside>
    </div>
    <dialog id="command-palette" class="command-palette">
      <section>
        <header><strong>Commands</strong><button id="command-palette-close" type="button" aria-label="Close command palette">×</button></header>
        <input id="command-palette-query" type="search" aria-label="Filter commands" placeholder="Type a command or session">
        <div class="palette-commands">
          <button type="button" class="palette-command" data-palette-action="control" ${controlActionDisabled ? "disabled" : ""}><span>${controlActionLabel}</span><small>${controlActionDisabled ? escapeHtml(takeControlReason ?? "Control unavailable") : "Current session"}</small></button>
          <button type="button" class="palette-command" data-palette-action="dismiss-info" ${infoItems.length === 0 ? "disabled" : ""}><span>Dismiss all info</span><small>${infoItems.length} outstanding</small></button>
          ${sessionCommands || '<p class="palette-empty">No live sessions available.</p>'}
        </div>
      </section>
    </dialog>
    <dialog id="session-picker" class="command-palette session-picker">
      <section>
        <header><strong>Sessions</strong><button id="session-picker-close" type="button" aria-label="Close session picker">×</button></header>
        <input id="session-picker-query" type="search" aria-label="Filter sessions" placeholder="Filter sessions">
        <div class="palette-commands" id="session-picker-list"></div>
      </section>
    </dialog>
    ${pairDialogMarkup()}`;
  if (preservedGrid) document.querySelector<HTMLElement>("#pane-grid")!.replaceWith(preservedGrid);
  const focusWinner = composerFocusWinner({ composerWasFocused, terminalWasFocused });
  if (focusWinner === "terminal") queueMicrotask(() => activePaneContext()?.surface.focus());
  restoreMessageDraft();
  if (focusWinner === "composer") queueMicrotask(() => document.querySelector<HTMLTextAreaElement>("#message-text")?.focus());
  lastRailSignature = undefined;
  lastShellSignature = signature;
  lastPairingView = pairingView;
  bindEvents(selected, lease);
  if (commandPaletteOpen) {
    document.querySelector<HTMLDialogElement>("#command-palette")?.showModal();
    queueMicrotask(() => document.querySelector<HTMLInputElement>("#command-palette-query")?.focus());
  }
  // A five-second heartbeat render must not slam the picker shut mid-choice.
  if (sessionPickerOpen) document.querySelector<HTMLDialogElement>("#session-picker")?.showModal();
  if (pairDialogWasOpen) document.querySelector<HTMLDialogElement>("#pair-dialog")?.showModal();
  renderRegions({ selected, session: selectedSession, status, connectionSnapshot, counts, liveRegions });
}

interface RegionContext {
  readonly selected: StoredMachine | undefined;
  readonly session: string | undefined;
  readonly status: Record<string, unknown> | undefined;
  readonly connectionSnapshot: ConnectionState | undefined;
  readonly counts: ReturnType<typeof attentionCounts>;
  readonly liveRegions: LiveRegionView;
}

/**
 * Everything a hub push can change, applied to the shell that is already on
 * screen. This runs on every render — after a rebuild, and instead of one.
 */
function renderRegions(context: RegionContext): void {
  renderMachineNavigation();
  renderSessionPicker();
  renderFleetBoard();
  const railCounts = document.querySelector("#attention-rail-counts");
  if (railCounts) {
    // Both forms ship; the compact block picks one. The button owns the
    // accessible name so the visuals can stay aria-hidden.
    railCounts.setAttribute("aria-label", `Open attention. ${attentionSummary(context.counts).description}`);
    railCounts.replaceChildren(renderAttentionSummary(context.counts), renderAttentionCounts(context.counts, true));
  }
  renderAttention();
  renderStatus(context.status);
  applyLiveRegions(app, context.liveRegions);
  if (context.selected && context.session && context.connectionSnapshot) {
    renderConnectionSurface(context.selected.id, context.session, context.connectionSnapshot);
  }
  syncConnectionViewTicker();
  if (context.selected && context.session) {
    const machineId = context.selected.id;
    const session = context.session;
    const state = sessionStates.get(sessionKey(machineId, session));
    if (state) queueMicrotask(() => void renderSessionState(machineId, session, state));
  }
  syncPairingCountdown();
}

/**
 * The rail and the drawer tree are rebuilt nodes, so they are only rebuilt when
 * something they show actually moved — otherwise a heartbeat would blur a
 * machine row the operator is on.
 */
function renderMachineNavigation(): void {
  const machineRail = document.querySelector("#machine-rail-list");
  const machineTree = document.querySelector("#machine-tree");
  if (!machineRail || !machineTree) return;
  const signature = [
    machineCatalogLoaded ? "loaded" : "loading",
    ...[...machines.values()].map((machine) => [
      machine.id,
      machine.label,
      connectionClass(connectionStates.get(machine.id)),
      connectionLabel(connectionStates.get(machine.id)),
      (sessions.get(machine.id) ?? []).map((item) => item.name).join(","),
    ].join("|")),
  ].join("~");
  if (signature === lastRailSignature) return;
  lastRailSignature = signature;
  machineRail.replaceChildren();
  machineTree.replaceChildren();
  for (const machine of machines.values()) {
    machineRail.append(machineRailButton(machine));
    machineTree.append(machineTreeGroup(machine));
  }
  if (machineCatalogLoaded && machines.size > 0) return;
  const message = document.createElement("p");
  message.className = "drawer-empty";
  message.setAttribute("role", "status");
  // Naming a control beats naming a glyph, and the machine being paired is the
  // one running the sessions — not the device holding this page.
  message.textContent = machineCatalogLoaded
    ? "No machines paired yet. Pair the machine your sessions run on."
    : "Loading paired machines…";
  machineTree.append(message);
  if (!machineCatalogLoaded) return;
  const pair = document.createElement("button");
  pair.id = "drawer-pair";
  pair.type = "button";
  pair.className = "primary drawer-pair";
  pair.textContent = "Pair a machine";
  // bindEvents only runs on a shell rebuild, and this node can be re-created by
  // a region update, so it carries its own handler.
  pair.onclick = () => document.querySelector<HTMLDialogElement>("#pair-dialog")!.showModal();
  machineTree.append(pair);
}

/**
 * The fleet in words a heartbeat cannot churn: phase only, no latency, so the
 * board is rebuilt when a machine or session actually changes state and a
 * button the operator is on is never pulled out from under a thumb.
 */
function fleetConnectionLabel(state: ConnectionState | undefined): string {
  if (!state) return "Idle";
  if (state.phase === "live") return state.degraded ? "Degraded" : "Live";
  if (state.phase === "backoff") return "Reconnecting";
  if (state.phase === "failed") return state.authFailure ? "Needs pairing" : "Unreachable";
  return "Connecting";
}

const fleetBoard = new FleetBoardRenderer();

function renderFleetBoard(): void {
  const board = document.querySelector<HTMLElement>("#fleet-board");
  const entries = sessionPickerEntries({
    machines: [...machines.values()].map((machine) => ({ id: machine.id, label: machine.label })),
    sessions,
    selection: selectedMachineId ? { machineId: selectedMachineId } : undefined,
    summaries: sessionSummaries,
  });
  fleetBoard.render(board, {
    machines: [...machines.values()].map((machine) => ({
      id: machine.id,
      label: machine.label,
      state: connectionClass(connectionStates.get(machine.id)),
      phase: fleetConnectionLabel(connectionStates.get(machine.id)),
      selected: machine.id === selectedMachineId,
    })),
    sessions: entries,
  }, {
    open: (machineId, session) => { machineDrawerOpen = false; void openSession(machineId, session); },
  });
}

function compatibilityWarning(machineId: string): string | undefined {
  const info = machineInfo.get(machineId);
  if (!info) return "Compatibility check unavailable: this hub may be older or newer. Read-only discovery may work, but controls stay disabled until it reports capabilities.";
  const missing = ["session_index", "daemon_attach", "machine_events"].filter((capability) => !info.capabilities.includes(capability));
  if (info.schema_version !== 1 || missing.length > 0) {
    return `Hub ${info.version} is version-skewed (schema ${info.schema_version}; missing ${missing.join(", ") || "no required capabilities"}). Upgrade or use a compatible Cassy Commander build; unsupported controls are disabled.`;
  }
  return undefined;
}

function machineInitials(label: string): string {
  const words = label.trim().split(/\s+/).filter(Boolean);
  return (words.length > 1 ? `${words[0][0]}${words[1][0]}` : words[0]?.slice(0, 2) ?? "?").toUpperCase();
}

function selectMachine(machine: StoredMachine): void {
  commitSelection({ machineId: machine.id });
  machineDrawerOpen = true;
  render();
}

function machineRailButton(machine: StoredMachine): HTMLButtonElement {
  const snapshot = connectionStates.get(machine.id);
  const state = connectionClass(snapshot);
  const button = document.createElement("button");
  button.className = `machine-icon ${machine.id === selectedMachineId ? "active" : ""}`;
  button.type = "button";
  // The dot leads so it can never be clipped by the chip's corner radius, and
  // the phone shows the machine's actual name instead of two initials.
  button.innerHTML = `<span class="machine-state ${state}"></span><span class="machine-initials">${escapeHtml(machineInitials(machine.label))}</span><span class="machine-name">${escapeHtml(machine.label)}</span>`;
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
  const summary = sessionSummaries.get(sessionKey(machineId, session.name));
  const stale = summary && summary.phase !== "idle" && Date.now() - Date.parse(summary.generated_at) > 10 * 60 * 1000;
  button.innerHTML = summary
    ? `<small class="session-name session-eyebrow">${escapeHtml(session.name)}</small><span class="session-summary-title">${escapeHtml(summary.title)}</span><span class="phase-chip phase-${escapeAttr(summary.phase)}">${escapeHtml(summary.phase)}</span><small class="session-summary-description${stale ? " stale" : ""}">${escapeHtml(summary.description)}</small>`
    : `<span class="session-name">${escapeHtml(session.name)}</span><small class="session-meta">${escapeHtml(session.supervisor)} · ${escapeHtml(workerCountLabel(session.workers.length))} · ${escapeHtml(session.liveness.replaceAll("_", " "))}</small>`;
  button.onclick = () => { machineDrawerOpen = false; void openSession(machineId, session.name); };
  return button;
}

function openSessionPicker(): void {
  sessionPickerOpen = true;
  render();
  queueMicrotask(() => {
    // A phone keyboard over a three-row list hides the list. The filter is
    // there for a fleet, not for the four sessions a laptop usually has.
    if (!phoneLayout()) document.querySelector<HTMLInputElement>("#session-picker-query")?.focus();
    document.querySelector<HTMLElement>("#session-picker-list [aria-current='true']")?.scrollIntoView({ block: "nearest" });
  });
}

function closeSessionPicker(): void {
  sessionPickerOpen = false;
  document.querySelector<HTMLDialogElement>("#session-picker")?.close();
}

function renderSessionPicker(): void {
  const list = document.querySelector<HTMLElement>("#session-picker-list");
  if (!list) return;
  const entries = sessionPickerEntries({
    machines: [...machines.values()].map((machine) => ({ id: machine.id, label: machine.label })),
    sessions,
    selection: selection.current ?? (selectedMachineId ? { machineId: selectedMachineId, session: selectedSession } : undefined),
    summaries: sessionSummaries,
  });
  if (entries.length === 0) {
    const empty = document.createElement("p");
    empty.className = "palette-empty";
    empty.setAttribute("role", "status");
    empty.textContent = machines.size === 0
      ? "No machine paired yet, so no sessions to switch between."
      : "No live sessions on the paired machines yet.";
    list.replaceChildren(empty);
    return;
  }
  list.replaceChildren();
  let renderedMachineId: string | undefined;
  for (const entry of entries) {
    if (entry.machineId !== renderedMachineId) {
      renderedMachineId = entry.machineId;
      const heading = document.createElement("p");
      heading.className = "picker-machine";
      heading.textContent = entry.machineLabel;
      list.append(heading);
    }
    const button = document.createElement("button");
    button.type = "button";
    button.className = "palette-command session-picker-entry";
    button.dataset.pickerMachine = entry.machineId;
    button.dataset.pickerSession = entry.session;
    button.dataset.searchText = `${entry.machineLabel} ${entry.supervisor ?? ""} ${entry.title ?? ""} ${entry.status}`;
    if (entry.current) button.setAttribute("aria-current", "true");
    // Role and status are what tell one supervisor from another; the name
    // alone reads as a random animal. The hub now derives the roster from the
    // live agent registry, so the count is stated rather than hidden.
    button.innerHTML = `<span class="session-name">${escapeHtml(entry.session)}</span><small class="session-meta">${escapeHtml(sessionPickerMeta(entry))}</small>${entry.title ? `<span class="session-summary-title">${escapeHtml(entry.title)}</span>` : ""}${entry.phase ? `<span class="phase-chip phase-${escapeAttr(entry.phase)}">${escapeHtml(entry.phase)}</span>` : ""}${entry.current ? '<span class="session-picker-current">Open</span>' : ""}`;
    button.onclick = () => {
      closeSessionPicker();
      machineDrawerOpen = false;
      void openSession(entry.machineId, entry.session);
    };
    list.append(button);
  }
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
  }, { animateIds: newCriticalAttentionIds, reclassifyIds: reclassifiedAttentionIds });
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
  if (item.session) {
    await openSession(item.machineId, item.session);
    return;
  }
  commitSelection({ machineId: item.machineId });
  render();
}

function renderStatus(status?: Record<string, unknown>): void {
  const container = document.querySelector("#status-view")!;
  // Region updates run against a container the shell rebuild is no longer
  // clearing for them, so this owns its own emptying.
  container.replaceChildren();
  if (!status) { container.textContent = "Open a session for push-refreshed status."; return; }
  const summary = selectedMachineId && selectedSession ? sessionSummaries.get(sessionKey(selectedMachineId, selectedSession)) : undefined;
  if (summary) {
    const row = document.createElement("article");
    row.className = "status-row session-summary-status";
    row.title = summary.description;
    row.innerHTML = `<span class="session-summary-title">${escapeHtml(summary.title)}</span><span class="phase-chip phase-${escapeAttr(summary.phase)}">${escapeHtml(summary.phase)}</span><small class="session-summary-description">${escapeHtml(summary.description)}</small>`;
    container.append(row);
  }
  const agents = (status.agents as any[]) ?? [];
  const tasks = [...((status.tasks_in_progress as any[]) ?? []), ...((status.tasks_ready as any[]) ?? [])];
  const identifier = (value: unknown): HTMLSpanElement => {
    const span = document.createElement("span");
    span.className = "status-identifier";
    span.textContent = String(value);
    return span;
  };
  const chip = (value: unknown): HTMLSpanElement => {
    const span = document.createElement("span");
    const state = String(value ?? "").toLowerCase().replaceAll("_", "-");
    span.className = `status-chip status-chip--${state.replaceAll(/[^a-z0-9-]/g, "") || "unknown"}`;
    span.textContent = String(value ?? "").replaceAll("_", " ");
    return span;
  };
  const sectionLabel = (text: string, count: number): HTMLParagraphElement => {
    const label = document.createElement("p");
    label.className = "status-section-label";
    label.textContent = `${text} · ${count}`;
    return label;
  };
  // Name, state, ticket, then the sentence: the identifiers stay mono and the
  // activity reads as prose, instead of one grey mono line per agent where the
  // eye had to find the dots to tell name from state from task.
  if (agents.length > 0) container.append(sectionLabel("Agents", agents.length));
  for (const agent of agents) {
    const row = document.createElement("article"); row.className = "status-row status-agent";
    const line = document.createElement("div"); line.className = "status-line";
    line.append(identifier(agent.name), chip(agent.status));
    if (agent.current_task) line.append(identifier(agent.current_task));
    row.append(line);
    if (agent.latest_activity?.summary) {
      const activity = document.createElement("p");
      activity.className = "status-activity";
      activity.textContent = agent.latest_activity.summary;
      row.append(activity);
    }
    container.append(row);
  }
  if (tasks.length > 0) container.append(sectionLabel("Tasks", tasks.length));
  for (const task of tasks) {
    const row = document.createElement("article"); row.className = "status-row status-task";
    const line = document.createElement("div"); line.className = "status-line";
    line.append(identifier(task.id), chip(task.status));
    const title = document.createElement("p");
    title.className = "status-task-title";
    title.textContent = String(task.title ?? "");
    row.append(line, title);
    container.append(row);
  }
  if (agents.length === 0 && tasks.length === 0 && !summary) {
    const empty = document.createElement("p");
    empty.className = "status-empty";
    empty.textContent = "No agents or tasks reported for this session yet.";
    container.append(empty);
  }
}

async function toggleControl(selected: StoredMachine | undefined, lease: LeaseState | undefined): Promise<void> {
  if (!selected || !selectedSession) return;
  if (lease?.held_by_me) {
    await connections.get(selected.id)?.releaseLease(selectedSession);
    invalidateMachineLeases(selected.id);
  } else {
    await connections.get(selected.id)?.requestControl(selectedSession, Boolean(lease?.controller_label && selected.scopes.includes("hub-admin")));
  }
  await loadLease(selected.id, selectedSession);
}

function openCommandPalette(): void {
  commandPaletteOpen = true;
  render();
}

function focusPaneByNumber(index: number): void {
  if (!selectedMachineId || !selectedSession) return;
  const state = sessionStates.get(sessionKey(selectedMachineId, selectedSession));
  if (!state) return;
  const panes = state.panes.filter((pane) => pane.kind !== "Director");
  const layout = layoutForPanes(sessionKey(selectedMachineId, selectedSession), panes, panes.find((pane) => pane.kind === "Supervisor")?.id);
  const paneId = layout ? orderedPaneIds(layout)[index] : undefined;
  if (paneId) focusPane(selectedMachineId, selectedSession, paneId);
}

function cycleRenderedAttention(direction: number): void {
  if (attentionPanelCollapsed || activeContextTab !== "attention") {
    attentionPanelCollapsed = false;
    activeContextTab = "attention";
    render();
  }
  queueMicrotask(() => {
    const container = document.querySelector<HTMLElement>("#attention-panel");
    if (container) cycleAttentionGroup(container, direction);
  });
}

function globalShortcut(event: KeyboardEvent): void {
  const command = event.metaKey || event.ctrlKey;
  if (command && event.key.toLowerCase() === "k") {
    event.preventDefault();
    event.stopPropagation();
    openCommandPalette();
    return;
  }
  if (command && event.key.toLowerCase() === "f" && activePaneContext()) {
    event.preventDefault();
    event.stopPropagation();
    openTerminalSearch();
    return;
  }
  if (command && /^[1-9]$/.test(event.key)) {
    event.preventDefault();
    event.stopPropagation();
    focusPaneByNumber(Number(event.key) - 1);
    return;
  }
  const target = event.target as HTMLElement | null;
  const editing = target?.matches("input, textarea, [contenteditable='true']") === true;
  if (!command && !event.altKey && !editing && (event.key === "[" || event.key === "]")) {
    event.preventDefault();
    cycleRenderedAttention(event.key === "[" ? -1 : 1);
  }
}

function bindEvents(selected: StoredMachine | undefined, lease: LeaseState | undefined): void {
  const paletteToggle = document.querySelector<HTMLButtonElement>("#command-palette-toggle");
  if (paletteToggle) paletteToggle.onclick = openCommandPalette;
  const palette = document.querySelector<HTMLDialogElement>("#command-palette")!;
  const closePalette = () => { commandPaletteOpen = false; palette.close(); };
  document.querySelector<HTMLButtonElement>("#command-palette-close")!.onclick = closePalette;
  palette.oncancel = () => { commandPaletteOpen = false; };
  const paletteQuery = document.querySelector<HTMLInputElement>("#command-palette-query")!;
  paletteQuery.oninput = () => {
    const query = paletteQuery.value.trim().toLocaleLowerCase();
    for (const command of palette.querySelectorAll<HTMLElement>(".palette-command")) {
      const searchable = `${command.textContent ?? ""} ${command.dataset.searchText ?? ""}`.toLocaleLowerCase();
      command.hidden = query.length > 0 && !searchable.includes(query);
    }
  };
  paletteQuery.onkeydown = (event) => {
    if (event.key !== "ArrowDown" && event.key !== "Enter") return;
    const first = [...palette.querySelectorAll<HTMLButtonElement>(".palette-command")]
      .find((command) => !command.hidden && !command.disabled);
    if (!first) return;
    event.preventDefault();
    if (event.key === "Enter") first.click();
    else first.focus();
  };
  for (const command of palette.querySelectorAll<HTMLButtonElement>("[data-palette-machine]")) {
    command.onclick = () => {
      commandPaletteOpen = false;
      const machineId = command.dataset.paletteMachine;
      const session = command.dataset.paletteSession;
      if (machineId && session) void openSession(machineId, session);
    };
  }
  const paletteControl = palette.querySelector<HTMLButtonElement>("[data-palette-action='control']");
  if (paletteControl) paletteControl.onclick = () => { closePalette(); void toggleControl(selected, lease); };
  const paletteDismiss = palette.querySelector<HTMLButtonElement>("[data-palette-action='dismiss-info']");
  if (paletteDismiss) paletteDismiss.onclick = () => { closePalette(); void acknowledgeAttentionGroup(dismissableInfoItems(attention)); };
  document.querySelector<HTMLButtonElement>("#session-picker-toggle")!.onclick = openSessionPicker;
  const back = document.querySelector<HTMLButtonElement>("#session-back");
  if (back) back.onclick = goBack;
  const picker = document.querySelector<HTMLDialogElement>("#session-picker")!;
  picker.oncancel = () => { sessionPickerOpen = false; };
  document.querySelector<HTMLButtonElement>("#session-picker-close")!.onclick = closeSessionPicker;
  const pickerQuery = document.querySelector<HTMLInputElement>("#session-picker-query")!;
  pickerQuery.oninput = () => {
    const query = pickerQuery.value.trim().toLocaleLowerCase();
    for (const entry of picker.querySelectorAll<HTMLElement>(".session-picker-entry")) {
      const searchable = `${entry.textContent ?? ""} ${entry.dataset.searchText ?? ""}`.toLocaleLowerCase();
      entry.hidden = query.length > 0 && !searchable.includes(query);
    }
    // A machine heading with every session filtered out is a label for nothing.
    for (const heading of picker.querySelectorAll<HTMLElement>(".picker-machine")) {
      const owned: HTMLElement[] = [];
      for (let sibling = heading.nextElementSibling; sibling instanceof HTMLElement && !sibling.classList.contains("picker-machine"); sibling = sibling.nextElementSibling) {
        owned.push(sibling);
      }
      heading.hidden = owned.length > 0 && owned.every((entry) => entry.hidden);
    }
  };
  pickerQuery.onkeydown = (event) => {
    if (event.key !== "ArrowDown" && event.key !== "Enter") return;
    const first = [...picker.querySelectorAll<HTMLButtonElement>(".session-picker-entry")].find((entry) => !entry.hidden);
    if (!first) return;
    event.preventDefault();
    if (event.key === "Enter") first.click();
    else first.focus();
  };
  document.querySelector<HTMLButtonElement>("#pair-toggle")!.onclick = () => (document.querySelector<HTMLDialogElement>("#pair-dialog")!).showModal();
  document.querySelector<HTMLButtonElement>("#machine-drawer-toggle")!.onclick = () => { machineDrawerOpen = !machineDrawerOpen; render(); };
  document.querySelector<HTMLButtonElement>("#machine-drawer-close")!.onclick = () => { machineDrawerOpen = false; render(); };
  const openMachines = document.querySelector<HTMLButtonElement>("#open-machines");
  if (openMachines) openMachines.onclick = () => { machineDrawerOpen = true; render(); };
  for (const pair of document.querySelectorAll<HTMLButtonElement>("#empty-pair, #drawer-pair")) {
    pair.onclick = () => document.querySelector<HTMLDialogElement>("#pair-dialog")!.showModal();
  }
  document.querySelector<HTMLButtonElement>("#attention-panel-toggle")!.onclick = () => { attentionPanelCollapsed = !attentionPanelCollapsed; render(); };
  document.querySelector<HTMLButtonElement>("#context-panel-close")!.onclick = () => { attentionPanelCollapsed = true; render(); };
  document.querySelector<HTMLButtonElement>("#mobile-message-toggle")!.onclick = openSupervisorComposer;
  const talkSupervisor = document.querySelector<HTMLButtonElement>("#talk-supervisor");
  if (talkSupervisor) talkSupervisor.onclick = openSupervisorComposer;
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
  const pairCopy = document.querySelector<HTMLButtonElement>("#pair-copy");
  if (pairCopy) pairCopy.onclick = () => {
    // The code has to be typed on another machine; retyping it by hand off a
    // phone screen is the error-prone step in this whole flow.
    void navigator.clipboard.writeText(pairCopy.dataset.pairCommand ?? "")
      .then(() => toast("Command copied"))
      .catch(() => toast("Copy failed — type the command shown above"));
  };
  if (pairDialogAutoOpen) {
    pairDialogAutoOpen = false;
    const opened = document.querySelector<HTMLDialogElement>("#pair-dialog");
    if (opened && !opened.open) opened.showModal();
  }
  if (pairCancel) pairCancel.onclick = cancelPendingPairing;
  // Read at click time: the label flips to Cancel through a live region while a
  // code is being minted, without rebuilding the dialog.
  if (pairClose) pairClose.onclick = () => {
    if (pairingCreateInFlight) { cancelPendingPairing(); return; }
    document.querySelector<HTMLDialogElement>("#pair-dialog")!.close();
  };
  const pairCleanupRetry = document.querySelector<HTMLButtonElement>("#pair-cleanup-retry");
  if (pairCleanupRetry) pairCleanupRetry.onclick = () => { void retryPairingCleanup(); };
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
  if (pairForm) pairForm.onsubmit = (event) => {
    event.preventDefault();
    void pairMachine(pairForm).then((paired) => {
      if (!paired) return;
      document.querySelector<HTMLDialogElement>("#pair-dialog")?.close();
      // Pairing ends by silently closing a dialog; say that it worked.
      toast(`${machines.get(selectedMachineId ?? "")?.label ?? "Machine"} paired`);
    }).catch((error) => {
      // A pairing failure is stated inside the dialog beside Pair; a toast
      // behind the backdrop only duplicated it. Anything else still surfaces.
      if (error instanceof PairingExchangeError) return;
      toast(error instanceof Error ? error.message : "Pairing failed");
    });
  };
  const remove = document.querySelector<HTMLButtonElement>("#remove-machine");
  if (remove && selected) remove.onclick = async () => {
    connections.get(selected.id)?.stop();
    connections.delete(selected.id); machines.delete(selected.id); sessions.delete(selected.id);
    await catalog.remove(selected.id);
    // Walking back into a credential that no longer exists is a dead end, and
    // reopening onto it would be the same dead end tomorrow.
    selection = forgetMachine(selection, selected.id);
    clearStoredSelection(selectionStorage());
    restoreTarget = undefined;
    const next = machines.keys().next().value;
    if (next) commitSelection({ machineId: next });
    else applySelection(undefined);
    render();
  };
  const explainIfUnavailable = (button: HTMLButtonElement): boolean => {
    const reason = button.dataset.disabledReason;
    if (!reason) return false;
    toast(reason);
    return true;
  };
  const leaseButton = document.querySelector<HTMLButtonElement>("#lease");
  if (leaseButton) leaseButton.onclick = () => {
    if (explainIfUnavailable(leaseButton)) return;
    void toggleControl(selected, lease);
  };
  const interruptButton = document.querySelector<HTMLButtonElement>("#interrupt");
  if (interruptButton) interruptButton.onclick = () => {
    if (explainIfUnavailable(interruptButton)) return;
    if (!selected || !selectedSession) return;
    const pane = selectedPanes.get(sessionKey(selected.id, selectedSession));
    if (pane) sendControl(selected.id, selectedSession, { InterruptPane: { pane_id: pane } });
  };
  bindSpeechComposer();
  document.querySelector<HTMLButtonElement>("#message-send")!.onclick = () => { void submitSupervisorMessage(); };
}

/**
 * Render the six scopes against the invitation's ceiling. A scope the machine
 * did not grant is shown, disabled, and explained — requesting it is what made
 * a default `cas hub pair` link fail its first exchange with a bare 401.
 */
function scopeChecks(selectedScopes: readonly Scope[], grantedScopes: readonly Scope[] | undefined): string {
  return scopeChoices(grantedScopes, selectedScopes).map((choice) => `<label class="scope${choice.granted ? "" : " scope-denied"}"><input type="checkbox" name="scope" value="${choice.scope}" ${choice.checked ? "checked" : ""} ${choice.granted ? "" : "disabled"}>${choice.label}${choice.granted ? "" : '<span class="scope-note">not granted by this invitation</span>'}</label>`).join("");
}

/** Name the missing scopes and the exact command that mints them. */
function scopeCeilingHint(grantedScopes: readonly Scope[] | undefined): string {
  const missing = ungrantedScopes(grantedScopes);
  if (!missing.length) return "";
  const command = pairCommand(location.origin, PAIRING_SCOPES);
  return `<p class="scope-hint">To also get ${missing.map((scope) => escapeHtml(scopeLabel(scope))).join(", ")}, run this on the machine and open the new link:</p><div class="pair-code-actions"><code>${escapeHtml(command)}</code><button id="pair-copy" type="button" data-pair-command="${escapeAttr(command)}">Copy command</button></div>`;
}

function escapeHtml(value: string): string { const span = document.createElement("span"); span.textContent = value; return span.innerHTML; }
function escapeAttr(value: string): string { return escapeHtml(value).replaceAll('"', "&quot;"); }

// A rebuild deferred while the operator was mid-sentence runs once the field is
// left AND the pointer gesture that left it has delivered its click. Rebuilding
// on focusout alone deleted the button under the finger before the browser
// dispatched the click, so the tap did nothing at all (cas-c142).
app.addEventListener("pointerdown", () => deferredRender.gestureStarted(), true);
app.addEventListener("pointerup", () => deferredRender.gestureEnded(), true);
app.addEventListener("pointercancel", () => deferredRender.gestureCancelled(), true);
app.addEventListener("focusout", () => {
  queueMicrotask(() => {
    // Moving between two fields is still composing; only a focus that has left
    // every editable control releases the rebuild.
    const active = document.activeElement;
    if (isEditableElement(active) && app.contains(active)) return;
    deferredRender.focusLeft();
  });
});

window.addEventListener("keydown", globalShortcut, true);
// Rotation changes the layout in CSS instantly, but which panes mount a
// terminal, whether the worker strip is collapsed and the PTY column floor are
// all decided in JS at render time. Without this, a phone turned on its side
// kept the composition it was mounted with until some hub event happened to
// redraw it.
for (const query of [PHONE_MEDIA_QUERY, COMPACT_MEDIA_QUERY]) {
  window.matchMedia(query).addEventListener("change", () => render());
}
render(false);
void boot();
