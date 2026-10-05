import { presentFleetSheet } from "./fleet-sheet";
import { cloudBrand, projectTitle } from "./cloud-brand";
import { CANT_REACH_RETRYING, machineFooterMarkup, orderPairedMachines, pairedMachinesDialogMarkup, renderPairedMachines, type PairedMachineRow } from "./paired-machines";
import { retainPendingSessions, visibleCatalog } from "./worker-visibility";
import "./styles.css";
import { activityTime, ConversationList, filterConversationRows, groupConversationRows, machineActivityAt, plainActivity, type ConversationRow } from "./conversation-list";
import { controlCommandCopy, paletteEnterTarget, sessionJumpCommandMarkup } from "./palette-commands";
import { applyHistoryCursor, ConversationHistory, supervisorWorking } from "./conversation-history";
import { gridPlaceholder, threadBeforePanes } from "./early-thread";
import { arrivalStore, draftStore, pendingSendStore, purgeConversations, type Arrivals, type Draft, type PendingSend } from "./conversation-store";
import { loadDismissedAsks, saveDismissedAsks, type DismissedAsksStorage } from "./dismissed-asks";
import { ConversationView, emptyActivityText, terminalOfferReason } from "./conversation-view";
import { applySheetSemantics, findByFocusKey, focusKey, layerAboveSheet, sheetFocusables, sheetKeydown } from "./attention-sheet";
import { isOperatorNotice, NOTICE_KIND, noticeFingerprint, noticeTime, planNotice } from "./operator-notices";
import { REFUSED_SEE_ABOVE, refusalSentence, refusal } from "./refusal";
import { installAttentionObjects } from "./attention-objects";
import { clearTransientAttachmentNotes, installAttachmentSheet, restateAttachmentNotes, setAttachmentNote } from "./attachment-sheet";
import { artifactFailureFollowsConnection, artifactFailureIsAboutTheFile, artifactIdFromHref, artifactIsLocalOnly, artifactLinkFor, artifactOpenFailure, openArtifact, type ArtifactMachineReach } from "./artifact-open";
import { applyTerminalOffer, arrangeConversationShell, bindKeyboardViewport, conversationAttentionBadge, keyboardViewportHeight, conversationListState, conversationNoMatchText, conversationSearchPlaceholder, conversationSkeletonMarkup, KEYBOARD_HINT_MEDIA_QUERY, paletteShortcutLabel, fitConversationHost } from "./conversation-shell";
import { clockLabel } from "./thread-model";
import { syncContextRail } from "./context-rail";
import { applyScheme, markAppearanceCommands, setScheme, type SchemePreference } from "./scheme";
import { applyAttentionEnrichment, attentionCounts, attentionSummary, attentionUrl, coalesceAttention, createAttentionItem, dismissableInfoItems, groupAttention, machineEventAttention, mergeAttentionItem, type AttentionAction, type AttentionContent, type AttentionEnrichment } from "./attention";
import { cycleAttentionGroup, renderAttentionPanel, renderAttentionSummary } from "./attention-view";
import { HubConnectionSupervisor, type ConnectionState, type HubMachineInfo } from "./connection";
import { attachElapsedSeconds, elapsedSeconds, headerConnectionChip, machineConnectionLabel, UNSTEADY, UNSTEADY_SENTENCE, type AttachSnapshot } from "./connection-state";
import { CONVERSATION_OPENING, OPENING_MOTION_DELAY_MS, attachInProgress, showOpeningInto, disconnectedView, fatalConnectionRecovery, lostConnectionBanner, outageControlsNotice, outageControlsReason, outageRefusal, pairingControlsReason, pairingLostBanner, pairingRefusal, unsteadyBanner, renderConnectionSurfaceInto, sessionOutageControlsReason, sessionReconnectingBanner, shouldRetainDisconnectedFrame, transportFailureNeedsAttention } from "./connection-state-view";
import { ensureMachineConnection, replaceMachineConnection } from "./connection-lifecycle";
import { createDeviceKey } from "./dpop";
import { readPairingFragment, watchPairingFragment } from "./fragment";
import { createPairingDraft, updatePairingDraft, type PairingStep } from "./pairing-draft";
import { bindPairingDialogCancel } from "./pairing-dialog";
import { EXPIRED_PAIRING_INVITATION_MESSAGE, INVALID_PAIRING_LINK_MESSAGE, cancellationOutcome, pairingCleanupFailureUpdate, pairingStorageClearFailureMessage, type CleanupStepContext } from "./pairing-cleanup";
import { exchangePendingPairing, PairingCleanupError, PairingExchangeError, PairingStorageError } from "./pairing-exchange";
import { PairingOperationCoordinator, commitPairingResult } from "./pairing-operation";
import { LATE_ROLLBACK_FAILURE_MESSAGE, PairingCancellationTracker, cleanupRetryOutcome } from "./pairing-cancellation";
import { launchDropped, launchDroppedNotice, preselectedScopes, repairCommand, repairStatus } from "./pairing-scopes";
import { loadLaunchDropped, saveLaunchDropped } from "./launch-dropped";
import { LaunchSheet, canLaunch } from "./launch-session";
import { pendingPairingStoreFor, type PendingPairing, type PendingRelayRequest } from "./pending-pairing";
import { DEFAULT_PAIRING_SCOPES, PairingRelayError, acknowledgePairing, createPairingRequest, pairingRelayOrigin, pollPairingRequest } from "./pairing-relay";
import { browserSupport, unsupportedBrowserNotice } from "./browser-support";
import { attentionStore, catalog } from "./storage";
import { createTerminalSurface, type TerminalSurface } from "./terminal";
import { TERMINAL_ESCAPE_HINT, TERMINAL_ESCAPE_HINT_ID } from "./terminal/ghostty/surface";
import { firstAttachRetry, machineConnection, sessionConnection } from "./session-connection";
import { toastPlacementInThread, toastTopAboveAction, toastTopClearOfBanner } from "./toast-placement";
import { relativeTimestamp } from "./time";
import { paneActivityLabel, paneShowsOutput } from "./pane-activity";
import { fitPaneActivityCaption, fitPaneActivityCaptions, updatePaneActivityCaption } from "./pane-activity-view";
import { fleetControlGate } from "./fleet-permissions";
import { FleetOpsState, UNDO_WINDOW_MS, requestMergeAction as requestMergeActionFor, type FleetAction, type FleetAgent, type FleetTask } from "./fleet-ops";
import { phoneFleetNotice, agentControls, headerControls, taskControls, undoBar, type FleetOpsViewContext } from "./fleet-ops-view";
import { runFleetOperation } from "./fleet-ops-request";
import { loadPaneLayout, movePane, normalizePaneLayout, orderedPaneIds, promotePane, savePaneLayout, type PaneLayout, type PaneLayoutStorage } from "./pane-layout";
import { detectSpeechInput, focusAfterDictation, SpeechDictationController, type SpeechInputCapability, type SpeechInputState } from "./speech-input";
import { backLabel, clearStoredSelection, forgetMachine, goBackSelection, loadStoredSelection, pairedSessionToOpen, previousSelection, restorableSession, saveStoredSelection, selectionAfterPairing, selectSelection, sessionPickerEntries, sessionPickerHeadline, sessionPickerRowMeta, type SelectionState, type SessionPickerEntry, type SelectionStorage, type SessionSelection } from "./session-selection";
import { composerFocusWinner, planSupervisorSend, sendsOnEnter, supervisorMessage, supervisorTarget } from "./supervisor-message";
import { hiddenWorkersLabel, saveWorkersRevealed, splitVisiblePanes, workersCommandLabel, workersRevealed, workersRoute } from "./worker-visibility";
import { dormantCommandLabel, dormantRevealed, dormantRoute, saveDormantRevealed } from "./dormant-visibility";
import { fleetMachineInitials, machineAccentClass, machineInitials, setMachineAccentFleet, storageAccentStore } from "./machine-accent";
import { COMPACT_MEDIA_QUERY, PHONE_MEDIA_QUERY } from "./viewport";
import { defaultTranscriptView, loadTranscriptView, saveTranscriptView, type TranscriptViewMode } from "./transcript";
import { TranscriptView } from "./transcript-view";
import { applyLiveRegions, sessionControlsNotice, type LiveRegionView } from "./live-regions";
import { DeferredRenderScheduler } from "./deferred-render";
import { FleetBoardRenderer } from "./fleet-board";
import { FirstConnectionAnnouncer, installPairedMachine } from "./first-connection";
import { isEditableElement, renderDecision, shellSignature } from "./render-model";
import { operatorThreadMarkup } from "./operator-thread";
import { applyDraftNote, applyMicState, composerMarkup } from "./composer-markup";
import { countdownLabel, nextCountdown, pairDialogMarkup as renderPairDialogMarkup } from "./pair-dialog-markup";
import type { AttentionItem, ConversationHistoryPage, HubSession, LeaseState, OperatorReply, PaneInfo, Scope, SessionCardSummary, SessionState, StoredMachine } from "./types";

applyScheme();

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
bindKeyboardViewport(window);
// The keyboard coming up or going away resizes the window: the pinned question folds or opens with it (cas-16eed).
window.addEventListener("resize", () => syncComposing());
window.visualViewport?.addEventListener("resize", () => syncComposing());
const machines = new Map<string, StoredMachine>();
let machineCatalogLoaded = false;
const sessions = new Map<string, HubSession[]>();
const catalogExpiresAt = new Map<string, number>();
const catalogExpiryTimers = new Map<string, number>();
const fleetCatalogUpdatedAt = new Map<string, string>();
const connections = new Map<string, HubConnectionSupervisor>();
const connectionStates = new Map<string, ConnectionState>();
const lastLiveAt = new Map<string, number>();
const attachStates = new Map<string, AttachSnapshot>();
/** Sessions whose socket has been live this visit: a later drop is a reconnect, not a first connect. */
const sessionsEverLive = new Set<string>();
/** Connection labels a conversation row shows in place of its last turn (cas-a447). */
// cas-a6f0: an unsteady machine is named on the row too, as the header names it.
const INTERRUPTED_LABELS = new Set(["Reconnecting", "Unreachable", "Needs pairing", UNSTEADY, CANT_REACH_RETRYING]);
const machineInfo = new Map<string, HubMachineInfo | undefined>();
const statuses = new Map<string, Record<string, unknown>>();
/** Sessions whose first status is on its way: the context rail holds its place for them (cas-813a). */
const statusPending = new Set<string>();
const leases = new Map<string, LeaseState>();
const surfaces = new Map<string, TerminalSurface>();
const transcripts = new Map<string, TranscriptView>();
// Pebble 3: ask and blocker render as the fused-tray objects on the Pebble 2 seam.
installAttentionObjects();
const conversationViews = new Map<string, ConversationView>();
// Pebble 4: a supervisor's artifact is a dog-eared sheet on the thread, not a link row.
installAttachmentSheet();
const conversationHistories = new Map<string, ConversationHistory>();
/** Supervisor turns per thread the operator had on screen the last time that thread was open. */
const readReplies = new Map<string, number>();
/** Rows as last rendered, so the compose FAB can pick the thread that most wants the operator. */
let conversationRows: ConversationRow[] = [];
/** The list search's text (journey F8); rows render filtered by it. */
let conversationSearchQuery = "";
const conversationList = new ConversationList();
let hubPresentation: "conversation" | "terminal" = "conversation";
const pendingSubmissions = new Set<string>();
/**
 * The thread for machine:session. With `session`, the thread learns which
 * supervisor session it is attached to, so questions a session that has since
 * ended asked stop waiting (cas-16eed).
 */
function conversationHistory(key: string, session?: string): ConversationHistory {
  let history = conversationHistories.get(key);
  if (!history) {
    history = new ConversationHistory();
    // Questions dismissed on an earlier visit stay dismissed when history replays them.
    for (const id of loadDismissedAsks(dismissedAskStorage(), key)) history.dismissAsk(id);
    // cas-8d52: and every turn keeps the time the last visit showed it.
    const seen = storedArrivals.get(key);
    if (seen && !conversationPersistenceBlocked.has(key.slice(0, key.indexOf(":")))) history.seedArrivals(seen);
    conversationHistories.set(key, history);
  }
  if (session !== undefined) history.currentSession = session;
  return history;
}
function dismissedAskStorage(): DismissedAsksStorage | undefined {
  try { return window.localStorage; } catch { return undefined; }
}
/**
 * `unavailable`: the session attached without durable history (a daemon
 * from before conversation_history), so no first page will ever arrive.
 */
const conversationHistoryPages = new Map<string, { hasEarlier: boolean; nextBefore?: number; loading: boolean; loaded: boolean; requested?: boolean; unavailable?: boolean }>();
function conversationHistoryPage(key: string): { hasEarlier: boolean; nextBefore?: number; loading: boolean; loaded: boolean; requested?: boolean; unavailable?: boolean } {
  let page = conversationHistoryPages.get(key);
  if (!page) {
    page = { hasEarlier: false, loading: false, loaded: false };
    conversationHistoryPages.set(key, page);
  }
  return page;
}
function updateConversationViews(): void { for (const view of conversationViews.values()) view.update(); syncConversationContext(); persistPendingSends(); persistArrivals(); syncEarlyThread(); }
// What the desktop context rail can show beyond the header (P10): the last
// status and attention renders record whether they had anything for the open
// thread; asks, blockers and attachments are read from its history.
let contextProgress = false;
let contextAttention = 0;
function syncConversationContext(): void {
  if (hubPresentation !== "conversation") return;
  // With no thread open there is nothing for the rail to hold: the last
  // thread's progress and attention must not keep it open as an empty column
  // (journey F15, cas-9225).
  if (!selectedMachineId || !selectedSession) {
    contextProgress = false;
    contextAttention = 0;
    syncContextRail(document, { history: undefined, progress: false, attention: 0 });
    return;
  }
  const history = conversationHistories.get(sessionKey(selectedMachineId, selectedSession));
  syncContextRail(document, { history, progress: contextProgress || progressSheetOpen(), attention: contextAttention });
}
let workingRefresh: ReturnType<typeof setTimeout> | undefined;
/** Pane output lights the working line now and schedules the check that puts it out. */
function refreshWorkingLines(): void {
  for (const view of conversationViews.values()) view.refreshWorking();
  if (workingRefresh !== undefined) clearTimeout(workingRefresh);
  workingRefresh = setTimeout(() => { workingRefresh = undefined; for (const view of conversationViews.values()) view.refreshWorking(); }, WORKING_WINDOW_MS + 250);
}
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
/** Pane output this recent keeps the thread's working line lit. */
const WORKING_WINDOW_MS = 30_000;
const authoritativeSessions = new Set<string>();
const paneKeyframesReady = new Set<string>();
const selectedPanes = new Map<string, string>();
const collapsedWorkerPanes = new Set<string>();
const leaseHeartbeats = new Map<string, number>();
const leaseExpiryTimers = new Map<string, number>();
/** How often an open session held by another device is re-checked for release (journey F5). */
const FOREIGN_LEASE_RECHECK_MS = 5_000;
// A live Claude/Ink proof after cas-9a29 decides whether one-row PTYs are safe.
// Until then collapsed phone rows preserve their last real terminal geometry.
const mobileCollapsedPaneGeometry = "freeze";
/**
 * Header action icons, shown in place of the words when the Terminal header's
 * own column is too narrow for them (styles.css, session-header container):
 * the buttons keep their accessible names (cas-3400 QA round 2 F01).
 */
const HEADER_CONTROL_ICON = '<svg class="action-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"><rect x="3" y="6" width="18" height="12" rx="2"/><path d="M7 10h.01M11 10h.01M15 10h.01M7 14h10"/></svg>';
const HEADER_PALETTE_ICON = '<svg class="action-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"><path d="M9 6a3 3 0 1 0-3 3h12a3 3 0 1 0-3-3v12a3 3 0 1 0 3-3H6a3 3 0 1 0 3 3z"/></svg>';
const HEADER_INTERRUPT_ICON = '<svg class="action-icon" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true" focusable="false"><rect x="6" y="6" width="12" height="12" rx="2"/></svg>';
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
// A machine just paired from a phone, held until its hub lists sessions so its
// first live conversation opens instead of the list (journey F8). Any
// selection the operator makes first cancels it.
let openAfterPairing: string | undefined;
let sessionPickerOpen = false;
let pairingStatus = pendingPairing?.kind === "relay-request" ? "Waiting for a machine to claim the code…" : pairingArrivalNotice;
// Cancellation whose durable cleanup did not complete: the dialog stays on a
// "could not finish cancelling" step with a retry until storage cooperates (F2).
let pairingCleanupFailed = false;
// Machines whose credential was just saved: "connected" is announced once per
// saved credential, only when the connection actually reaches live (F8).
const firstConnections = new FirstConnectionAnnouncer();
// Why the cleanup step is showing: what was discarded and which cleanup is
// still owed, so the step says only what is true of this cleanup.
let pairingCleanupContext: CleanupStepContext = { cause: "cancel", storeOpen: false, rollbackPending: false };
// The generation of the exchange Cancel would invalidate, so a late rollback
// result can be matched to the cancellation that caused it.
let exchangeOperationGeneration: number | undefined;
let pairingPollTimer: number | undefined;
let pairingCountdownTimer: number | undefined;
let connectionViewTicker: number | undefined;
let pairingCreateInFlight = false;
let pairingExchangeInFlight = false;
// A `cas hub pair` link names its machine's address and display name; both open
// prefilled (and editable), so only the operator's own name is left to type.
let pairingDraft = createPairingDraft(location.origin, preselectedScopes(pendingPairing), pendingPairing?.kind === "invitation" ? pendingPairing : undefined);
let machineDrawerOpen = false;
let attentionPanelCollapsed = window.matchMedia(PHONE_MEDIA_QUERY).matches;
// Off by default (cas-6261): the Hub lists supervisors only until the operator
// asks for workers through the route or the palette.
const revealWorkers = workersRevealed(location.search, workerVisibilityStorage());
// Off by default: sessions whose supervisor is no longer live are retained
// only for an explicit recovery view.
const revealDormant = dormantRevealed(location.search, workerVisibilityStorage());
let activeContextTab: "attention" | "status" = "attention";
let commandPaletteOpen = false;
/** Whether the open palette was opened by a touch tap (cas-990d). Its filter
 * then took focus under a soft keyboard, so the keyboard's Enter in it must
 * land to read, not in the reply box: that would keep the keyboard up over
 * the conversation. Ctrl/Cmd+K and mouse opens mean a hardware keyboard. */
let commandPaletteOpenedByTouch = false;
/** The control that opened Paired machines, by id, so its close can hand focus back (cas-460a). */
let pairedMachinesOpener: string | undefined;
let speechCapability: SpeechInputCapability | undefined;
let speechDetectionStarted = false;
let speechController: SpeechDictationController | undefined;
let speechInputState: SpeechInputState = "idle";
let speechInputDetail = "";
/** Dictation wrote into the composer during the current listening run (cas-71f4). */
let speechWroteThisRun = false;
let messageDelivery: { session: string; target: string; clientRef: string } | undefined;
/** The refused send whose text Edit put back in the composer (F6): the next
 * composer send in that thread is its edited version and retires it. */
let editingRefused: { threadKey: string; id: string } | undefined;
const operatorReplies = new Map<string, OperatorReply[]>();
/** Sessions this page ended (cas-55a4): they leave the list instead of reading "Unreachable". */
const endedSessions = new Set<string>();
// Why a send did not happen has to survive the render that follows it, and has
// to sit beside the composer: a toast is gone before a phone operator has
// finished reading it, and a disabled button says nothing at all.
/** `transport` marks a refusal caused by the connection itself: it clears when the session is live again (cas-b789). */
// `held`: the line about sends waiting on the connection; its words are
// re-chosen on every render from the state the banner reads (cas-a6f0).
let messageStatus: { session: string | undefined; text: string; tone: "info" | "error"; transport?: boolean; held?: boolean } | undefined;

// An engine cannot gain an API mid-session, so this is probed once. Saying so
// in one line beats a "Connecting…" spinner that can never finish
// (report cas-b652, defect D3).
const browserNotice = unsupportedBrowserNotice(browserSupport());

// One phone definition shared by the stylesheet, layout state, pane mounting
// and pane tapping — see viewport.ts. Rotation must not put the CSS and this
// logic in different modes, which a width-only breakpoint guaranteed it would.
function phoneLayout(): boolean { return window.matchMedia(PHONE_MEDIA_QUERY).matches; }
function keyboardHintOffered(): boolean { return window.matchMedia(KEYBOARD_HINT_MEDIA_QUERY).matches; }


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
  conversationViews.get(key)?.dispose();
  conversationViews.delete(key);
  transcripts.get(key)?.dispose();
  transcripts.delete(key);
  surface.dispose();
  surfaces.delete(key);
}

/**
 * The conversation thread over a pane's mount. It goes up as soon as the pane
 * card exists — before the terminal surface finishes loading beneath it — so
 * opening a conversation never shows the terminal's own dark frame or a bare
 * panel (cas-04ee); the surface keeps it in place when it mounts.
 */
function mountConversation(key: string, mount: HTMLElement): void {
  mount.classList.remove("transcript-active");
  mount.classList.add("conversation-active");
  let conversation = conversationViews.get(key);
  if (!conversation) {
    const threadKey = sessionKey(selectedMachineId!, selectedSession!);
    const threadMachineId = selectedMachineId!, threadSession = selectedSession!;
    const hubSession = sessions.get(selectedMachineId!)?.find((item) => item.name === selectedSession);
    const target = supervisorTarget(hubSession) || "Supervisor";
    const history = conversationHistory(threadKey, threadSession);
    conversation = new ConversationView(document, history, {
      supervisor: target,
      machine: machines.get(selectedMachineId!)?.label,
      project: projectTitle(hubSession?.project_dir),
      header: false,
      // The supervisor is executing while a send it received awaits its
      // reply on a reachable machine, or the pane produced output in the last
      // half minute (cas-5a8f: never on a held send or an unreachable machine).
      working: () => supervisorWorking(
        history,
        connectionStates.get(threadMachineId),
        [...paneLastActivity].some(([paneId, at]) => paneId.startsWith(`${threadKey}:`) && Date.now() - at < WORKING_WINDOW_MS),
      ),
      editMessage: (text, send) => {
        const composer = document.querySelector<HTMLTextAreaElement>("#message-text");
        if (!composer || composer.dataset.threadKey !== threadKey) return;
        if (composer.value.trim()) { showComposerStatus("Your draft already has text. Clear it before editing the refused message.", "info"); composer.focus(); return; }
        composer.value = text; composer.dispatchEvent(new Event("input")); composer.focus();
        editingRefused = { threadKey, id: send.id };
      },
      // A quick-reply chip answers the ask through the same leased path as
      // the composer, with in_reply_to = the ask's notification id.
      respond: (ask, text) => { void submitSupervisorMessage({ text, replyTo: ask.notification_id }); },
      // Retry sends the refused text again through the same leased path,
      // keeping its original in_reply_to; the refused bubble leaves the
      // thread only once the new send is actually on the wire.
      retryMessage: (send) => { void submitSupervisorMessage({ text: send.text, replyTo: send.replyTo, retryOf: send.id }); },
      // The refusal says "Take control, then retry"; the control is on the
      // refused message because the conversation header has none (cas-3433).
      takeControl: () => { void takeControlForRefused(threadMachineId, threadSession); },
      controlHeld: () => controlTakenAfterRefusal.has(threadKey) && leases.get(threadKey)?.held_by_me === true,
      // The header's own state: a send that expired waiting for the session
      // says Retry will go through once this reads live (cas-d15c).
      sessionLive: () => conversationConnection(threadMachineId, threadSession)?.phase === "live",
      // cas-1730 (cas-008f N01): while another device holds control and this
      // one cannot force a takeover, the refused message names that device
      // and says to take control once it is released, as the composer does.
      controlHolder: () => {
        const lease = leases.get(threadKey);
        const machine = machines.get(threadMachineId);
        return lease && !lease.held_by_me && lease.controller_label && !machine?.scopes.includes("hub-admin")
          ? lease.controller_label
          : undefined;
      },
      // cas-16eed: a failed send swiped away takes the composer's pointer at
      // it along, the list preview stops saying "Not sent", and a dismissed
      // question stays dismissed across a reload.
      dismissalsChanged: () => {
        saveDismissedAsks(dismissedAskStorage(), threadKey, history.dismissedAskIds());
        if (messageStatus?.session === threadKey && messageStatus.text === REFUSED_SEE_ABOVE && !history.visibleEvents().some((event) => event.kind === "send" && history.isFailedSend(event.value))) clearComposerStatus();
        renderConversationList();
        syncConversationContext();
      },
      // cas-55a4: a session that has not written to Commander shows what it
      // is doing (its newest queue row, else its panes' last output) and a
      // way into its Terminal view, instead of another session's thread.
      activity: () => sessionActivity(threadMachineId, threadSession),
      openTerminal: () => { document.querySelector<HTMLButtonElement>("#conversation-terminal")?.click(); },
      // cas-010f: the empty thread reads the header's own connection words.
      connection: () => conversationHeaderLabel(threadMachineId, threadSession),
      hasEarlier: () => conversationHistoryPage(threadKey).hasEarlier,
      loadingEarlier: () => conversationHistoryPage(threadKey).loading,
      // cas-010f: until this session's first page resolves, "no messages"
      // would be a guess, before the request goes out as much as after; a
      // session whose daemon keeps no history resolves on attach.
      loadingHistory: () => {
        const page = conversationHistoryPage(threadKey);
        return !page.loaded && !page.unavailable;
      },
      openingSince: () => conversationOpenedAt.get(threadKey),
      historyEnd: () => {
        const page = conversationHistoryPage(threadKey);
        return page.loaded && !page.hasEarlier;
      },
      loadEarlier: () => {
        const page = conversationHistoryPage(threadKey);
        if (page.loading || page.nextBefore === undefined) return;
        page.loading = true;
        updateConversationViews();
        const sent = connections.get(selectedMachineId!)?.requestConversationHistory(selectedSession!, page.nextBefore);
        if (!sent) {
          page.loading = false;
          updateConversationViews();
        }
      },
    });
    conversationViews.set(key, conversation);
  }
  if (conversation.element.parentElement !== mount) mount.append(conversation.element);
  // The unanswered ask is pinned directly above the composer as well as in the flow.
  const composerSlot = document.querySelector<HTMLElement>("#conversation-composer-slot");
  if (composerSlot && conversation.pinned.parentElement !== composerSlot) composerSlot.prepend(conversation.pinned);
  // "1 unsent message" sits above the pinned question: the way back to a dismissed failed send (cas-16eed).
  if (composerSlot && conversation.unsent.parentElement !== composerSlot) composerSlot.prepend(conversation.unsent);
  // "Jump to latest" gets its own row above the pinned card and composer, so
  // it never floats over a turn in the thread (cas-97ea).
  if (composerSlot && conversation.jump.parentElement !== composerSlot) composerSlot.prepend(conversation.jump);
  conversation.update();
}

/** The view key of a thread shown before its session's panes (cas-fc2c). */
const EARLY_THREAD = "early-thread";
const earlyThreadKey = (threadKey: string): string => `${threadKey}:${EARLY_THREAD}`;

/**
 * cas-fc2c: while the selected conversation is still opening or reconnecting,
 * a thread that already holds something to show (the messages this browser
 * kept across a reload) goes up on its own above the connecting card,
 * instead of waiting for the session's panes. Once the panes are up, or there
 * is nothing to show, it goes away and the pane's thread takes over.
 */
function syncEarlyThread(): void {
  const grid = document.querySelector<HTMLElement>("#pane-grid");
  const threadKey = selectedMachineId && selectedSession ? sessionKey(selectedMachineId, selectedSession) : undefined;
  const wanted = Boolean(grid && threadKey && grid.dataset.sessionKey === threadKey)
    && threadBeforePanes({ presentation: hubPresentation, placeholder: gridPlaceholder(grid!), history: conversationHistories.get(threadKey!) });
  for (const [key, view] of [...conversationViews]) {
    if (!key.endsWith(`:${EARLY_THREAD}`) || (wanted && key === earlyThreadKey(threadKey!))) continue;
    view.dispose();
    conversationViews.delete(key);
  }
  const existing = grid?.querySelector<HTMLElement>(":scope > .conversation-early") ?? undefined;
  if (!wanted) { existing?.remove(); grid?.classList.remove("has-early-thread"); return; }
  let mount = existing;
  if (!mount) {
    mount = document.createElement("div");
    mount.className = "terminal-mount conversation-early";
    grid!.prepend(mount);
    grid!.classList.add("has-early-thread");
  }
  mountConversation(earlyThreadKey(threadKey!), mount);
}

function applyPaneView(key: string, mount: HTMLElement, surface: TerminalSurface, view: TranscriptViewMode): void {
  surface.setMinimumColumns(compactViewport() ? COMPACT_MINIMUM_COLUMNS : 0);
  if (hubPresentation === "conversation") {
    transcripts.get(key)?.dispose(); transcripts.delete(key);
    surface.setCanvasPainting(false);
    mountConversation(key, mount);
    return;
  }
  mount.classList.remove("conversation-active");
  conversationViews.get(key)?.dispose(); conversationViews.delete(key);
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

function workerVisibilityStorage(): SelectionStorage | undefined {
  try { return window.localStorage; } catch { return undefined; }
}

/** Flip worker visibility. The stream gate is negotiated at attach, so the
 *  Hub reloads on the matching route rather than re-attaching every session. */
function setWorkersRevealed(next: boolean): void {
  saveWorkersRevealed(workerVisibilityStorage(), next);
  const route = `${location.pathname}${workersRoute(location.search, next)}${location.hash}`;
  location.assign(route);
}

function setDormantRevealed(next: boolean): void {
  saveDormantRevealed(workerVisibilityStorage(), next);
  const route = `${location.pathname}${dormantRoute(location.search, next)}${location.hash}`;
  location.assign(route);
}

function visibleSessions(machineId: string): HubSession[] {
  return visibleCatalog(sessions.get(machineId) ?? [], name => conversationHistories.get(sessionKey(machineId, name))?.hasPending() ?? false,
    Date.now() < (catalogExpiresAt.get(machineId) ?? Infinity), revealDormant);
}

/**
 * The sessions every surface lists, per machine: the conversation list, the
 * palette's Jump rows, the session picker and its "N available" count all
 * read this, so they cannot disagree on which sessions exist (cas-645e).
 */
function visibleSessionMap(): Map<string, HubSession[]> {
  return new Map([...machines.keys()].map((machineId) => [machineId, visibleSessions(machineId)]));
}

function selectionStorage(): SelectionStorage | undefined {
  try { return window.localStorage; } catch { return undefined; }
}

function applySelection(next: SessionSelection | undefined): void {
  selectedMachineId = next?.machineId;
  selectedSession = next?.session;
  syncFleetSelection();
}

/**
 * Every deliberate move — a machine, a session, an attention jump — goes
 * through here, so the back control and the restored-on-reopen session are
 * always describing the same trail.
 */
function commitSelection(next: SessionSelection): void {
  restoreTarget = undefined;
  openAfterPairing = undefined;
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

/**
 * A pane header's time is the pane's last output seen by this page. With
 * none yet it says so, rather than "No activity", which contradicted the
 * session's own last activity on the empty thread (cas-010f), and a pane that
 * opened on earlier output says that instead of "No output yet" above it
 * (journey F42).
 */
const bufferShowsOutput = new WeakMap<readonly number[], boolean>();
function paneBufferShowsOutput(key: string): boolean {
  const buffer = paneBuffers.get(key);
  if (!buffer) return false;
  let shows = bufferShowsOutput.get(buffer);
  if (shows === undefined) { shows = paneShowsOutput(buffer); bufferShowsOutput.set(buffer, shows); }
  return shows;
}

function updatePaneActivity(element: HTMLElement, key: string): void {
  const label = paneActivityLabel(paneLastActivity.get(key), paneBufferShowsOutput(key));
  updatePaneActivityCaption(element, label);
}

let paneActivityFitFrame: number | undefined;
function queuePaneActivityFit(): void {
  if (paneActivityFitFrame !== undefined) return;
  paneActivityFitFrame = window.requestAnimationFrame(() => {
    paneActivityFitFrame = undefined;
    fitPaneActivityCaptions(document);
  });
}
window.addEventListener("resize", queuePaneActivityFit, { passive: true });
window.visualViewport?.addEventListener("resize", queuePaneActivityFit, { passive: true });
void document.fonts?.ready.then(queuePaneActivityFit);
document.fonts?.addEventListener("loadingdone", queuePaneActivityFit);

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

/** Re-pairing is about a named machine; say so instead of a blank create prompt. */
function openRepairDialog(machineId: string): void {
  const machine = machines.get(machineId);
  const label = machine?.label ?? "this machine";
  if (!pendingPairing && !pairingCleanupFailed) {
    // cas-0e14 F29: a code re-pair can't keep starting sessions; say so first.
    pairingStatus = repairStatus(label, machine?.scopes ?? []);
    // cas-093d F02: the command that keeps it, as its own copyable code.
    const command = repairCommand(machine?.scopes ?? [], location.origin);
    pairingRepairCommand = command ? { status: pairingStatus, command } : undefined;
    render(false);
  }
  openPairDialog();
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
  // Keep what the operator typed into the form that is still on screen, then
  // replace the machine, address and ceiling with the new link's. The render
  // below must not capture the form again: the old link's address and machine
  // name are still in it and would overwrite the new prefill, sending the new
  // token to the old machine (QA F01).
  capturePairingDraft();
  pendingPairing = fragment;
  pairingDraft = {
    ...createPairingDraft(location.origin, preselectedScopes(fragment), fragment),
    deviceLabel: pairingDraft.deviceLabel,
    operatorLabel: pairingDraft.operatorLabel,
    email: pairingDraft.email,
  };
  pairingStatus = "";
  render(false);
  openPairDialog();
}, () => {
  if (pendingPairing) return;
  pairingStatus = INVALID_PAIRING_LINK_MESSAGE;
  render();
  openPairDialog();
});

function createConnection(machine: StoredMachine): HubConnectionSupervisor {
  restoreStoredSends(machine);
  return new HubConnectionSupervisor(machine, {
    onState: (state) => {
      const wasLive = connectionStates.get(machine.id)?.phase === "live";
      connectionStates.set(machine.id, state);
      // cas-0e14 F30: an open New session follows the machine's connection.
      launchSheet.refresh();
      // Anchor staleness to the last live moment: retry transitions rewrite
      // snapshot.since, which would report a ten-minute outage as "just now".
      if (state.phase === "live") lastLiveAt.set(machine.id, Date.now());
      // cas-c808 QA F01: a card that said the machine couldn't be reached
      // must not keep saying so once it is back.
      if (state.phase === "live" && !wasLive) clearTransientAttachmentNotes(document, machine.id);
      // Journey F28: a card that said the machine is connected says what the
      // header says once it is not.
      else restateAttachmentNotes(document, machine.id);
      const connectedNotice = firstConnections.observe(machine.id, machine.label, state);
      if (connectedNotice) toast(connectedNotice);
      if (state.phase === "failed" || state.phase === "backoff") invalidateMachineLeases(machine.id);
      // cas-a6f0 (journey F35): a refused pairing will not come back by
      // itself, so nothing may keep saying it is sending.
      if (state.phase === "failed" && state.authFailure) settleSendsForPairingLoss(machine);
      // One outage is one problem. A stable fingerprint per machine and kind
      // collapses every retry into a single card with a repeat count instead of
      // burying the feed under a card for each attempt.
      // cas-7752: a revoked pairing takes the operator's stored words with it.
      if (state.authFailure === "revoked") purgeMachineConversations(machine.id, { forgetInMemory: false });
      if (state.authFailure) {
        // cas-b452 (journey F37): a refused pairing can't send a fresh catalog,
        // so its last one must not expire into an empty list while its
        // conversation stays open beside it. The rows stay, reading "Needs
        // pairing", until the next catalog after Re-pair replaces them.
        window.clearTimeout(catalogExpiryTimers.get(machine.id));
        catalogExpiryTimers.delete(machine.id);
        catalogExpiresAt.delete(machine.id);
        // The hub answered, so it is not "Reconnecting to hub" any more; the
        // pairing card below says what is wrong (cas-d15c QA F01).
        resolveAttention(`${machine.id}:hub_disconnected`);
        // cas-7b31: control does not come back by itself now, and a toast
        // still saying the connection dropped would contradict the pairing
        // card, on screen or to a screen reader.
        for (const key of [...controlLostToOutage]) if (key.startsWith(`${machine.id}:`)) controlLostToOutage.delete(key);
        const shown = document.querySelector<HTMLElement>("#toast");
        if (shown?.textContent === CONTROL_DROPPED_TOAST) shown.textContent = CONTROL_PAIRING_TOAST;
        // cas-a6f0 (journey F8): one name for a pairing the hub refused,
        // revoked or otherwise, as the header's "Needs pairing" and the
        // banner's "needs pairing again" say.
        void addAttention(machine, undefined, "auth_loss", {
          headline: "Machine needs pairing",
          detail: state.reason ?? pairingLostBanner(machine.label),
          severity: "critical",
          action: "repair",
          fingerprint: `${machine.id}:auth_loss`,
        });
      }
      if (state.phase === "live") {
        resolveAttention(`${machine.id}:hub_disconnected`);
        // cas-d636 QA F02: a pairing that works again is no longer blocked.
        resolveAttention(`${machine.id}:auth_loss`);
      }
      // cas-7b31 (journey F1): a drop that retries is told by the banner,
      // header, row and footer in plain words; a rail card beside them said
      // it again in transport terms. Only a failure that will not retry
      // earns a card, worded as the banner words it.
      if (state.phase === "failed" && state.fatal === true && !state.authFailure) {
        // cas-be76: the card names the machine; the transport reason (raw
        // host, stage) stays behind Details.
        // A session alarm for this machine is the same stopped connection.
        for (const item of attention) {
          if (item.machineId === machine.id && item.kind === "session_transport" && item.fingerprint) resolveAttention(item.fingerprint);
        }
        void addAttention(machine, undefined, "hub_disconnected", {
          headline: `Lost connection to ${machine.label}`,
          detail: fatalConnectionRecovery(state.reason),
          severity: "warning",
          action: "none",
          payload: { reason: state.reason, stage: state.stage },
          fingerprint: `${machine.id}:hub_disconnected`,
        });
      }
      render();
    },
    onAttachState: (session, state) => {
      const key = sessionKey(machine.id, session);
      const attachWasLive = attachStates.get(key)?.phase === "live";
      attachStates.set(key, state);
      if (state.phase === "live") {
        sessionsEverLive.add(key);
        clearTransportStatus(key);
        void reclaimControlThenFlush(machine, session);
        if (!attachWasLive) clearTransientAttachmentNotes(document, machine.id);
        // The socket is back: its transport alarm is history, not attention.
        resolveAttention(`${machine.id}:${session}:session_transport`);
      }
      else restateAttachmentNotes(document, machine.id);
      if (selectedMachineId === machine.id && selectedSession === session) render();
      // The list row and the footer read the session's connection too.
      else renderConversationList();
    },
    onAuthFailure: (kind, detail) => {
      if (kind === "expired") return;
      pairingStatus = `${detail}. Re-pair in Cassy Cloud; no browser reset is required.`;
      render();
    },
    onCredentialRefreshed: async (refreshed) => { machines.set(refreshed.id, refreshed); await catalog.put(refreshed); },
    onMachineInfo: (info) => { machineInfo.set(machine.id, info); render(); },
    onSessions: (items, freshnessThresholdSecs) => {
      fleetCatalogUpdatedAt.set(machine.id, new Date().toISOString());
      const ttl = freshnessThresholdSecs !== undefined && Number.isFinite(freshnessThresholdSecs) && freshnessThresholdSecs > 0 ? freshnessThresholdSecs * 1000 : Infinity;
      catalogExpiresAt.set(machine.id, Date.now() + ttl);
      window.clearTimeout(catalogExpiryTimers.get(machine.id));
      if (Number.isFinite(ttl)) catalogExpiryTimers.set(machine.id, window.setTimeout(() => render(), ttl));
      retireEndedSessionNotices(machine.id, items);
      // A session the operator ended is gone, not unreachable: it is never retained (cas-55a4).
      sessions.set(machine.id, retainPendingSessions(sessions.get(machine.id) ?? [], items, name => !endedSessions.has(sessionKey(machine.id, name)) && (conversationHistories.get(sessionKey(machine.id, name))?.hasPending() ?? false)));
      if (!openPairedSession(machine.id, visibleSessions(machine.id))) restoreLastSession(machine.id, visibleSessions(machine.id));
      render();
    },
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
      // Journey F42: the header follows what the pane now shows.
      const activity = document.querySelector<HTMLElement>(`[data-pane-id="${CSS.escape(pane)}"] .pane-last-activity`);
      if (activity && selectedMachineId === machine.id && selectedSession === session) updatePaneActivity(activity, key);
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
      refreshWorkingLines();
      if (authoritativeSessions.has(sessionKey(machine.id, session)) && !paneKeyframesReady.has(key)) return;
      const buffered = [...(paneBuffers.get(key) ?? []), ...data];
      paneBuffers.set(key, buffered.slice(-2_000_000));
      surfaces.get(key)?.write(data);
      const activity = document.querySelector<HTMLElement>(`[data-pane-id="${CSS.escape(pane)}"] .pane-last-activity`);
      if (activity && selectedMachineId === machine.id && selectedSession === session) updatePaneActivity(activity, key);
    },
    onMessageQueued: (session, receipt) => {
      conversationHistory(sessionKey(machine.id, session)).acknowledge(receipt);
      if (messageDelivery?.session === sessionKey(machine.id, session) && messageDelivery.clientRef === receipt.client_ref) { messageDelivery = undefined; document.querySelector<HTMLElement>("#message-delivery")?.setAttribute("hidden", ""); }
      updateConversationViews(); renderConversationList();
    },
    onOperatorMessage: (session, message) => {
      conversationHistory(sessionKey(machine.id, session), session).hydrateSend(message);
      updateConversationViews(); renderConversationList();
      if (selectedMachineId === machine.id && selectedSession === session) render();
    },
    onMessageRejected: (session, clientRef, detail, rejection) => {
      const key = sessionKey(machine.id, session);
      // cas-0653: the hub could not reach the session's daemon, so the
      // message never arrived there. It waits in the thread and goes out once
      // on the next live attach (the connection reattaches for it), instead
      // of reading "Not sent".
      if (rejection?.retryable && reholdRefusedSend(machine, session, clientRef)) {
        if (messageDelivery?.session === key && messageDelivery.clientRef === clientRef) {
          messageDelivery = undefined;
          document.querySelector<HTMLElement>("#message-delivery")?.setAttribute("hidden", "");
        }
        if (selectedMachineId === machine.id && selectedSession === session && heldSends.get(key)?.some((held) => held.clientRef === clientRef)) {
          // Worded as the banner words this outage (cas-d15c, cas-a6f0).
          showHeldSendStatus(machine.id, session);
        }
        updateConversationViews(); renderConversationList();
        return;
      }
      // A control refusal proves the cached lease is stale: control counts as
      // held again only after a take succeeds (cas-8e0a). Other refusals say
      // nothing about the lease and leave it alone.
      if (refusal(detail).action === "take-control") controlTakenAfterRefusal.delete(key);
      const onBubble = conversationHistory(key).reject(clientRef, detail);
      if (messageDelivery?.session === key && messageDelivery.clientRef === clientRef) {
        messageDelivery = undefined;
        document.querySelector<HTMLElement>("#message-delivery")?.setAttribute("hidden", "");
        if (selectedMachineId === machine.id && selectedSession === session) {
          // The refused bubble carries the reason and the next step; the
          // composer only points at it, so the reason is said once (cas-4d92).
          // Without a bubble to point at, the composer gives the whole sentence.
          showComposerStatus(onBubble ? REFUSED_SEE_ABOVE : refusalSentence(detail), "error");
        }
      }
      updateConversationViews(); renderConversationList();
    },
    onOperatorReply: (session, reply) => {
      // cas-e829: a system notice goes to the attention lane, never the thread.
      if (isOperatorNotice(reply)) { applyOperatorNotice(machine, session, reply); return; }
      conversationHistory(sessionKey(machine.id, session), session).receive(reply, Date.now(), session);
      // A later supervisor turn shortens an unreceipted send's wait (cas-1622).
      scheduleReceiptCheck(sessionKey(machine.id, session));
      updateConversationViews(); renderConversationList();
      const key = sessionKey(machine.id, session);
      const replies = operatorReplies.get(key) ?? [];
      if (!replies.some((item) => item.notification_id === reply.notification_id)) {
        replies.push(reply);
        operatorReplies.set(key, replies.slice(-20));
      }
      if (selectedMachineId === machine.id && selectedSession === session) render();
    },
    onConversationHistoryRequested: (session) => {
      const cursor = conversationHistoryPage(sessionKey(machine.id, session));
      if (cursor.loaded) return;
      // The first page, not an earlier one: the thread shows its own loading
      // line, and the "Load earlier" control stays out of it.
      cursor.requested = true;
      updateConversationViews();
    },
    onConversationHistoryUnavailable: (session) => {
      const cursor = conversationHistoryPage(sessionKey(machine.id, session));
      if (cursor.loaded || cursor.unavailable) return;
      cursor.unavailable = true;
      updateConversationViews();
    },
    onOperatorNoticeResolved: (session, resolved) => {
      resolveAttention(noticeFingerprint(machine.id, session, resolved.notification_id, resolved.subject));
    },
    onConversationHistory: (session, page: ConversationHistoryPage) => {
      const key = sessionKey(machine.id, session);
      const cursor = conversationHistoryPage(key);
      applyHistoryCursor(cursor, page);
      const history = conversationHistory(key, session);
      // cas-55a4: the thread is this session's own turns. Other sessions'
      // turns (the page's earlier_* section, or a project-wide page from an
      // older daemon) are filed beside it by session, never into it.
      for (const message of [...page.messages, ...(page.earlier_messages ?? [])]) history.hydrateSend(message);
      const replies = operatorReplies.get(key) ?? [];
      for (const reply of [...page.replies, ...(page.earlier_replies ?? [])]) {
        // cas-e829: this session's notices raise or retire attention; another
        // session's are its own business.
        if (isOperatorNotice(reply)) {
          if (reply.session === undefined || reply.session === session) applyOperatorNotice(machine, session, reply);
          continue;
        }
        history.hydrateReply(reply);
        const own = reply.session === undefined || reply.session === session;
        if (own && !replies.some((item) => item.notification_id === reply.notification_id)) replies.push(reply);
      }
      replies.sort((a, b) => a.notification_id - b.notification_id);
      operatorReplies.set(key, replies.slice(-100));
      updateConversationViews();
      renderConversationList();
      if (selectedMachineId === machine.id && selectedSession === session) render();
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
      // A retrying drop is already one plain status on the banner, header, row
      // and footer; the rail defers to it instead of repeating it (cas-90d4).
      if (!transportFailureNeedsAttention(attachStates.get(sessionKey(machine.id, session)), connectionStates.get(machine.id))) return;
      void addAttention(machine, session, "session_transport", { headline: `Lost connection to ${machine.label}`, detail: `Not retrying: ${detail}`, severity: "critical", action: "none", payload: detail, fingerprint: `${machine.id}:${session}:session_transport` });
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

/**
 * Pairing from a phone lands in the new machine's conversation (journey F8):
 * the first session list from its hub settles the choice once, whether or not
 * a live session is on it, so a session that starts later never pulls the
 * operator out of wherever they went next. Returns whether one was opened.
 */
function openPairedSession(machineId: string, items: readonly HubSession[]): boolean {
  if (openAfterPairing !== machineId) return false;
  openAfterPairing = undefined;
  const session = pairedSessionToOpen(items);
  if (!session || selectedMachineId !== machineId || selectedSession !== undefined) return false;
  void openSession(machineId, session);
  return true;
}

/**
 * New session (cas-0f51): offered once any paired machine grants
 * session-launch; with machines paired but none granting it, the control
 * becomes the way to allow it.
 */
function launchAvailability(): "ready" | "grant" | undefined {
  if (!machines.size) return undefined;
  return [...machines.values()].some(canLaunch) ? "ready" : "grant";
}

function launchConnection(machineId: string): HubConnectionSupervisor {
  const machine = machines.get(machineId);
  if (!machine) throw new Error("That machine is no longer paired with this browser.");
  return ensureConnection(machine);
}

const launchSheet = new LaunchSheet({
  machines: () => [...machines.values()].map((machine) => ({
    id: machine.id, label: machine.label, scopes: machine.scopes, defaultCli: machineInfo.get(machine.id)?.default_supervisor_cli,
    // cas-0e14 F30: the header's words for the machine's connection.
    connection: fleetConnectionLabel(connectionStates.get(machine.id), machine.id),
    launchDropped: launchDroppedMachines.has(machine.id),
  })),
  currentMachineId: () => selectedMachineId,
  origin: location.origin,
  projects: (machineId, signal) => launchConnection(machineId).projects(signal),
  profiles: (machineId, signal) => launchConnection(machineId).launchProfiles(signal),
  browse: (machineId, root, path, signal) => launchConnection(machineId).browseProjects(root, path, signal),
  launch: (machineId, request) => launchConnection(machineId).launchSession(request),
  grant: async (machineId) => {
    await launchConnection(machineId).enableSessionLaunch();
    // cas-0e14 F29: allowed again, so the re-pair notice is done.
    settleLaunchDropped(machineId);
  },
  sessionListed: async (machineId, session) => (await launchConnection(machineId).refreshSessions()).some((item) => item.name === session),
  open: (machineId, session) => {
    // Land on the new session's supervisor, as a palette jump does.
    focusJumpedComposer(openSession(machineId, session));
  },
  copy: (text) => navigator.clipboard.writeText(text),
  returnFocus: () => document.querySelector<HTMLButtonElement>("#new-session-toggle")?.focus(),
});

function ensureConnection(machine: StoredMachine): HubConnectionSupervisor {
  return ensureMachineConnection(machine, connections, createConnection);
}

async function addAttention(machine: StoredMachine, session: string | undefined, kind: string, content: string | AttentionContent, at?: string): Promise<void> {
  // An item about something that happened earlier (a replayed notice) keeps
  // that time, not the moment this browser heard of it (cas-5c22).
  const createdAt = at ?? new Date().toISOString();
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

/** Raise, keep or retire the attention item for one system notice (cas-e829). */
function applyOperatorNotice(machine: StoredMachine, session: string, reply: OperatorReply & { at?: string }): void {
  const plan = planNotice(machine.id, session, reply, (fingerprint) => attention.some((item) => item.fingerprint === fingerprint));
  if (plan.action === "resolve") resolveAttention(plan.fingerprint);
  else if (plan.action === "raise") void addAttention(machine, session, NOTICE_KIND, plan.content, noticeTime(reply.at));
  else void retimeNotice(plan.fingerprint, noticeTime(reply.at));
}

/**
 * A notice that arrived live (no stamp) and is then replayed by history with
 * its row's own time takes that earlier time (cas-5c22).
 */
async function retimeNotice(fingerprint: string, at: string | undefined): Promise<void> {
  if (at === undefined) return;
  const changed = attention.filter((item) => item.fingerprint === fingerprint && !item.acknowledgedAt && at < item.createdAt);
  if (!changed.length) return;
  attention = attention.map((item) => changed.includes(item) ? { ...item, createdAt: at, ...(item.firstSeenAt === undefined || at < item.firstSeenAt ? { firstSeenAt: at } : {}) } : item);
  for (const item of attention.filter((candidate) => changed.some((old) => old.id === candidate.id))) await attentionStore.put(item);
  render();
}

/** A session that left the catalog takes its open notices with it (cas-e829). */
function retireEndedSessionNotices(machineId: string, listed: readonly HubSession[]): void {
  const names = new Set(listed.map((item) => item.name));
  const open = attention.filter((item) => !item.acknowledgedAt && item.machineId === machineId && item.fingerprint?.startsWith(`notice:${machineId}:`) && item.session !== undefined && !names.has(item.session));
  if (open.length) void acknowledgeAttentionGroup(open);
}

/**
 * A connection alarm is about a connection; once that connection is live again
 * the alarm resolves itself instead of waiting for a hand dismissal (cas-a447).
 */
function resolveAttention(fingerprint: string): void {
  const open = attention.filter((item) => !item.acknowledgedAt && item.fingerprint === fingerprint);
  if (open.length) void acknowledgeAttentionGroup(open);
}

/** The one connection state a conversation's header, row and footer show (cas-a447). */
function conversationConnection(machineId: string, session: string | undefined): ConnectionState | undefined {
  const machine = connectionStates.get(machineId);
  if (!session) return machine;
  const key = sessionKey(machineId, session);
  return sessionConnection(machine, attachStates.get(key), sessionsEverLive.has(key));
}

function machineFooterConnection(machineId: string): ConnectionState | undefined {
  const prefix = `${machineId}:`;
  const attached = [...attachStates].filter(([key]) => key.startsWith(prefix)).map(([key, attach]) => ({ attach, wasLive: sessionsEverLive.has(key) }));
  return machineConnection(connectionStates.get(machineId), attached);
}

/**
 * A file card's connection note in the footer's terms (journey F28): Live,
 * a drop that is retrying, or neither.
 */
function artifactMachineReach(machineId: string): ArtifactMachineReach {
  const state = machineFooterConnection(machineId);
  if (state?.phase === "live") return "live";
  const label = machineConnectionLabel(state, lastLiveAt.has(machineId));
  return label === "Reconnecting" ? "reconnecting" : "unreachable";
}

/**
 * Machines whose code re-pair dropped session launch (cas-0e14 F29), kept
 * across a reload (cas-093d F01).
 */
const launchDroppedStorage = (() => { try { return window.localStorage; } catch { return undefined; } })();
const launchDroppedMachines = loadLaunchDropped(launchDroppedStorage);

function announceLaunchDropped(machine: StoredMachine): void {
  launchDroppedMachines.add(machine.id);
  saveLaunchDropped(launchDroppedStorage, launchDroppedMachines);
  const notice = launchDroppedNotice(machine.label);
  void addAttention(machine, undefined, "launch_dropped", { ...notice, severity: "warning", action: "none", fingerprint: `${machine.id}:launch_dropped` });
}

function settleLaunchDropped(machineId: string): void {
  const dropped = launchDroppedMachines.delete(machineId);
  if (dropped) saveLaunchDropped(launchDroppedStorage, launchDroppedMachines);
  if (!dropped && !attention.some((item) => item.fingerprint === `${machineId}:launch_dropped`)) return;
  resolveAttention(`${machineId}:launch_dropped`);
  launchSheet.refresh();
}

async function acknowledgeAttentionGroup(items: AttentionItem[]): Promise<void> {
  const acknowledgedAt = new Date().toISOString();
  const pending = items.filter((item) => !item.acknowledgedAt);
  for (const item of pending) item.acknowledgedAt = acknowledgedAt;
  await Promise.all(pending.map((item) => attentionStore.put(item)));
  render();
}

/** Resolves with the installed machine, or false when nothing was installed. */
async function pairMachine(form: HTMLFormElement): Promise<StoredMachine | false> {
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
          pairingCleanupContext = { cause: "cancel", storeOpen: !cleared.failClosed, rollbackPending: true };
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
      // The exchange itself failed and its rollback rejected: nobody pressed
      // Cancel, so the recovery step needs an owner of its own before Retry
      // cleanup can do anything (review 25564).
      pairingCancellations.begin(operation.generation);
      pairingCleanupContext = { cause: "failure", storeOpen: !cleared.failClosed, rollbackPending: true };
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
      // Keep the pending capability for a bounded retry. A fetch rejection may
      // follow server-side consumption, while a typed throttle happened before
      // this attempt consumed anything; the error copy preserves that uncertainty.
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
  const previousScopes = machines.get(machine.id)?.scopes;
  machines.set(machine.id, machine);
  // cas-0e14 F29: a code re-pair can't carry session launch. Say plainly that
  // starting sessions needs allowing again, rather than letting New session
  // quietly open the permission view again (its label stays "New session", cas-865c).
  if (launchDropped(previousScopes, machine.scopes)) announceLaunchDropped(machine);
  else if (machine.scopes.includes("session-launch")) settleLaunchDropped(machine.id);
  // cas-7752: only a fresh pairing lets a revoked machine's drafts be stored
  // again. A "live" report is not enough: the machine stream can come back
  // while every authenticated request is still refused.
  conversationPersistenceBlocked.delete(machine.id);
  // cas-b452 (journey F39): a re-pair started from an open conversation
  // returns to that conversation, not to the landing page. A first pairing
  // still lands on the new machine.
  const landing = selectionAfterPairing(machine.id, previousScopes !== undefined, selectedMachineId === undefined ? undefined : { machineId: selectedMachineId, session: selectedSession });
  const returnTo = landing.session === undefined ? undefined : { machineId: landing.machineId, session: landing.session };
  commitSelection(landing);
  // A phone shows the list or one conversation, never both: with only the
  // machine selected it stays on the list, so its first live session opens
  // when the connection installed below first lists sessions (journey F8).
  if (phoneLayout() && !returnTo) openAfterPairing = machine.id;
  // The installation seam: "Access saved" and the armed first-connection
  // announcement both precede the connection, so a hub that reports healthy
  // live synchronously still yields saved → connected, named from the machine
  // that was installed (reviews 25642, 25649).
  installPairedMachine(machine, {
    announcer: firstConnections,
    notify: toast,
    startConnection: (installed) => { replaceMachineConnection(installed, connections, connectionStates, createConnection); },
  });
  render(false);
  // The conversation re-attaches over the new pairing's connection.
  if (returnTo?.machineId === machine.id) void attachSelectedSession(machine.id, returnTo.session);
  return machine;
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
          // The heading already says "Machine authorized"; the status only names the next step (cas-b2e4 F01).
          pairingStatus = "Add your name, then press Pair.";
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
  pairingCleanupContext = { cause: "cancel", storeOpen: !cleared.failClosed, rollbackPending: false };
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
  pairingCleanupContext = {
    cause: pairingCleanupContext.cause,
    storeOpen: !cleared.failClosed,
    rollbackPending: recovery.failed === true || (recovery.pendingCleanup ?? 0) > 0,
  };
  render(false);
}

function stopPairingTimers(): void {
  if (pairingPollTimer !== undefined) window.clearTimeout(pairingPollTimer);
  if (pairingCountdownTimer !== undefined) window.clearInterval(pairingCountdownTimer);
  pairingPollTimer = undefined;
  pairingCountdownTimer = undefined;
}

/**
 * cas-d8a5 (journey F31): the countdown last shown, per request. A dialog the
 * machine's claim rebuilds starts from it, and the next tick may only lower it.
 */
let pairingCountdownShown: { request: string; ms: number } | undefined;

/**
 * One pairing request from its code to the machine's claim: the code request
 * and the invitation the claim delivers share the relay's request id.
 */
function pairingRequestKey(pending: PendingPairing | null): string | undefined {
  if (!pending) return undefined;
  return pending.kind === "relay-request" ? pending.pairingRequestId : pending.relay?.pairingRequestId ?? pending.token;
}

function lastPairingCountdown(): number | undefined {
  const request = pairingRequestKey(pendingPairing);
  return request !== undefined && pairingCountdownShown?.request === request ? pairingCountdownShown.ms : undefined;
}

function shownPairingCountdown(): string | undefined {
  const expiresAt = pendingPairing?.expiresAt;
  if (!expiresAt) return undefined;
  return countdownLabel(nextCountdown(expiresAt, Date.now(), lastPairingCountdown()));
}

function syncPairingCountdown(): void {
  if (pairingCountdownTimer !== undefined) window.clearInterval(pairingCountdownTimer);
  pairingCountdownTimer = undefined;
  const expiresAt = pendingPairing?.expiresAt;
  if (!expiresAt) { pairingCountdownShown = undefined; return; }
  const update = () => {
    const output = document.querySelector<HTMLElement>("#pair-countdown");
    const remaining = nextCountdown(expiresAt, Date.now(), lastPairingCountdown());
    const request = pairingRequestKey(pendingPairing);
    if (request !== undefined) pairingCountdownShown = { request, ms: remaining };
    const label = countdownLabel(remaining);
    if (output && output.textContent !== label) output.textContent = label;
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
  // cas-813a: the opening line's motion clock runs from here, across the
  // attach and the first history page.
  conversationOpenedAt.set(sessionKey(machineId, session), Date.now());
  // The rail keeps its place while the status is on its way, so the composer
  // doesn't narrow under the operator a moment later (cas-813a).
  if (!statuses.has(sessionKey(machineId, session))) statusPending.add(sessionKey(machineId, session));
  render();
  renderTerminalConnecting(machineId, session);
  await Promise.all([loadStatus(machineId, session), loadLease(machineId, session)]);
  await connections.get(machineId)?.attach(session);
}

function renderTerminalConnecting(machineId: string, session: string): void {
  if (selectedMachineId !== machineId || selectedSession !== session) return;
  const grid = document.querySelector<HTMLElement>("#pane-grid");
  if (grid?.dataset.sessionKey !== sessionKey(machineId, session)) return;
  const placeholder = grid.querySelector<HTMLElement>(":scope > .empty");
  if (!placeholder) return;
  // A conversation opens calmly (journey F3): no codename, no relay words,
  // and the one opening line the thread keeps until its first page (cas-813a).
  if (hubPresentation === "conversation") { showOpeningInto(placeholder, CONVERSATION_OPENING, openingDelay(sessionKey(machineId, session))); return; }
  placeholder.classList.remove("terminal-state");
  placeholder.textContent = `Connecting to ${session}…`;
}

/** When each conversation began to open (cas-813a). */
const conversationOpenedAt = new Map<string, number>();

/** Milliseconds until a conversation's opening line starts to move. */
function openingDelay(key: string, now = Date.now()): number {
  const since = conversationOpenedAt.get(key);
  return since === undefined ? OPENING_MOTION_DELAY_MS : OPENING_MOTION_DELAY_MS - (now - since);
}

function clearDisconnectedState(grid: HTMLElement): void {
  grid.classList.remove("terminal-disconnected");
  grid.querySelector(".terminal-disconnected-banner")?.remove();
  const shown = document.querySelector<HTMLElement>("#toast");
  if (shown) placeToastClearOfBanner(shown);
}

function openConnectionLog(machineId: string): void {
  let dialog = document.querySelector<HTMLDialogElement>("#connection-log");
  if (!dialog) {
    dialog = document.createElement("dialog");
    dialog.id = "connection-log";
    dialog.className = "connection-log";
    dialog.innerHTML = '<section><header><div><p class="connection-log-eyebrow">Evidence ledger</p><h2>Connection log</h2></div><form method="dialog"><button type="submit" aria-label="Close connection log">×</button></form></header><pre>Running diagnostics…</pre></section>';
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

/**
 * The hub closed just this session's stream and the machine is still
 * connected: the drop is the conversation's, not the machine's (cas-d15c).
 */
function sessionOnlyDrop(machineId: string, session: string): boolean {
  return attachStates.get(sessionKey(machineId, session))?.sessionOnly === true && connectionStates.get(machineId)?.phase === "live";
}

/** A conversation as the operator knows it: its project, else its session name. */
function conversationLabel(machineId: string, session: string): string {
  return projectTitle(sessions.get(machineId)?.find((item) => item.name === session)?.project_dir) || session;
}

/**
 * Whether the reconnect banner over the session's last frame states its
 * connection now (or will, in this render): the session has a frame on
 * screen and is not plainly live (renderConnectionSurface's own test).
 */
function bannerStatesConnection(machineId: string, session: string | undefined): boolean {
  if (!session) return false;
  const grid = document.querySelector<HTMLElement>("#pane-grid");
  if (grid?.dataset.sessionKey !== sessionKey(machineId, session) || !grid.querySelector(".pane")) return false;
  const snapshot = attachStates.get(sessionKey(machineId, session)) ?? connectionStates.get(machineId);
  return snapshot !== undefined && shouldRetainDisconnectedFrame(snapshot);
}

function renderConnectionSurface(machineId: string, session: string, snapshot: ConnectionState, now = Date.now()): void {
  if (selectedMachineId !== machineId || selectedSession !== session) return;
  const grid = document.querySelector<HTMLElement>("#pane-grid");
  if (grid?.dataset.sessionKey !== sessionKey(machineId, session)) return;
  // A fatal machine verdict wins over a session's last retry snapshot.
  const machineState = connectionStates.get(machineId);
  if (machineState?.fatal === true) snapshot = machineState;
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
    // Plain words in the body font (cas-a447): who was lost and what happens next.
    const where = machines.get(machineId)?.label ?? "the machine";
    // cas-d15c: with the machine still connected, only this session's daemon
    // link dropped; the banner names the conversation, not the machine.
    const sessionOnly = sessionOnlyDrop(machineId, session);
    // A refused pairing is not reconnecting: say what the header's "Needs
    // pairing" means instead (cas-d15c).
    const pairingLost = Boolean(snapshot.authFailure ?? connectionStates.get(machineId)?.authFailure);
    // The words sit in their own span so the 1 Hz repaint updates them
    // without rebuilding the Re-pair control (and dropping its focus).
    let words = banner.querySelector<HTMLElement>(":scope > .banner-text");
    if (!words) {
      words = document.createElement("span");
      words.className = "banner-text";
      banner.replaceChildren(words);
    }
    // cas-a6f0: still live but its heartbeats go unanswered: not lost yet.
    const unsteady = !pairingLost && snapshot.phase === "live" && snapshot.degraded;
    const sentence = pairingLost
      ? pairingLostBanner(where)
      : unsteady
        ? unsteadyBanner(where)
        : sessionOnly
          ? sessionReconnectingBanner(conversationLabel(machineId, session), where, snapshot.fatal === true)
          : lostConnectionBanner(where, snapshot.fatal === true, snapshot.reason);
    // Journey F42: the banner is a live region, and rewriting the same words
    // on every repaint announced the outage again each time. Only a change
    // of words is written, so the outage is announced once.
    if (words.textContent !== sentence) words.textContent = sentence;
    // cas-d636 QA F01: a phone hides the rail's Re-pair, so the banner that
    // says the pairing is gone carries it.
    const repair = banner.querySelector<HTMLButtonElement>(":scope > .banner-repair");
    if (pairingLost && !repair) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "banner-repair";
      button.textContent = "Re-pair";
      button.setAttribute("aria-label", `Re-pair ${where}`);
      button.onclick = () => openRepairDialog(machineId);
      banner.append(button);
    } else if (!pairingLost && repair) {
      repair.remove();
    }
    banner.dataset.scope = pairingLost ? "pairing" : unsteady ? "unsteady" : sessionOnly ? "session" : "machine";
    banner.dataset.attempt = String(view.attempt);
    grid.classList.add("terminal-disconnected");
    // A toast already up when the banner arrives moves clear of it (cas-00cc).
    const shown = document.querySelector<HTMLElement>("#toast.visible");
    if (shown) placeToastClearOfBanner(shown);
    return;
  }
  clearDisconnectedState(grid);
  if (snapshot.phase === "live") return;
  const placeholder = grid.querySelector<HTMLElement>(":scope > .empty");
  if (!placeholder) return;
  renderConnectionSurfaceInto(placeholder, session, snapshot, {
    retry: () => { void connections.get(machineId)?.attach(session); },
    diagnose: () => openConnectionLog(machineId),
    repair: () => openRepairDialog(machineId),
  }, now, {
    ...(hubPresentation === "conversation" ? { openingTitle: CONVERSATION_OPENING, quietOpening: true } : {}),
    quietRetry: "session" in snapshot && firstAttachRetry(snapshot as AttachSnapshot, sessionsEverLive.has(sessionKey(machineId, session))),
  });
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
  const placeholder = grid.querySelector<HTMLElement>(":scope > .empty");
  if (!placeholder) return;
  // cas-28df: a never-live conversation's first retry is still opening; the
  // connection surface shows it calmly, with this detail behind "Details".
  const key = sessionKey(machineId, session);
  if (firstAttachRetry(attachStates.get(key), sessionsEverLive.has(key))) return;
  const message = document.createElement("p");
  message.textContent = `Terminal unavailable: ${detail}`;
  const retry = document.createElement("button");
  retry.className = "primary retry-terminal";
  retry.textContent = "Try again";
  retry.onclick = () => {
    renderTerminalConnecting(machineId, session);
    void connections.get(machineId)?.attach(session);
  };
  placeholder.classList.remove("conversation-opening");
  placeholder.classList.add("terminal-state");
  placeholder.replaceChildren(message, retry);
}

async function loadStatus(machineId: string, session: string): Promise<void> {
  const connection = connections.get(machineId);
  const machine = machines.get(machineId);
  const key = sessionKey(machineId, session);
  if (!connection || !machine) { if (statusPending.delete(key)) render(); return; }
  if (!statuses.has(key)) statusPending.add(key);
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
    statusPending.delete(key);
    render();
  } catch {
    // The connection supervisor owns transport/auth reporting; the rail stops
    // holding a place for a status that is not coming.
    if (statusPending.delete(key)) render();
  }
}

async function loadLease(machineId: string, session: string): Promise<void> {
  try {
    const state = await connections.get(machineId)?.lease(session);
    // A control on a message that the lease change repaints (the "Waiting for
    // …" pill turning back into Take control) hands focus to its replacement,
    // not to the page body (cas-88d86 QA F01), as a refused take does.
    const pressed = document.activeElement instanceof HTMLElement && document.activeElement.closest(".conversation-reading.thread") ? document.activeElement : null;
    let holderChanged = false;
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
      leaseExpiryTimers.delete(key);
      // While another device holds the open session, its release is checked
      // for, so "Waiting for …" turns back into Take control without a blind
      // retry (journey F5). The expiry, when sooner, still wins.
      const heldElsewhere = !state.held_by_me && Boolean(state.controller_label);
      const watching = heldElsewhere && selectedMachineId === machineId && selectedSession === session;
      const expiryDelay = state.expires_at ? Math.max(0, new Date(state.expires_at).getTime() - Date.now() + 100) : undefined;
      const delay = watching ? Math.min(expiryDelay ?? Infinity, FOREIGN_LEASE_RECHECK_MS) : expiryDelay;
      if (delay !== undefined) leaseExpiryTimers.set(key, window.setTimeout(() => void loadLease(machineId, session), delay));
      // The refused message reads the holder: repaint it when that changes.
      holderChanged = previousLease?.controller_label !== state.controller_label || previousLease?.held_by_me !== state.held_by_me;
      if (holderChanged) updateConversationViews();
      if (state.held_by_me) startLeaseHeartbeat(machineId, session);
    }
    render();
    if (pressed && holderChanged && selectedMachineId === machineId && selectedSession === session) {
      landFocus([messageControl(pressed)], { keep: true, nextTask: true, waitMs: 1_000, since: pressed });
    }
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
  for (const pane of splitVisiblePanes(state.panes, revealWorkers, visibleSessions(machineId).find(item => item.name === session)?.workers ?? []).visible) {
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
  handFocusFromConnectionCard(grid);
  const { visible: visiblePanes, hiddenWorkers } = splitVisiblePanes(state.panes, hubPresentation === "terminal" && revealWorkers, visibleSessions(machineId).find(item => item.name === session)?.workers ?? []);
  // The hub strips hidden workers from the stream, so the roster count comes
  // from the catalog; local filtering covers hubs that predate the gate.
  const hiddenWorkerCount = revealWorkers
    ? 0
    : Math.max(hiddenWorkers.length, sessions.get(machineId)?.find((item) => item.name === session)?.workers.length ?? 0);
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
    grid.classList.remove("pane-layout", "single-pane", "workers-hidden");
    grid.replaceChildren(empty);
    syncEarlyThread();
    return;
  }
  // cas-fc2c: the panes are about to replace the thread shown before them.
  // A reader in it lands in the pane's thread, as a reader on the connecting
  // card does, rather than on the page once the grid is rebuilt.
  const earlyActive = document.activeElement;
  if (earlyActive instanceof HTMLElement && grid.querySelector(":scope > .conversation-early")?.contains(earlyActive)) {
    landFocus([focusTargets.thread], { keep: true, nextTask: true, waitMs: 2_000, since: earlyActive });
  }
  // Only the grid's own placeholder: a bare ".empty" also matched the
  // conversation thread's empty state and deleted it, leaving a re-opened
  // conversation on a blank panel (cas-04ee).
  grid.querySelector(":scope > .empty")?.remove();
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
  grid.classList.toggle("single-pane", visiblePanes.length === 1 && hiddenWorkerCount === 0);
  grid.classList.toggle("workers-hidden", hiddenWorkerCount > 0);
  grid.dataset.secondaryPaneGeometry = mobileCollapsedPaneGeometry;
  let primarySlot = grid.querySelector<HTMLElement>(".primary-pane-slot");
  let secondaryStrip = grid.querySelector<HTMLElement>(".secondary-pane-strip");
  if (!primarySlot || !secondaryStrip) {
    primarySlot = document.createElement("div"); primarySlot.className = "primary-pane-slot";
    secondaryStrip = document.createElement("div"); secondaryStrip.className = "secondary-pane-strip";
    grid.replaceChildren(primarySlot, secondaryStrip);
  }
  // cas-fc2c: the panes are up, so the thread shown before them gives way to
  // the one mounted over the supervisor pane below.
  syncEarlyThread();
  for (const [key, surface] of surfaces) {
    if (key.startsWith(`${machineId}:${session}:`) && !active.has(key.split(":").at(-1)!)) releaseSurface(key, surface);
  }
  renderHiddenWorkersNote(secondaryStrip, hiddenWorkerCount);
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
        if (hubPresentation === "conversation") return;
        focusPane(machineId, session, pane.id);
      };
      const title = document.createElement("header"); title.className = "pane-header";
      const statusDot = document.createElement("span"); statusDot.className = `pane-status-dot ${pane.exited ? "exited" : "live"}`;
      const label = document.createElement("span"); label.className = "pane-title"; label.textContent = pane.title || pane.id;
      const role = document.createElement("span"); role.className = "pane-role"; role.textContent = pane.kind.toLowerCase();
      const activity = document.createElement("span"); activity.className = "pane-last-activity"; updatePaneActivity(activity, key);
      const controls = document.createElement("div"); controls.className = "pane-layout-controls";
      const button = (label: string, className: string, action: () => void) => {
        const control = document.createElement("button"); control.type = "button"; control.className = className; control.textContent = label;
        control.setAttribute("aria-label", label);
        // cas-2072: a short header shows these as glyphs; the tooltip names them.
        control.title = label;
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
      // cas-d1fa: while this browser is in control the terminal keeps Tab, so
      // its own header says how the keyboard leaves it, outside the drawing
      // area. Activated, it takes focus itself (Safari does not focus a
      // clicked button), which also puts a phone's soft keyboard away.
      const leave = button("Leave terminal", "pane-leave", () => leave.focus());
      // A short visible word beside its key, so the pane's own name keeps its
      // room; where the header is compact the word gives way to a glyph and
      // the key stays on screen (cas-7d25).
      leave.textContent = "";
      const leaveWord = document.createElement("span"); leaveWord.className = "pane-leave-word"; leaveWord.textContent = "Leave";
      leave.append(leaveWord);
      leave.setAttribute("aria-keyshortcuts", "Control+Alt+M");
      leave.title = "Leave terminal (Ctrl+Alt+M)";
      const leaveKey = document.createElement("kbd"); leaveKey.setAttribute("aria-hidden", "true"); leaveKey.textContent = "Ctrl+Alt+M";
      leave.append(leaveKey);
      controls.append(
        leave,
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
    updatePaneActivity(card.querySelector<HTMLElement>(".pane-last-activity")!, key);
    const makePrimary = card.querySelector<HTMLButtonElement>(".make-primary");
    const moveEarlier = card.querySelector<HTMLButtonElement>(".move-earlier");
    const moveLater = card.querySelector<HTMLButtonElement>(".move-later");
    if (makePrimary) makePrimary.disabled = pane.id === layout.primaryPaneId;
    if (moveEarlier) moveEarlier.disabled = pane.id === layout.primaryPaneId || position <= 1;
    if (moveLater) moveLater.disabled = pane.id === layout.primaryPaneId || position === layout.paneIds.length - 1;
    const leaveTerminal = card.querySelector<HTMLButtonElement>(".pane-leave");
    if (leaveTerminal) leaveTerminal.hidden = leases.get(selectedKey)?.held_by_me !== true;
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
    // Creation measured a detached card; controls and placement now own their
    // final room. A heartbeat re-fit never moves a control or changes focus.
    fitPaneActivityCaption(card.querySelector<HTMLElement>(".pane-last-activity")!);
    const collapsedOnPhone = phoneLayout() && secondaryOnPhone;
    const existingSurface = surfaces.get(key);
    existingSurface?.setControlMode(leases.get(selectedKey)?.held_by_me === true);
    if (existingSurface && (collapsedOnPhone || existingSurface.element !== mount || !existingSurface.element.isConnected)) {
      releaseSurface(key, existingSurface);
    }
    if (collapsedOnPhone) continue;
    // The thread goes up before the surface loads beneath it: the canvas
    // stays hidden under it and the reader sees the conversation at once.
    if (hubPresentation === "conversation" && !surfaces.has(key)) mountConversation(key, mount);
    if (!surfaces.has(key)) {
      const surface = await createTerminalSurface(mount, {
        onData: (data) => { if (canControl(machineId, session, "pane-input")) sendControl(machineId, session, { Input: { pane_id: pane.id, data: [...data] } }); },
        onResize: (cols, rows) => requestPaneSize(machineId, session, pane.id, cols, rows),
        // The transcript is a reading of the same frame the grid just rendered,
        // so it follows the emulator's own tick instead of polling it.
        onRender: () => { transcripts.get(key)?.update(); conversationViews.get(key)?.update(); },
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
  const note = secondaryStrip.querySelector<HTMLElement>(".hidden-workers");
  if (note && secondaryStrip.lastElementChild !== note) secondaryStrip.append(note);
}

/**
 * One quiet line where the worker strip would be: how many workers the default
 * view keeps off screen, and the one control that reveals them.
 */
function renderHiddenWorkersNote(strip: HTMLElement, count: number): void {
  let note = strip.querySelector<HTMLElement>(".hidden-workers");
  if (count === 0) { note?.remove(); return; }
  if (!note) {
    note = document.createElement("p");
    note.className = "hidden-workers";
    note.setAttribute("role", "status");
    const label = document.createElement("span");
    label.className = "hidden-workers-label";
    const reveal = document.createElement("button");
    reveal.type = "button";
    reveal.className = "hidden-workers-reveal";
    reveal.textContent = "Show workers";
    reveal.title = "Show worker panes for debugging; reloads the Hub";
    reveal.onclick = (event) => { event.stopPropagation(); setWorkersRevealed(true); };
    note.append(label, reveal);
    strip.append(note);
  }
  // A live region: the same words rewritten on every repaint were spoken
  // again each time, through an outage too (journey F42).
  const label = note.querySelector<HTMLElement>(".hidden-workers-label")!;
  const words = hiddenWorkersLabel(count);
  if (label.textContent !== words) label.textContent = words;
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
  if (!hubSupports(machine.id, "daemon_attach")) return "This hub does not support Cassy Cloud control. Upgrade the hub, then reconnect this machine.";
  const missingScopes = ["pane-input", "message-send", "pane-interrupt"] as const;
  if (missingScopes.some((scope) => !machine.scopes.includes(scope))) {
    return `Relay pairing granted read-only scopes for ${location.origin}. Run cas hub pair --origin ${location.origin}, open the new pairing URL here, and approve control access on ${machine.label}. Pairings are specific to each Cassy Cloud origin.`;
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
  // While the session is known to be down, the banner and the header already
  // say it is reconnecting; a toast repeating it would only cover the banner
  // (cas-00cc). A send that fails while the state still reads live does warn.
  // Nor does a message held while the machine is unsteady or being probed:
  // the held bubble and the composer say so (cas-a6f0).
  if (connections.get(machineId)?.holdsMessages()) return false;
  const attach = attachStates.get(sessionKey(machineId, session));
  if (!attach || attach.phase === "live" || attach.phase === "idle") toast("Terminal is reconnecting");
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

/**
 * Sessions whose control this browser held when the connection dropped
 * (cas-7b31, journey F2). The outage controls promise that control "returns
 * when it reconnects", so it is taken back once the session is live again,
 * unless another device took it meanwhile (the hub refuses that take) or the
 * operator released it, or the pairing was refused.
 */
const controlLostToOutage = new Set<string>();

const CONTROL_DROPPED_TOAST = "Control released — the hub connection dropped";
const CONTROL_PAIRING_TOAST = "Control released — this browser needs pairing again";

function invalidateMachineLeases(machineId: string, cause: "outage" | "released" = "outage"): void {
  for (const [key, timer] of leaseHeartbeats) {
    if (key.startsWith(`${machineId}:`)) { window.clearInterval(timer); leaseHeartbeats.delete(key); }
  }
  let held = false;
  for (const [key, lease] of leases) {
    if (!key.startsWith(`${machineId}:`)) continue;
    held ||= lease.held_by_me;
    if (lease.held_by_me && cause === "outage") controlLostToOutage.add(key);
    if (cause === "released") controlLostToOutage.delete(key);
    // The controller identity was learned over the connection that just died.
    // Keeping it told the operator that another controller — in fact this very
    // browser — was holding the session against them.
    leases.set(key, { ...lease, held_by_me: false, controller_label: undefined, controller_device_id: undefined });
  }
  // Control disappearing in silence invites typing into a terminal that is no
  // longer listening.
  // cas-d15c QA N2: a refused pairing is not a dropped connection; cas-a6f0:
  // nor is a machine that still reads live (its lease heartbeat failed before
  // its heartbeats said Unsteady): that connection is being checked.
  // cas-7b31 (journey F2): the conversation view says nothing here. Its
  // header, banner and controls already say the session is down, the toast
  // covered thread text, and control comes back by itself. A release the
  // operator asked for needs no toast either: the control says so.
  if (!held || cause === "released" || hubPresentation === "conversation") return;
  const state = connectionStates.get(machineId);
  toast(state?.authFailure ? CONTROL_PAIRING_TOAST
    : state?.phase === "live" ? "Control released — checking the connection…"
    : CONTROL_DROPPED_TOAST);
}

let toastTimer: number | undefined;

/**
 * A toast never sits on the reconnect banner (cas-00cc): when the banner is on
 * screen where the toast would land (a phone thread, where the toast sits just
 * below the header), the toast drops below it. Otherwise the stylesheet's own
 * placement applies.
 */
function placeToastClearOfBanner(output: HTMLElement): void {
  output.style.removeProperty("top");
  output.style.removeProperty("right");
  const visibleBox = (element: HTMLElement | null) => element?.getClientRects().length ? element.getBoundingClientRect() : undefined;
  const inThread = toastPlacementInThread(
    visibleBox(document.querySelector<HTMLElement>(".conversation-shell.thread-open .conversation-heading")),
    visibleBox(document.querySelector<HTMLElement>(".conversation-shell.thread-open .conversation-main")),
    document.documentElement.clientWidth,
  );
  if (inThread) { output.style.top = `${inThread.top}px`; output.style.right = `${inThread.right}px`; }
  // On the list the header row is the brand and the appearance control; a
  // toast at the phone's top edge covered both (journey F8). It drops below
  // that row whenever it would land on it, as it does below a thread header.
  const brandRow = visibleBox(document.querySelector<HTMLElement>(".conversation-shell:not(.thread-open) .conversation-list-top"));
  const belowBrand = toastTopClearOfBanner(parseFloat(getComputedStyle(output).top), output.getBoundingClientRect(), brandRow);
  if (belowBrand !== undefined) {
    // Below the brand it covered the list's title row (cas-71af, dfb2 QA F01):
    // above the list's bottom action it covers no heading at all.
    const heading = visibleBox(document.querySelector<HTMLElement>(".conversation-shell:not(.thread-open) .conversation-list-title"));
    const action = visibleBox(document.querySelector<HTMLElement>(".conversation-shell:not(.thread-open) #compose-fab"))
      ?? visibleBox(document.querySelector<HTMLElement>(".conversation-shell:not(.thread-open) #hub-footer-badges"));
    const above = toastTopAboveAction(output.getBoundingClientRect(), action, heading);
    output.style.top = `${above ?? toastTopClearOfBanner(belowBrand, output.getBoundingClientRect(), heading) ?? belowBrand}px`;
  }
  const banner = document.querySelector<HTMLElement>(".terminal-disconnected-banner");
  const top = toastTopClearOfBanner(parseFloat(getComputedStyle(output).top), output.getBoundingClientRect(), banner?.getClientRects().length ? banner.getBoundingClientRect() : undefined);
  if (top !== undefined) output.style.top = `${top}px`;
}

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
  placeToastClearOfBanner(output);
  output.classList.add("visible");
  if (toastTimer !== undefined) window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => output.classList.remove("visible"), 3200);
}

function connectionLabel(state: ConnectionState | AttachSnapshot | undefined): string {
  if (!state) return "idle";
  if (state.phase === "live") {
    if (state.degraded) return `unsteady · ${state.missedHeartbeats} missed`;
    return state.latencyMs === undefined ? "live" : `live · ${state.latencyMs}ms`;
  }
  if (state.phase === "backoff") return `retrying ${state.stage} in ${Math.ceil((state.retryInMs ?? 0) / 1000)}s`;
  if (state.phase === "failed") return state.reason ?? `failed during ${state.stage}`;
  const elapsed = "session" in state ? attachElapsedSeconds(state) : elapsedSeconds(state);
  return `${state.phase} · ${elapsed}s`;
}

function connectionClass(state: ConnectionState | undefined): string { return state?.degraded ? "degraded" : state?.phase ?? "idle"; }

/** The re-pair command shown under the status that introduces it (cas-093d F02). */
let pairingRepairCommand: { status: string; command: string } | undefined;
/** Shown only while the status that introduces it is the one on screen. */
function shownRepairCommand(): string | undefined {
  return pairingRepairCommand && pairingRepairCommand.status === pairingStatus ? pairingRepairCommand.command : undefined;
}

function pairDialogMarkup(): string {
  return renderPairDialogMarkup({
    repairCommand: shownRepairCommand(),
    cleanupFailed: pairingCleanupFailed,
    cleanupContext: pairingCleanupContext,
    pendingPairing,
    draft: pairingDraft,
    status: pairingStatus,
    createInFlight: pairingCreateInFlight,
    exchangeInFlight: pairingExchangeInFlight,
    relayOrigin,
    pageOrigin: location.origin,
    countdown: shownPairingCountdown(),
  });
}

// A phone sentence takes longer to type than the heartbeat render interval, so
// the composer draft survives re-render exactly like the pairing draft does.
let messageDraft = "";
let messageDraftSelection = 0;
// cas-7752: drafts also survive a reload or a same-tab navigation (a pair
// link opened in this tab), per conversation, in the bounded conversation store.
const conversationStorage = (() => { try { return window.localStorage; } catch { return undefined; } })();
const drafts = draftStore(conversationStorage);
const conversationDrafts: Map<string, Draft> = drafts.load();
/**
 * Machines whose pairing was revoked or removed in this page: their
 * conversations' words are not written to disk again until the machine is
 * paired again here. Without this, the next render would store the draft
 * still on screen right after it was purged.
 */
const conversationPersistenceBlocked = new Set<string>();

/**
 * cas-8d52: when this browser first saw each turn, per conversation, so a
 * reload rebuilds the thread at the times the visit showed (a machine whose
 * clock runs ahead otherwise re-timed every turn to the reload).
 */
const arrivals = arrivalStore(conversationStorage);
const storedArrivals: Map<string, Arrivals> = arrivals.load();
const persistedArrivals = new Map<string, string>();

function persistArrivals(): void {
  for (const [key, history] of conversationHistories) {
    if (conversationPersistenceBlocked.has(key.slice(0, key.indexOf(":")))) continue;
    const record = history.arrivalsRecord();
    const serialized = JSON.stringify(record);
    if (persistedArrivals.get(key) === serialized) continue;
    persistedArrivals.set(key, serialized);
    arrivals.save(key, record);
  }
}

/**
 * cas-adfc, cas-f657: conversations whose draft lives only in this page, and
 * why: over the store's bound, or refused by a full localStorage. The draft
 * stays in memory (conversationDrafts); the composer says so while it shows
 * that conversation.
 */
const draftsNotKept = new Map<string, "too-long" | "not-saved">();

/** The composer's draft note, for the conversation it is showing. */
function paintDraftNote(): void {
  const composer = document.querySelector<HTMLTextAreaElement>("#message-text");
  const key = composer?.dataset.threadKey;
  applyDraftNote(document, (key && draftsNotKept.get(key)) || false);
}

/** Record (or, with no draft, forget) a conversation's draft, in memory and in storage. */
function rememberDraft(key: string, draft: Draft | undefined): void {
  if (draft) conversationDrafts.set(key, draft); else conversationDrafts.delete(key);
  const machineId = key.slice(0, key.indexOf(":"));
  const saved = drafts.save(key, conversationPersistenceBlocked.has(machineId) ? undefined : draft);
  if (saved === "too-long" || saved === "not-saved") draftsNotKept.set(key, saved); else draftsNotKept.delete(key);
  paintDraftNote();
}

/**
 * The machine's pairing is gone (revoked, or removed from this browser): purge
 * every stored conversation for it, drafts and (cas-e7b1) unconfirmed messages
 * alike, and stop writing new ones until it is paired again.
 */
function purgeMachineConversations(machineId: string, options: { forgetInMemory: boolean }): void {
  conversationPersistenceBlocked.add(machineId);
  purgeConversations(conversationStorage, machineId);
  for (const key of [...storedSends.keys()]) if (key.startsWith(`${machineId}:`)) storedSends.delete(key);
  for (const key of [...storedArrivals.keys()]) if (key.startsWith(`${machineId}:`)) storedArrivals.delete(key);
  if (options.forgetInMemory) {
    for (const key of [...conversationDrafts.keys()]) if (key.startsWith(`${machineId}:`)) conversationDrafts.delete(key);
    for (const key of [...draftsNotKept.keys()]) if (key.startsWith(`${machineId}:`)) draftsNotKept.delete(key);
  }
}

/** The composer's text, stored as its thread's draft. */
function rememberComposerDraft(): void {
  const composer = document.querySelector<HTMLTextAreaElement>("#message-text");
  if (composer?.dataset.threadKey) rememberDraft(composer.dataset.threadKey, { text: composer.value, caret: composer.selectionStart ?? composer.value.length });
}
// Typing is stored as it happens, and once more as the page goes away, so a
// reload between renders loses nothing.
document.addEventListener("input", (event) => { if ((event.target as Element | null)?.id === "message-text") rememberComposerDraft(); });
window.addEventListener("pagehide", () => { rememberComposerDraft(); persistPendingSends(); persistArrivals(); });

function captureMessageDraft(): void {
  rememberComposerDraft();
  const key = selectedMachineId && selectedSession ? sessionKey(selectedMachineId, selectedSession) : "";
  const draft = conversationDrafts.get(key);
  messageDraft = draft?.text ?? "";
  messageDraftSelection = draft?.caret ?? 0;
}

function restoreMessageDraft(): void {
  const composer = document.querySelector<HTMLTextAreaElement>("#message-text");
  if (!composer) return;
  composer.dataset.threadKey = selectedMachineId && selectedSession ? sessionKey(selectedMachineId, selectedSession) : "";
  composer.value = messageDraft;
  const caret = Math.min(messageDraftSelection, messageDraft.length);
  composer.setSelectionRange(caret, caret);
  paintDraftNote();
}

/** A click made with a mouse (fine pointer). Browsers that do not report
 * `pointerType` on click fall back to the device's primary pointer. */
function finePointerClick(event: MouseEvent): boolean {
  const pointerType = (event as Partial<PointerEvent>).pointerType;
  if (pointerType) return pointerType === "mouse";
  return window.matchMedia("(pointer: fine)").matches;
}

/**
 * One place for "where does keyboard focus go now" (cas-7eaf). A view change
 * that rebuilds or closes what held focus used to leave it on <body>, so a
 * keyboard user had to Tab from the top of the page. After the render
 * settles, the first target that can take focus gets it. With `keep`, focus
 * the operator already has somewhere real is left alone.
 */
type FocusTarget = () => HTMLElement | null | undefined;
const focusTargets = {
  composer: (() => document.querySelector<HTMLTextAreaElement>("#message-text")) as FocusTarget,
  thread: (() => document.querySelector<HTMLElement>(".conversation-reading.thread")) as FocusTarget,
  sessionTitle: (() => document.querySelector<HTMLElement>("#session-picker-toggle")) as FocusTarget,
  terminal: (() => activePaneContext()?.surface.element.querySelector<HTMLElement>(".t3-ghostty-input")) as FocusTarget,
  conversationReturn: (() => document.querySelector<HTMLElement>("#conversation-return")) as FocusTarget,
};
function landFocus(targets: readonly FocusTarget[], options: { keep?: boolean; nextTask?: boolean; waitMs?: number; since?: Element | null } = {}): void {
  // `nextTask` waits a task, not a microtask: a dialog's cancel event runs
  // before the dialog hands focus back, and a tap's deferred render runs in
  // the task after its click (DeferredRenderScheduler.afterGesture).
  // `waitMs` keeps trying, a frame at a time, while the target is still
  // mounting (a thread whose history is loading, a terminal still attaching),
  // and for that long re-lands if a re-mount drops the landed focus to
  // <body>; focus the operator moves elsewhere is never taken back.
  const deadline = Date.now() + (options.waitMs ?? 0);
  // Where focus was when the operator acted: moving away from it is their own
  // choice, which `keep` respects. A landing scheduled for later passes the
  // moment it was asked for as `since`; capturing it when the timer fires
  // would treat a control the operator has since chosen as the baseline and
  // pull focus off it (cas-7eaf QA F01).
  const initial = options.since !== undefined ? options.since : document.activeElement;
  let landed: Element | undefined;
  const attempt = (): void => {
    const active = document.activeElement;
    // Focus left inside a dialog that just closed is lost too: the browser
    // drops it to <body> a moment later.
    const held = active && active !== document.body && active.isConnected && !active.closest("dialog:not([open])");
    const onTarget = held && targets.some((target) => target() === active);
    if (landed && held && !onTarget) return;
    if (!landed && options.keep && held && active !== initial) return;
    if (!onTarget) {
      for (const target of targets) {
        const element = target();
        if (!element || !element.isConnected || element.getClientRects().length === 0) continue;
        element.focus();
        if (document.activeElement === element) { landed = element; break; }
      }
    }
    if (Date.now() < deadline) requestAnimationFrame(attempt);
  };
  if (options.nextTask) window.setTimeout(attempt, 0);
  else queueMicrotask(attempt);
}

/**
 * The opened session replaces the connection card. Focus inside the card
 * (its Details, Retry or another action) would fall to the page body with it,
 * so it lands where the next keystroke belongs: the composer, else the
 * thread; in the terminal workspace, the attached terminal, else the way
 * back. Focus anywhere else is left alone (cas-9a96).
 */
function handFocusFromConnectionCard(grid: HTMLElement): void {
  const card = grid.querySelector<HTMLElement>(":scope > .empty:is(.terminal-state, .conversation-opening)");
  const active = document.activeElement;
  if (!card || !(active instanceof HTMLElement) || !card.contains(active)) return;
  const targets = hubPresentation === "terminal"
    ? [focusTargets.terminal, focusTargets.conversationReturn]
    : [focusTargets.composer, focusTargets.thread];
  // After this render replaces the card; `since` is the card's control, so a
  // control the operator moves to meanwhile is theirs and is not taken back.
  landFocus(targets, { keep: true, nextTask: true, waitMs: 2_000, since: active });
}

/**
 * Entering the terminal workspace: the keyboard and the mouse land in the
 * attached terminal, so keystrokes go to the pane; before a pane is attached,
 * and for a touch (no soft keyboard over the pane), on the way back to the
 * conversation (cas-7eaf).
 */
function landInTerminalView(event: MouseEvent | undefined): void {
  landFocus(touchActivation(event) ? [focusTargets.conversationReturn] : [focusTargets.terminal, focusTargets.conversationReturn]);
}

/** A tap from a touch screen or pen; keyboard (detail 0) and mouse are not. */
function touchActivation(event: MouseEvent | undefined): boolean {
  return Boolean(event && event.detail > 0 && !finePointerClick(event));
}

/**
 * After opening a conversation from a list row or the session picker: the
 * keyboard and the mouse land where the next keystroke belongs, as a palette
 * jump does; a touch lands on the thread to read, without raising a soft
 * keyboard (cas-7eaf).
 */
function landAfterOpen(opened: Promise<void>, event: MouseEvent | undefined): void {
  if (!touchActivation(event)) { focusJumpedComposer(opened); return; }
  // The thread may only mount once the session's history loads, so land
  // again when the open settles unless the operator has moved on.
  landFocus([focusTargets.thread, focusTargets.sessionTitle], { keep: true, nextTask: true, waitMs: 2_000 });
}

/** After a palette jump, hand focus to the opened conversation's composer
 * (restoreMessageDraft already put its caret back). Where the composer cannot
 * take focus — the terminal workspace hides it — the attached pane takes focus
 * once the attach settles, unless the operator has moved focus themselves. */
function focusJumpedComposer(opened: Promise<void>): void {
  // Captured at the pick, before anything moves focus: the landings below
  // run later and must not take focus the operator moved in the meantime.
  const pickedFrom = document.activeElement;
  const machineId = selectedMachineId;
  const session = selectedSession;
  const composer = document.querySelector<HTMLTextAreaElement>("#message-text");
  composer?.focus();
  if (composer && document.activeElement === composer) {
    const landed = composer.value;
    // A focused composer defers structural rebuilds (cas-8434), so the status
    // and lease that load after the jump would leave the shell stale — the
    // control command still offering "Take control" to its holder. Until the
    // operator types, nothing is lost by flushing that rebuild and landing
    // back in the fresh composer.
    void opened.then(() => {
      if (selectedMachineId !== machineId || selectedSession !== session || !deferredRender.pending) return;
      const field = document.querySelector<HTMLTextAreaElement>("#message-text");
      if (!field || document.activeElement !== field || field.value !== landed) return;
      field.blur();
      deferredRender.focusLeft();
      document.querySelector<HTMLTextAreaElement>("#message-text")?.focus();
    });
    return;
  }
  void opened.then(() => {
    if (selectedMachineId !== machineId || selectedSession !== session) return;
    // The attached terminal once it is ready (attaching takes a moment),
    // else the session title: never <body> (cas-7eaf). Focus the operator
    // moved themselves is kept.
    landFocus([focusTargets.terminal], { keep: true, waitMs: 1_500, since: pickedFrom });
    window.setTimeout(() => landFocus([focusTargets.terminal, focusTargets.sessionTitle], { keep: true, since: pickedFrom }), 1_600);
  });
}

function syncSpeechComposer(): void {
  const mic = document.querySelector<HTMLButtonElement>("#message-mic");
  if (!mic) return;
  applyMicState(mic, { mode: speechCapability === undefined ? "checking" : speechCapability.mode === "typing" ? "typing" : "speech", listening: speechInputState === "listening", detail: speechInputDetail });
}

function createSpeechController(capability: SpeechInputCapability): SpeechDictationController {
  return new SpeechDictationController(capability, {
    read: () => document.querySelector<HTMLTextAreaElement>("#message-text")?.value ?? messageDraft,
    caret: () => document.querySelector<HTMLTextAreaElement>("#message-text")?.selectionStart ?? messageDraftSelection,
    write: (value, _interim, caret) => {
      messageDraft = value;
      messageDraftSelection = caret ?? value.length;
      messageDelivery = undefined;
      const composer = document.querySelector<HTMLTextAreaElement>("#message-text");
      if (!composer) return;
      composer.value = value;
      composer.setSelectionRange(messageDraftSelection, messageDraftSelection);
      speechWroteThisRun = true;
      const delivery = document.querySelector<HTMLElement>("#message-delivery");
      if (delivery) delivery.hidden = true;
    },
    state: (next, detail = "") => {
      const wasListening = speechInputState === "listening";
      if (next === "listening" && !wasListening) speechWroteThisRun = false;
      speechInputState = next;
      speechInputDetail = detail;
      syncSpeechComposer();
      // cas-71f4 (journey F21): listening stopped with new words: on a fine
      // pointer the reply box takes focus, caret after them, to review and
      // send without another click. A touch device keeps its focus, so the
      // on-screen keyboard does not jump up unasked.
      if (wasListening && next !== "listening" && speechWroteThisRun) {
        speechWroteThisRun = false;
        const composer = document.querySelector<HTMLTextAreaElement>("#message-text");
        if (composer && focusAfterDictation(window)) {
          composer.focus({ preventScroll: true });
          composer.setSelectionRange(messageDraftSelection, messageDraftSelection);
        }
      }
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
    // Clearing the composer abandons the edit: the next send is a new message.
    if (!composer.value.trim()) editingRefused = undefined;
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
  // The keyboard is about to cover the bottom of the thread: pin the tail so
  // the last turn and any pinned ask sit directly above the field (cas-edc9).
  composer.onfocus = () => {
    for (const view of selectedConversationViews()) view.followTail();
    syncComposing();
  };
  // Deferred: focus passing to Send or the mic and straight back must not flicker the question open.
  composer.onblur = () => { window.setTimeout(syncComposing, COMPOSING_BLUR_MS); };
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

/**
 * cas-16eed: while the operator writes on a phone or touch screen (or with a
 * soft keyboard up), the pinned question folds to a one-line bar so at least
 * a few lines of the latest conversation stay readable above the field. On a
 * desktop there is room for both, so a question being answered in the
 * composer stays open.
 */
const COMPOSE_COLLAPSE_MEDIA_QUERY = `${PHONE_MEDIA_QUERY}, (pointer: coarse)`;
const COMPOSING_BLUR_MS = 150;
function syncComposing(): void {
  const composer = document.querySelector<HTMLTextAreaElement>("#message-text");
  const focused = composer !== null && composer.isConnected && document.activeElement === composer;
  const small = window.matchMedia(COMPOSE_COLLAPSE_MEDIA_QUERY).matches || keyboardViewportHeight(window.innerHeight, window.visualViewport) !== undefined;
  const selected = new Set(selectedConversationViews());
  for (const view of conversationViews.values()) view.setComposing(focused && small && selected.has(view));
}
/** The open thread's views. Views are keyed by pane (machine:session:pane), so match the session prefix. */
function selectedConversationViews(): ConversationView[] {
  if (!selectedMachineId || !selectedSession) return [];
  const thread = sessionKey(selectedMachineId, selectedSession);
  return [...conversationViews].filter(([key]) => key === thread || key.startsWith(`${thread}:`)).map(([, view]) => view);
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
function showComposerStatus(text: string, tone: "info" | "error", transport = false): void {
  messageStatus = { session: selectedMachineId && selectedSession ? sessionKey(selectedMachineId, selectedSession) : undefined, text, tone, ...(transport ? { transport } : {}) };
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

/**
 * A "reconnecting" refusal is about the connection, not the message: once the
 * session is live again it would contradict the header's "Live", so it clears.
 * The draft stays in the composer to send again (cas-b789).
 */
function clearTransportStatus(key: string): void {
  if (messageStatus?.transport && messageStatus.session === key) clearComposerStatus();
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
/**
 * Threads where this device took control after the hub last refused one of
 * its messages (cas-8e0a). The cached lease can say held while the hub
 * refuses (the refusal is the fresher evidence), so a refused message says
 * Retry will go through only once a take has actually succeeded since.
 */
const controlTakenAfterRefusal = new Set<string>();
function noteControlTaken(machineId: string, session: string): void {
  const key = sessionKey(machineId, session);
  if (leases.get(key)?.held_by_me) { controlTakenAfterRefusal.add(key); retireControlReleasedToast(); } else controlTakenAfterRefusal.delete(key);
}

/**
 * cas-ca7f (cas-7b31 QA F01): control is held again, so a "Control released
 * — …" toast from before (shown, or faded but still in the accessibility tree
 * as a role=status) is no longer true. It says control is back where a toast
 * is shown, and goes quiet otherwise.
 */
function retireControlReleasedToast(): void {
  const shown = document.querySelector<HTMLElement>("#toast");
  if (!shown?.textContent?.startsWith("Control released")) return;
  if (hubPresentation === "terminal") toast("Control is back");
  else { shown.classList.remove("visible"); shown.textContent = ""; }
}

async function takeControlForMessage(machine: StoredMachine, session: string): Promise<boolean> {
  try {
    await connections.get(machine.id)?.requestControl(session, false);
  } catch {
    return false;
  }
  await loadLease(machine.id, session);
  noteControlTaken(machine.id, session);
  return leases.get(sessionKey(machine.id, session))?.held_by_me === true;
}

/**
 * Take control from a refused message (cas-3433). The refusal tells the
 * operator to take control, and the conversation header carries no such
 * control, so the refused bubble offers it beside Retry. The hub's refusal is
 * itself evidence that the cached lease is stale, so this always asks the hub
 * rather than trusting `held_by_me`.
 */
async function takeControlForRefused(machineId: string, session: string): Promise<void> {
  const machine = machines.get(machineId);
  const key = sessionKey(machineId, session);
  if (!machine || pendingSubmissions.has(key)) return;
  const stillHere = () => selectedMachineId === machineId && selectedSession === session;
  // The message's control the operator pressed (cas-008f): the lease refresh
  // can rebuild the shell around the conversation, which drops focus to the
  // page body, so it is handed back to the same message afterwards.
  const pressed = document.activeElement instanceof HTMLElement && document.activeElement.closest(".conversation-reading.thread") ? document.activeElement : null;
  const before = leases.get(key);
  const force = Boolean(before?.controller_label && !before.held_by_me && machine.scopes.includes("hub-admin"));
  pendingSubmissions.add(key);
  try {
    showComposerStatus("Taking control of this session…", "info");
    let requested = true;
    try {
      await connections.get(machineId)?.requestControl(session, force);
    } catch {
      requested = false;
    }
    await loadLease(machineId, session);
    if (requested) noteControlTaken(machineId, session);
    // The refused message repaints: once control is held, Take control leaves
    // it and it says Retry will go through (cas-8e0a).
    updateConversationViews();
    if (!stillHere()) return;
    const after = leases.get(key);
    if (requested && after?.held_by_me) {
      // cas-b00c (journey F18): the message itself now says this device
      // controls the session and that Retry goes through, and focus is on its
      // Retry. The composer does not say it again in other words.
      clearComposerStatus();
      return;
    }
    // Journey F5: the message already names the device in control and what
    // to do, so the composer only points at it rather than saying it twice.
    showComposerStatus(after?.controller_label && !after.held_by_me
      ? REFUSED_SEE_ABOVE
      : "Could not take control of this session. Check that it is live, then take control again.", "error");
  } finally {
    pendingSubmissions.delete(key);
    if (pressed && stillHere()) landFocus([messageControl(pressed), focusTargets.thread], { keep: true, nextTask: true, waitMs: 1_000, since: pressed });
  }
}

/**
 * The control a keyboard user pressed on a message, or its stand-in once the
 * message repaints (cas-008f): the same element while it is still on screen
 * (a refused take leaves Take control in place), otherwise the same kind of
 * control on the same message, otherwise that message's Take control or
 * Retry. The message is found again by its bubble key.
 */
function messageControl(pressed: HTMLElement): FocusTarget {
  const bubbleKey = pressed.closest<HTMLElement>("[data-key]")?.dataset.key;
  const kind = pressed.className;
  return () => {
    if (pressed.isConnected && pressed.getClientRects().length > 0) return pressed;
    const bubble = bubbleKey ? [...document.querySelectorAll<HTMLElement>(".conversation-reading.thread [data-key]")].find((node) => node.dataset.key === bubbleKey) : undefined;
    if (!bubble) return null;
    return [...bubble.querySelectorAll<HTMLElement>("button")].find((button) => button.className === kind)
      ?? bubble.querySelector<HTMLElement>(".conversation-take-control, .conversation-retry");
  };
}

/** One pending receipt check per thread (cas-1622). */
const receiptChecks = new Map<string, ReturnType<typeof setTimeout>>();
/**
 * A send whose delivery receipt never comes stops saying "Sending…": at its
 * deadline (conversation-history RECEIPT_TIMEOUT_MS, or the shorter grace
 * once a supervisor turn lands after it) it turns "Not confirmed" with Retry.
 */
function scheduleReceiptCheck(key: string): void {
  const pending = receiptChecks.get(key);
  if (pending !== undefined) clearTimeout(pending);
  receiptChecks.delete(key);
  const history = conversationHistories.get(key);
  const wait = history?.nextReceiptCheck(Date.now());
  if (!history || wait === undefined) return;
  receiptChecks.set(key, setTimeout(() => {
    receiptChecks.delete(key);
    const changed = history.unconfirmSilent(Date.now());
    if (changed.length) {
      if (messageDelivery?.session === key && changed.includes(messageDelivery.clientRef)) {
        messageDelivery = undefined;
        document.querySelector<HTMLElement>("#message-delivery")?.setAttribute("hidden", "");
      }
      updateConversationViews(); renderConversationList();
    }
    scheduleReceiptCheck(key);
  }, wait));
}

/**
 * Sends made while the machine is unreachable (cas-0978), per thread, in
 * order. They never left this browser, so sending them once the session is
 * back cannot duplicate anything; one not sent within HELD_SEND_MS turns
 * "Not sent" with Retry and Edit instead of waiting forever.
 */
type HeldSend = { clientRef: string; supervisor: string; text: string; replyTo?: number; expiry: ReturnType<typeof setTimeout> };
const heldSends = new Map<string, HeldSend[]>();
const flushingHeldSends = new Set<string>();
const HELD_SEND_MS = 120_000;

/** The machine is reconnecting on its own, as opposed to refused (pairing gone, unsupported browser). */
function machineWillReconnect(machineId: string): boolean {
  const snapshot = connections.get(machineId)?.snapshot();
  return snapshot !== undefined && snapshot.phase !== "idle" && !snapshot.authFailure && !snapshot.fatal;
}

/**
 * Whether the thread's session is attached and its machine live right now. A
 * machine that holds messages (unsteady, or a doubted socket being probed) is
 * not: control is taken when it answers, as for any held send (cas-a6f0).
 */
function sessionIsUp(machineId: string, session: string): boolean {
  const connection = connections.get(machineId);
  return attachStates.get(sessionKey(machineId, session))?.phase === "live" && connection?.snapshot().phase === "live" && !connection.holdsMessages();
}

function holdSupervisorMessage(machine: StoredMachine, session: string, clientRef: string, supervisor: string, text: string, replyTo?: number): void {
  const key = sessionKey(machine.id, session);
  conversationHistory(key).hold(clientRef, supervisor, text, Date.now(), replyTo, session);
  queueHeldSend(machine, key, clientRef, supervisor, text, replyTo, HELD_SEND_MS);
}

/** When each send was first held, so a send held again keeps its original expiry (cas-0653). */
const heldSince = new Map<string, number>();

/**
 * The hub refused a send as retryable (cas-0653, `upstream_unavailable`): the
 * machine's daemon never received it. Hold it again, as a send made while the
 * machine was away, to go out once on the next live attach, or turn "Not
 * sent" when HELD_SEND_MS has passed since it was first held. Returns false
 * when there is no such send to hold (already receipted, or unknown).
 */
function reholdRefusedSend(machine: StoredMachine, session: string, clientRef: string): boolean {
  const key = sessionKey(machine.id, session);
  const history = conversationHistory(key);
  const send = history.rehold(clientRef);
  if (!send) return false;
  const now = Date.now();
  const first = heldSince.get(clientRef) ?? now;
  heldSince.set(clientRef, first);
  const remaining = HELD_SEND_MS - (now - first);
  if (remaining <= 0) {
    expireHeldSend(machine, key, clientRef);
    return true;
  }
  queueHeldSend(machine, key, clientRef, send.target, send.text, send.replyTo, remaining);
  return true;
}

/**
 * A held send ran out of time: it says Not sent on the message, with Retry
 * and Edit. The composer's "will go out by itself" line was about this send;
 * once nothing is held for the session it would contradict the message, so it
 * points at the message instead (cas-a355).
 */
function expireHeldSend(machine: StoredMachine, key: string, clientRef: string): void {
  heldSince.delete(clientRef);
  conversationHistory(key).reject(clientRef, outageRefusal(machine.label));
  if (!heldSends.get(key)?.length && messageStatus?.transport && messageStatus.session === key) showComposerStatus(REFUSED_SEE_ABOVE, "error");
}

function queueHeldSend(machine: StoredMachine, key: string, clientRef: string, supervisor: string, text: string, replyTo: number | undefined, expiresInMs: number): void {
  heldSince.set(clientRef, heldSince.get(clientRef) ?? Date.now());
  const expiry = setTimeout(() => {
    const queue = heldSends.get(key) ?? [];
    const index = queue.findIndex((held) => held.clientRef === clientRef);
    if (index < 0) return;
    queue.splice(index, 1);
    if (queue.length === 0) heldSends.delete(key);
    expireHeldSend(machine, key, clientRef);
    updateConversationViews(); renderConversationList();
  }, expiresInMs);
  const queue = heldSends.get(key) ?? [];
  queue.push({ clientRef, supervisor, text, replyTo, expiry });
  heldSends.set(key, queue);
}

/**
 * cas-e7b1: the operator's unsettled messages (held, unreceipted, not
 * confirmed, not sent) are kept per conversation in the conversation store,
 * so a reload or a discarded tab does not lose them. `storedSends` holds what
 * the last page left until each machine's connection is set up here.
 */
const sendStore = pendingSendStore(conversationStorage);
const storedSends: Map<string, PendingSend[]> = sendStore.load();
/** What was last written per conversation, so an unchanged thread is not rewritten. */
const persistedSends = new Map<string, string>();

function persistPendingSends(): void {
  for (const [key, history] of conversationHistories) {
    // Not restored yet: writing now would replace what the last page left.
    if (storedSends.has(key)) continue;
    const machineId = key.slice(0, key.indexOf(":"));
    const sends = conversationPersistenceBlocked.has(machineId) ? [] : history.pendingSends().map((send) => send.state === "held" ? { ...send, heldAt: heldSince.get(send.id) ?? send.at } : send);
    const serialized = JSON.stringify(sends);
    if (persistedSends.get(key) === serialized) continue;
    persistedSends.set(key, serialized);
    sendStore.save(key, sends);
  }
}

/**
 * Put the machine's messages from before the reload back in their threads.
 * A held one goes out once, under its own client_ref, when its session is
 * live, or turns Not sent when its wait (from when it was first held) is over.
 */
function restoreStoredSends(machine: StoredMachine): void {
  const prefix = `${machine.id}:`;
  const now = Date.now();
  for (const [key, sends] of [...storedSends]) {
    if (!key.startsWith(prefix)) continue;
    storedSends.delete(key);
    const history = conversationHistory(key);
    for (const held of history.restorePending(sends, now)) {
      const since = held.heldAt ?? held.at;
      heldSince.set(held.id, since);
      const remaining = HELD_SEND_MS - (now - since);
      if (remaining <= 0) expireHeldSend(machine, key, held.id);
      else queueHeldSend(machine, key, held.id, held.target, held.text, held.replyTo, remaining);
    }
  }
}

/**
 * The composer's line while a send waits on the connection, in the words the
 * banner, header and row use for the same outage (cas-a6f0, journey F8): the
 * machine is unsteady; only this conversation's link dropped (the banner's
 * own check, sessionOnlyDrop); or the machine is not connected.
 */
function heldSendStatus(machineId: string, session: string): string {
  const label = machines.get(machineId)?.label ?? "the machine";
  const machine = connectionStates.get(machineId);
  const after = "Your message will go out by itself when it's back.";
  // Unsteady, or a doubted socket being probed: it is being checked.
  if (machine?.phase === "live" && (machine.degraded || connections.get(machineId)?.holdsMessages())) return `${unsteadyBanner(label)} ${after}`;
  if (sessionOnlyDrop(machineId, session)) return `${conversationLabel(machineId, session)} on ${label} is reconnecting. ${after}`;
  return `${lostConnectionBanner(label, false)} ${after}`;
}

function showHeldSendStatus(machineId: string, session: string): void {
  showComposerStatus(heldSendStatus(machineId, session), "info", true);
  if (messageStatus) messageStatus.held = true;
}

/**
 * The hub refused this browser's pairing (revoked, unknown key): it will not
 * reconnect by itself, so the machine's sends stop waiting (cas-a6f0, journey
 * F35). A held send never left this browser: it is Not sent, re-pair to send.
 * One already on the wire may or may not have arrived and no receipt can come
 * now: it is Not confirmed, with Retry, rather than Sending… forever.
 */
function settleSendsForPairingLoss(machine: StoredMachine): void {
  const prefix = `${machine.id}:`;
  let changed = false;
  for (const [key, queue] of [...heldSends]) {
    if (!key.startsWith(prefix)) continue;
    heldSends.delete(key);
    for (const held of queue) {
      clearTimeout(held.expiry);
      heldSince.delete(held.clientRef);
      changed = conversationHistory(key).reject(held.clientRef, pairingRefusal(machine.label)) || changed;
    }
  }
  for (const [key, history] of conversationHistories) {
    if (!key.startsWith(prefix)) continue;
    const unconfirmed = history.unconfirmInFlight(Date.now());
    if (unconfirmed.length) changed = true;
    if (messageDelivery?.session === key && unconfirmed.includes(messageDelivery.clientRef)) {
      messageDelivery = undefined;
      document.querySelector<HTMLElement>("#message-delivery")?.setAttribute("hidden", "");
    }
  }
  if (messageStatus?.held && messageStatus.session?.startsWith(prefix)) {
    if (selectedMachineId && selectedSession && messageStatus.session === sessionKey(selectedMachineId, selectedSession)) showComposerStatus(REFUSED_SEE_ABOVE, "error");
    else messageStatus = undefined;
  }
  if (changed) { updateConversationViews(); renderConversationList(); }
}

/**
 * The session is live again: take back control this browser held when the
 * connection dropped (cas-7b31), then send what was held. A take the hub
 * refuses (another device took control meanwhile) leaves this browser an
 * observer, as before.
 */
async function reclaimControlThenFlush(machine: StoredMachine, session: string): Promise<void> {
  const key = sessionKey(machine.id, session);
  // While the machine still holds messages (unsteady, or a socket being
  // probed) the take waits: the next live announcement brings it back here.
  if (controlLostToOutage.has(key) && !connections.get(machine.id)?.holdsMessages()) {
    controlLostToOutage.delete(key);
    if (leases.get(key)?.held_by_me !== true) {
      await takeControlForMessage(machine, session);
      if (selectedMachineId === machine.id && selectedSession === session) render();
    }
  }
  await flushHeldSends(machine, session);
}

/** The session is back: send what was held, in order, each once. */
async function flushHeldSends(machine: StoredMachine, session: string): Promise<void> {
  const key = sessionKey(machine.id, session);
  if (flushingHeldSends.has(key) || !heldSends.get(key)?.length) return;
  flushingHeldSends.add(key);
  try {
    const history = conversationHistory(key);
    // The lease lapsed while the machine was away; the hub refuses a message
    // from an observer, so control is taken back first, as a send does.
    if (leases.get(key)?.held_by_me !== true && !await takeControlForMessage(machine, session)) {
      for (const held of heldSends.get(key) ?? []) {
        clearTimeout(held.expiry);
        history.reject(held.clientRef, `Could not take control of ${session} after reconnecting. Retry to send it.`);
      }
      heldSends.delete(key);
      return;
    }
    const queue = heldSends.get(key) ?? [];
    while (queue.length) {
      const held = queue[0]!;
      if (!connections.get(machine.id)?.send(session, supervisorMessage(held.supervisor, held.text, held.clientRef, held.replyTo))) break;
      queue.shift();
      clearTimeout(held.expiry);
      history.release(held.clientRef);
    }
    if (queue.length === 0) heldSends.delete(key);
    scheduleReceiptCheck(key);
  } finally {
    flushingHeldSends.delete(key);
    updateConversationViews(); renderConversationList();
  }
}

/** "the cas-src supervisor", never the generated codename (cas-71f4, journey F20). */
function supervisorPhrase(machineId: string, session: string): string {
  const project = projectTitle(visibleSessions(machineId).find((item) => item.name === session)?.project_dir);
  return project ? `the ${project} supervisor` : "the supervisor";
}

function deliverSupervisorMessage(machine: StoredMachine, session: string, supervisor: string, text: string, replyTo?: number, retryOf?: string, editOf?: string): void {
  const clientRef = crypto.randomUUID();
  // Earlier sends still held go first: a new message must not overtake them.
  const queued = (heldSends.get(sessionKey(machine.id, session))?.length ?? 0) > 0;
  const sent = !queued && sendControl(machine.id, session, supervisorMessage(supervisor, text, clientRef, replyTo));
  // Without an outcome the operator cannot tell a sent message from a lost
  // one, and the natural response is to send it a second time.
  if (!sent && !machineWillReconnect(machine.id)) {
    // In the banner's words, naming the machine it names (journey F9); a
    // refused pairing is not an outage (cas-a6f0).
    const refused = connectionStates.get(machine.id)?.authFailure
      ? `${pairingRefusal(machine.label)} Your message is kept; re-pair, then send it.`
      : outageRefusal(machine.label);
    showComposerStatus(refused, "error", true);
    return;
  }
  const history = conversationHistory(sessionKey(machine.id, session), session);
  if (retryOf) history.discardRefused(retryOf);
  // The edited version is on the wire: the refused original stays as a record
  // but can no longer be retried.
  if (editOf) { history.retireRefused(editOf); editingRefused = undefined; }
  if (!sent) {
    holdSupervisorMessage(machine, session, clientRef, supervisor, text, replyTo);
    updateConversationViews(); renderConversationList();
    if (selectedMachineId === machine.id && selectedSession === session) {
      const composer = document.querySelector<HTMLTextAreaElement>("#message-text");
      if (composer && composer.value.trim() === text) composer.value = "";
      rememberDraft(sessionKey(machine.id, session), undefined);
      messageDraft = composer?.value ?? "";
      messageDraftSelection = messageDraft.length;
      // A transport status: it clears when the session is back (and the held
      // message goes out then).
      showHeldSendStatus(machine.id, session);
      composer?.focus();
    }
    return;
  }
  history.submit(clientRef, supervisor, text, Date.now(), replyTo, session);
  scheduleReceiptCheck(sessionKey(machine.id, session));
  updateConversationViews(); renderConversationList();
  const storedDraft = conversationDrafts.get(sessionKey(machine.id, session));
  if (storedDraft?.text.trim() === text) rememberDraft(sessionKey(machine.id, session), undefined);
  if (selectedMachineId !== machine.id || selectedSession !== session) return;
  clearComposerStatus();
  const composer = document.querySelector<HTMLTextAreaElement>("#message-text");
  if (composer && composer.value.trim() === text) composer.value = "";
  rememberDraft(sessionKey(machine.id, session), undefined);
  messageDraft = composer?.value ?? "";
  messageDraftSelection = messageDraft.length;
  messageDelivery = { session: sessionKey(machine.id, session), target: supervisor, clientRef };
  // cas-71f4 (journey F20): the bubble's own "Sending…" is the one sending
  // signal; the composer no longer repeats it with the supervisor's codename.
  const delivery = document.querySelector<HTMLElement>("#message-delivery");
  if (delivery) { delivery.hidden = true; delivery.textContent = ""; }
  // Terminal view has no bubble on screen, so it says so once, by project.
  if (hubPresentation === "terminal") toast(`Sending to ${supervisorPhrase(machine.id, session)}`);
  // A phone operator usually has a second sentence; keep the caret where they
  // left it rather than dropping focus to the page body.
  composer?.focus();
}

/**
 * `quick` is a tapped quick-reply chip: its text goes out instead of the
 * composer's, answering that ask. Free text from the composer answers the
 * pinned ask, if one is waiting, so either way the send carries in_reply_to.
 * A retry of a refused send is also `quick`: it carries the refused send's own
 * in_reply_to (possibly none) and `retryOf`, the refused send it replaces.
 */
async function submitSupervisorMessage(quick?: { text: string; replyTo?: number; retryOf?: string }): Promise<void> {
  const composer = document.querySelector<HTMLTextAreaElement>("#message-text");
  if (!composer) return;
  const renderedThread = composer.dataset.threadKey;
  const selectedThread = selectedMachineId && selectedSession ? sessionKey(selectedMachineId, selectedSession) : undefined;
  if (renderedThread !== selectedThread) {
    showComposerStatus("The selected conversation changed. Reopen the conversation before sending.", "error");
    return;
  }
  const text = quick?.text ?? composer.value.trim();
  const replyTo = quick ? quick.replyTo : (selectedThread ? conversationHistory(selectedThread).pinnedAsk()?.notification_id : undefined);
  // A composer send in the thread whose refused message Edit reopened is that
  // message's edited version; a chip or a Retry is not.
  const editOf = !quick && editingRefused?.threadKey === selectedThread ? editingRefused?.id : undefined;
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
  const submissionKey = sessionKey(machine.id, session);
  if (pendingSubmissions.has(submissionKey)) return;
  pendingSubmissions.add(submissionKey);
  try {
    // While the session is down, taking control cannot succeed and its
    // refusal would blame the lease; the message is held instead and control
    // is taken when the session is back (cas-0978).
    if (plan.kind === "take-control-then-send" && (sessionIsUp(machine.id, session) || !machineWillReconnect(machine.id))) {
    showComposerStatus(plan.notice, "info");
    if (!await takeControlForMessage(machine, session)) {
      showComposerStatus(`Could not take control of ${session}, and the hub refuses a message from a device that is only observing. Send again to retry; if another device controls the session, wait for it to release control.`, "error");
      return;
    }
  }
  deliverSupervisorMessage(machine, session, supervisor, text, replyTo, quick?.retryOf, editOf);
  } finally {
    pendingSubmissions.delete(submissionKey);
  }
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
  return '<p class="empty-title">No session open</p><p class="empty-hint">Pick a session to attach its supervisor.</p><button id="open-machines" class="primary" type="button">Open machines</button>';
}

function capturePairingDraft(): void {
  const email = document.querySelector<HTMLInputElement>("#pair-email");
  if (email) pairingDraft.email = email.value;
  const technical = document.querySelector<HTMLDetailsElement>("#pair-dialog details.pair-technical");
  if (technical) pairingDraft.technicalOpen = technical.open ? technical.dataset.step as PairingStep : undefined;
  const form = document.querySelector<HTMLFormElement>("#pair-form");
  // A background re-render rebuilds the dialog; an opened disclosure stays open.
  const addressHelp = form?.querySelector<HTMLDetailsElement>("details.pair-address-help");
  if (addressHelp) pairingDraft.addressHelpOpen = addressHelp.open;
  if (form) pairingDraft = updatePairingDraft(pairingDraft, new FormData(form).entries(), pendingPairing?.kind === "invitation" && !pendingPairing.hubUrl);
}

/** Each machine's accent, recorded when it first pairs so later pairings never re-colour it (cas-50a7). */
const machineAccentStore = storageAccentStore((() => { try { return window.localStorage; } catch { return undefined; } })());

function render(captureDraft = true): void {
  // Every row, header and rail icon below reads its colour from this fleet.
  setMachineAccentFleet(machines.keys(), machineAccentStore);
  if (captureDraft) capturePairingDraft();
  captureMessageDraft();
  const composerWasFocused = document.activeElement?.id === "message-text";
  // A shell rebuild re-mounts the thread, which would drop focus a touch
  // landed there to <body> (cas-7eaf).
  const threadWasFocused = document.activeElement?.matches(".conversation-reading.thread") === true;
  // cas-d362: a control the thread owns (Load earlier, Jump to latest, a
  // pinned question's choices) is the same node after the rebuild re-mounts
  // the thread, but moving it dropped focus to <body>, so a reader who had
  // just tabbed to Load earlier pressed Enter on nothing. It takes focus back.
  const threadControl = document.activeElement instanceof HTMLElement && !threadWasFocused
    && document.activeElement.closest(".conversation-reading.thread, .pinned-ask, .conversation-jump, .conversation-unsent")
    ? document.activeElement : undefined;
  // A shell rebuild replaces every control. A keyboard user on one of them
  // (the Terminal-view return, a toolbar button) would drop to <body> and
  // their next Enter would do nothing, so the rebuilt control with the same id
  // takes focus back. Dialogs, the composer and the terminal have their own
  // rules below.
  const focusedControl = document.activeElement instanceof HTMLElement && document.activeElement.id
    && document.activeElement.id !== "message-text" && app.contains(document.activeElement)
    && !document.activeElement.closest("dialog") ? document.activeElement.id : undefined;
  const selected = selectedMachineId ? machines.get(selectedMachineId) : undefined;
  const lease = selected && selectedSession ? leases.get(sessionKey(selected.id, selectedSession)) : undefined;
  const status = selected && selectedSession ? statuses.get(sessionKey(selected.id, selectedSession)) : undefined;
  const compatibility = selected ? compatibilityWarning(selected.id) : undefined;
  const machineConnectionSnapshot = selected ? connectionStates.get(selected.id) : undefined;
  const terminalAttachSnapshot = selected && selectedSession ? attachStates.get(sessionKey(selected.id, selectedSession)) : undefined;
  const connectionSnapshot = terminalAttachSnapshot ?? machineConnectionSnapshot;
  // The header reads the same one connection state as the banner, the row and
  // the footer (cas-a447). While the session is down it names that state
  // instead of the machine's live latency, and it claims no control
  // (cas-edcd, cas-4a93): the lease cannot be exercised until it is back.
  const headerConnection = selected && selectedSession ? conversationConnection(selected.id, selectedSession) : machineConnectionSnapshot;
  const sessionDown = Boolean(selected && selectedSession) && headerConnection !== undefined && headerConnection.phase !== "live";
  // cas-1730: a session that was live and dropped cannot take or release
  // control or be interrupted until it is back, so those controls say why
  // instead of offering an action that cannot reach the machine. A first
  // connection is not an outage.
  const outageKind = sessionDown && selected && selectedSession
    && (sessionsEverLive.has(sessionKey(selected.id, selectedSession))
      || (machineConnectionSnapshot !== undefined && machineConnectionSnapshot.phase !== "live" && lastLiveAt.has(selected.id)))
    // cas-7b31 (journey F2): a refused pairing does not reconnect, and
    // control does not come back by itself, so it does not promise either.
    ? (machineConnectionSnapshot?.authFailure ? "pairing" as const
      : sessionOnlyDrop(selected.id, selectedSession) ? "session" as const : "machine" as const)
    : undefined;
  const outageReason = outageKind && selected && selectedSession
    ? (outageKind === "pairing" ? pairingControlsReason(selected.label)
      : outageKind === "session" ? sessionOutageControlsReason(conversationLabel(selected.id, selectedSession))
      : machineConnectionSnapshot?.fatal === true ? "Update your browser, then reload to use control and interrupts."
      : outageControlsReason(selected.label))
    : undefined;
  const controlReason = controlDisabledReason(selected, selectedSession, lease);
  const takeControlReason = outageReason ?? takeControlDisabledReason(selected, selectedSession, lease);
  const selectedHubSession = selected && selectedSession
    ? sessions.get(selected.id)?.find((item) => item.name === selectedSession)
    : undefined;
  const supervisor = supervisorTarget(selectedHubSession);
  const delivery = selectedSession && messageDelivery?.session === (selected && selectedSession ? sessionKey(selected.id, selectedSession) : undefined) ? messageDelivery : undefined;
  const thread = selected && selectedSession ? operatorReplies.get(sessionKey(selected.id, selectedSession)) ?? [] : [];
  // Evaluated with the draft the operator can actually see, so the button's
  // stated reason and the reason a send would print are the same sentence. It
  // never carries `disabled`: a disabled Send button swallows the tap and looks
  // exactly like a broken one.
  const sendPlan = planSupervisorSend(supervisorSendContext(messageDraft));
  const sendReason = sendPlan.kind === "blocked" && sendPlan.block !== "empty" ? sendPlan.reason : undefined;
  const composerStatus = messageStatus?.session === (selected && selectedSession ? sessionKey(selected.id, selectedSession) : undefined) ? messageStatus : undefined;
  // A held send's line follows the outage as it changes (unsteady, then
  // reconnecting), as the banner and header beside it do (cas-a6f0).
  if (composerStatus?.held && selected && selectedSession) composerStatus.text = heldSendStatus(selected.id, selectedSession);
  // A phone has no hover, so a title attribute is an explanation nobody can
  // reach. Unavailable controls stay focusable and say why when tapped.
  const interruptReason = outageReason ?? (!selected || !selectedSession || !canControl(selected.id, selectedSession, "pane-interrupt")
    ? controlReason ?? "Interrupt is unavailable for this session."
    : undefined);
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
  // cas-d15c QA F01: a refused pairing is not reconnecting; the rail says
  // what the header's "Needs pairing" and the banner say.
  const pairingLostHere = Boolean(selected) && (machineConnectionSnapshot?.authFailure !== undefined
    || (terminalAttachSnapshot?.authFailure !== undefined));
  // cas-a6f0 (journey F8): a machine that still reads live with its
  // heartbeats unanswered is unsteady, not reconnecting, as the header says.
  const unsteadyHere = machineConnectionSnapshot?.phase === "live" && machineConnectionSnapshot.degraded && !sessionDown;
  const staleStatusText = statusIsStale
    ? (pairingLostHere ? `Not live — this browser needs pairing again.${staleStatusTail}`
      : machineConnectionSnapshot?.fatal === true ? `Not live — ${fatalConnectionRecovery(machineConnectionSnapshot.reason)}${staleStatusTail}`
      : unsteadyHere ? `${UNSTEADY_SENTENCE}${staleStatusTail}`
      : `Not live — reconnecting.${staleStatusTail}`)
    : undefined;
  const terminalSessionKey = selected && selectedSession ? sessionKey(selected.id, selectedSession) : undefined;
  // While the session is up the chip reads the machine's own connection, as
  // the rail does (cas-bf07 QA F01); while it is down it names that state.
  const connectionChip = sessionDown
    ? { state: connectionClass(headerConnection), text: fleetConnectionLabel(headerConnection, selected?.id) }
    : headerConnectionChip(machineConnectionSnapshot, connectionClass(connectionSnapshot), fleetConnectionLabel(machineConnectionSnapshot, selected?.id));
  const connectionState = connectionChip.state;
  // cas-71af (bf07 QA F01): the chip's tooltip reads the same state as the
  // chip, the machine's own connection while the session is up, not the
  // terminal attach (which said "live" beside a Degraded chip).
  const connectionText = !selected ? "idle"
    : sessionDown ? connectionLabel(headerConnection)
    : connectionChip.state === "checking" ? "Checking the connection…"
    : connectionLabel(machineConnectionSnapshot ?? connectionSnapshot);
  const latencyText = connectionChip.text;
  const counts = attentionCounts(attention);
  const infoItems = dismissableInfoItems(attention);
  // With no paired machine and no event to inspect, the canvas is the only
  // useful surface. The rail's second pairing button and the empty attention
  // well otherwise split a phone into three unrelated empty states.
  const fleetEmpty = machineCatalogLoaded && machines.size === 0 && attention.length === 0;
  const showSessionControls = selected !== undefined && selectedSession !== undefined;
  // A touch screen has no tooltip: the reason a header control is unavailable
  // is printed under the header, not left in title and aria text (journey F9).
  // Journey F42: during an outage the banner says what was lost; this line
  // says only what it means for the controls, so the outage reads once.
  const controlsNotice = !showSessionControls ? undefined
    : outageKind === "machine" && machineConnectionSnapshot?.fatal === true ? outageReason
    : outageKind ? outageControlsNotice(outageKind) : sessionControlsNotice(takeControlReason, interruptReason);
  // With machines paired and nothing open, the canvas is the fleet: every
  // machine and its sessions, one tap from opening. An empty card pointing at a
  // drawer was a detour to the same list.
  const showFleetBoard = machineCatalogLoaded && machines.size > 0 && selectedSession === undefined;
  const mode = lease?.held_by_me ? "CONTROL" : "OBSERVER";
  const controlActionLabel = lease?.held_by_me ? "Release control" : lease?.controller_label && selected?.scopes.includes("hub-admin") ? "Force takeover" : "Take control";
  const machineLabel = selected?.label ?? "No machine";
  const compactMachineLabel = selected ? fleetInitialsFor(selected) : machineInitials(machineLabel);
  const controlActionDisabled = takeControlReason !== undefined;
  const controlCommand = controlCommandCopy({
    heldByMe: Boolean(lease?.held_by_me),
    forceTakeover: controlActionLabel === "Force takeover",
    controller: lease?.controller_label ?? undefined,
    disabledReason: controlActionDisabled ? takeControlReason ?? "Control unavailable" : undefined,
  });
  const sessionCommands = [...machines.values()].flatMap((machine) => visibleSessions(machine.id).map((session) => {
    // cas-786a (journey F30): the open conversation and those that need the
    // operator are marked, so palette Enter moves on to the next one that does.
    const current = machine.id === selectedMachineId && session.name === selectedSession;
    return sessionJumpCommandMarkup(machine, session, sessionSummaries.get(sessionKey(machine.id, session.name)), { current, needsYou: conversationNeedsYou(machine.id, session.name) });
  })).join("");
  const backTarget = previousSelection(selection);
  const backText = backLabel(backTarget, (machineId) => machines.get(machineId)?.label);
  // The session name is the switch: on a phone it is the only always-visible
  // chrome that can carry one, and the ⌘K palette is hidden below 500px.
  const sessionCount = [...machines.values()].reduce((total, machine) => total + visibleSessions(machine.id).length, 0);
  // The Terminal view title leads with the project, as the list and the
  // conversation header do; the generated codename follows it, secondary
  // (3.30.0 journey F2). No project named: the session name is the title.
  const selectedProject = projectTitle(selectedHubSession?.project_dir);
  const sessionTitleLead = selectedSession ? selectedProject ?? selectedSession : "Fleet overview";
  const sessionTitleCodename = selectedSession && selectedProject ? selectedHubSession?.supervisor || selectedSession : undefined;
  // The toggle is the page's h1, so its accessible name is the page title: it
  // starts with the visible words (project, then codename), names the machine,
  // and only then offers the switch. "Switch session — N available" alone hid
  // the open conversation from screen readers (journey F19).
  const sessionPickerCount = sessionCount === 0 ? "no sessions listed yet" : `${sessionCount} available`;
  const sessionPickerHint = `switch session (${sessionPickerCount})`;
  const sessionPickerTooltip = `Switch session (${sessionPickerCount})`;
  const sessionTitleText = [sessionTitleLead, sessionTitleCodename].filter(Boolean).join(" ");
  const sessionPickerLabel = `${sessionTitleText}${selectedSession && selected ? ` on ${selected.label}` : ""} — ${sessionPickerHint}`;
  // The browser tab and a screen reader's window title name the open
  // conversation too, not only the app (cas-3400 QA; cas-cf10 QA F03).
  const documentTitle = selectedSession ? `${sessionTitleText} — Cassy Cloud` : "Cassy Cloud";
  if (document.title !== documentTitle) document.title = documentTitle;
  const liveRegions: LiveRegionView = {
    ...(selected ? {
      connection: { state: connectionState, title: compatibility ?? connectionText, latencyText },
      mode: { badge: mode, compact: lease?.held_by_me ? "CTL" : "OBS", hidden: sessionDown },
    } : {}),
    ...(showSessionControls ? {
      controlAction: { label: controlActionLabel, ...(takeControlReason ? { disabledReason: takeControlReason } : {}) },
      ...(controlsNotice ? { controlsNotice } : {}),
    } : {}),
    ...(interruptReason ? { interruptReason } : {}),
    ...(staleStatusText ? { staleNotice: staleStatusText } : {}),
    ...(controlReason ? { controlReason } : {}),
    ...(sendReason ? { sendReason } : {}),
    ...(composerStatus ? { messageStatus: { text: composerStatus.text, error: composerStatus.tone === "error" } } : {}),
    // cas-71f4: no composer "Sending to <codename>…" line; the bubble says it.
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
    pairingCleanupFailed ? `cleanup-failed:${pairingCleanupContext.cause}:${pairingCleanupContext.storeOpen ? "store" : ""}:${pairingCleanupContext.rollbackPending ? "rollback" : ""}` : "",
    shownRepairCommand() ?? "",
  ].join("|");
  const signature = shellSignature({
    machineId: selectedMachineId,
    session: selectedSession,
    // Label as well as id: a credential refresh can rename a machine, and the
    // header chip and rail read that label.
    machineIds: [...machines.values()].map((machine) => `${machine.id}:${machine.label}`),
    sessionKeys: [...machines.keys()].flatMap((id) => visibleSessions(id).map((item) => `${id}/${item.name}`)),
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
    pairingView,
  }) + JSON.stringify([hubPresentation, selectedHubSession?.project_dir, infoItems.length > 0, launchAvailability()]);
  const active = document.activeElement;
  // Focus anywhere inside the open palette counts as composing too: a rebuild
  // would replace the dialog under a focused row, wipe its filter and leave
  // Enter to run whatever command now leads (cas-9648 QA F02).
  // The open session picker is the same: a rebuild would replace it under the
  // entry a keyboard user has arrowed onto (cas-1b86).
  const inOpenPalette = active instanceof HTMLElement && active.closest("#command-palette[open], #session-picker[open]") !== null;
  const composing = (isEditableElement(active) && app.contains(active)) || inOpenPalette;
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
  const machineDialog = document.querySelector<HTMLDialogElement>("#paired-machines-dialog");
  const pairedDialogWasOpen = machineDialog?.open === true;
  if (pairedDialogWasOpen) machineDialog!.remove();
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
    for (const [key, surface] of surfaces) releaseSurface(key, surface);
  }
  app.innerHTML = `
    ${browserNotice ? `<p class="browser-unsupported" role="alert">${escapeHtml(browserNotice)}</p>` : ""}
    <div class="shell${browserNotice ? " with-browser-notice" : ""}${machineDrawerOpen ? " drawer-open" : ""}${attentionPanelCollapsed ? " attention-collapsed" : " attention-expanded"}${fleetEmpty ? " fleet-empty" : ""}">
      <aside class="machine-navigation${machineDrawerOpen ? " drawer-open" : ""}" aria-label="Machines and sessions">
        <div class="machine-rail">
          <button id="machine-drawer-toggle" class="rail-control commander-mark" type="button" aria-label="Open machines and sessions" title="Machines and sessions" aria-expanded="${machineDrawerOpen}">${cloudBrand()}<span class="commander-mark-label">Machines</span></button>
          <nav id="machine-rail-list" aria-label="Machines"></nav>
          <button id="pair-toggle" class="rail-control pair-machine" type="button" aria-label="Pair a machine" title="Pair a machine"><span aria-hidden="true">+</span><span class="pair-machine-label">Pair</span></button>
        </div>
        <div class="machine-drawer" aria-hidden="${!machineDrawerOpen}"${machineDrawerOpen ? "" : " inert"}>
          <header class="drawer-header">${cloudBrand()}<button id="machine-drawer-close" type="button" aria-label="Close machines and sessions">×</button></header>
          ${compatibility ? `<div class="compatibility-warning" role="alert">${escapeHtml(compatibility)}</div>` : ""}
          <nav id="machine-tree" aria-label="Machine sessions"></nav>
          ${selected ? `<button id="remove-machine" class="remove-machine">Remove ${escapeHtml(selected.label)} from this browser</button>` : ""}
        </div>
      </aside>
      <main>
        <header class="session-header">
          <div class="session-identity">
            ${backTarget ? `<button id="session-back" class="session-back" type="button" aria-label="${escapeAttr(backText)}" title="${escapeAttr(backText)}"><span aria-hidden="true">‹</span><span class="session-back-label" aria-hidden="true">Back</span></button>` : ""}
            <h1 class="${selectedSession ? "toolbar-session-title" : ""}"><button id="session-picker-toggle" class="session-picker-toggle" type="button" aria-haspopup="dialog" aria-expanded="${sessionPickerOpen}" aria-label="${escapeAttr(sessionPickerLabel)}" title="${escapeAttr(sessionPickerTooltip)}"><span class="session-picker-name">${escapeHtml(sessionTitleLead)}</span>${sessionTitleCodename ? `<span class="session-picker-codename codename">${escapeHtml(sessionTitleCodename)}</span>` : ""}<span class="session-picker-caret" aria-hidden="true">▾</span></button></h1>
          </div>
          ${selected ? `<span class="machine-chip" data-compact-label="${escapeAttr(compactMachineLabel)}" title="${escapeAttr(machineLabel)}">${escapeHtml(machineLabel)}</span><span class="mode-badge ${mode.toLowerCase()}" data-compact-label="${lease?.held_by_me ? "CTL" : "OBS"}"${sessionDown ? " hidden" : ""}>${mode}</span><span class="connection-summary ${connectionState}" title="${escapeAttr(compatibility ?? connectionText)}"><span class="connection-dot"></span><span data-machine-latency="${escapeAttr(selected.id)}">${latencyText}</span></span>` : ""}
          <div class="actions"><button id="command-palette-toggle" class="command-palette-trigger" type="button" aria-label="Open command palette (${escapeAttr(paletteShortcutLabel())})" aria-keyshortcuts="Control+K Meta+K" title="Command palette (${escapeAttr(paletteShortcutLabel())})">${HEADER_PALETTE_ICON}<span class="action-label">${escapeHtml(paletteShortcutLabel())}</span></button>${showSessionControls ? `<span class="control-action" title="${escapeAttr(takeControlReason ?? controlActionLabel)}"><button id="lease" data-compact-label="${lease?.held_by_me ? "Rel" : "Ctrl"}" aria-label="${escapeAttr(controlActionLabel)}"${takeControlReason ? ` aria-disabled="true" data-disabled-reason="${escapeAttr(takeControlReason)}" aria-describedby="control-disabled-reason"` : ""}>${HEADER_CONTROL_ICON}<span class="action-label">${controlActionLabel}</span></button>${takeControlReason ? `<span id="control-disabled-reason" class="sr-only">${escapeHtml(takeControlReason)}</span>` : ""}</span><button id="interrupt" class="danger" data-compact-label="Int" aria-label="Interrupt selected pane" title="${escapeAttr(interruptReason ?? "Interrupt selected pane")}"${interruptReason ? ` aria-disabled="true" data-disabled-reason="${escapeAttr(interruptReason)}" aria-describedby="session-controls-reason"` : ""}>${HEADER_INTERRUPT_ICON}<span class="action-label">Interrupt</span></button>${lease?.held_by_me && hubPresentation === "terminal" ? `<span id="${TERMINAL_ESCAPE_HINT_ID}" class="sr-only">${TERMINAL_ESCAPE_HINT}</span>` : ""}` : ""}</div>
        </header>
        ${showSessionControls ? `<p id="session-controls-reason" class="session-controls-reason" role="note"${controlsNotice ? "" : " hidden"}>${escapeHtml(controlsNotice ?? "")}</p>` : ""}
        <p id="network-access-help" class="compatibility-warning" role="status" hidden></p>
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
          <section class="context-tab status-context" data-context-content="status" ${activeContextTab === "status" ? "" : "hidden"}><p class="status-stale" role="status" hidden></p><div id="status-view"></div>${composerMarkup(supervisor, operatorThreadMarkup(thread))}</section>
        </div>
      </aside>
    </div>
    <dialog id="command-palette" class="command-palette">
      <section>
        <header><strong>Commands</strong><button id="command-palette-close" type="button" aria-label="Close command palette">×</button></header>
        <input id="command-palette-query" type="search" aria-label="Filter commands" placeholder="Type a command or conversation">
        <div class="palette-commands">
          <section class="palette-group" data-palette-group="conversations" aria-labelledby="palette-group-conversations">
            <h3 id="palette-group-conversations" class="palette-group-heading">Conversations</h3>
            ${sessionCommands || '<p class="palette-empty">No live conversations yet.</p>'}
          </section>
          ${showSessionControls ? `<section class="palette-group" data-palette-group="session" aria-labelledby="palette-group-session">
            <h3 id="palette-group-session" class="palette-group-heading">This conversation</h3>
            <button type="button" class="palette-command" data-palette-action="control" ${controlActionDisabled ? "disabled" : ""}><span>${escapeHtml(controlCommand.title)}</span><small>${escapeHtml(controlCommand.hint)}</small></button>
          </section>` : ""}
          <section class="palette-group" data-palette-group="machines" aria-labelledby="palette-group-machines">
            <h3 id="palette-group-machines" class="palette-group-heading">Machines</h3>
            ${launchAvailability() === "ready" ? '<button type="button" class="palette-command" data-palette-action="new-session"><span>New session</span><small>Start a supervisor on a project</small></button>' : ""}
            <button type="button" class="palette-command" id="palette-paired-machines"><span>Paired machines</span><small>Hosts, connection and last seen</small></button>
            ${launchAvailability() === "grant" ? '<button type="button" class="palette-command" data-palette-action="new-session" data-launch-grant="true"><span>New session</span><small>Asks this browser\'s permission first</small></button>' : ""}
            ${infoItems.length > 0 ? `<button type="button" class="palette-command" data-palette-action="dismiss-info"><span>Dismiss all info</span><small>${infoItems.length} outstanding</small></button>` : ""}
          </section>
          <section class="palette-group" data-palette-group="appearance" aria-labelledby="palette-group-appearance">
            <h3 id="palette-group-appearance" class="palette-group-heading">Appearance</h3>
            ${(["system", "light", "dark"] as const).map((scheme) => `<button type="button" class="palette-command" data-palette-scheme="${scheme}"><span><span class="palette-check" aria-hidden="true" hidden>✓</span>Appearance · ${scheme === "system" ? "System" : scheme === "light" ? "Light" : "Dark"}</span><small>${scheme === "system" ? "Follow this device" : "Use this scheme"}</small></button>`).join("")}
          </section>
          <details class="palette-group palette-advanced" data-palette-group="advanced">
            <summary class="palette-group-heading">Advanced</summary>
            <button type="button" class="palette-command" data-palette-action="terminal-view"><span>Open the terminal view</span><small>Machines and terminal controls</small></button>
            <button type="button" class="palette-command" data-palette-action="workers"><span>${escapeHtml(workersCommandLabel(revealWorkers).title)}</span><small>${escapeHtml(workersCommandLabel(revealWorkers).hint)}</small></button>
            <button type="button" class="palette-command" data-palette-action="dormant"><span>${escapeHtml(dormantCommandLabel(revealDormant).title)}</span><small>${escapeHtml(dormantCommandLabel(revealDormant).hint)}</small></button>
          </details>
          <p class="palette-empty" id="palette-no-match" role="status" hidden></p>
        </div>
      </section>
    </dialog>
    <dialog id="session-picker" class="command-palette session-picker">
      <section>
        <header><strong>Sessions</strong><button id="session-picker-close" type="button" aria-label="Close session picker">×</button></header>
        <input id="session-picker-query" type="search" aria-label="Filter sessions" placeholder="Filter sessions">
        <div class="palette-commands" id="session-picker-list"></div>
        <p class="palette-empty" id="session-picker-no-match" role="status" hidden></p>
      </section>
    </dialog>
    ${pairedMachinesDialogMarkup()}
    ${pairDialogMarkup()}`;
  if (pairedDialogWasOpen && machineDialog) {
    document.querySelector('#paired-machines-dialog')?.replaceWith(machineDialog);
    machineDialog.close(); machineDialog.showModal();
  }
  if (hubPresentation === "conversation") {
    arrangeConversationShell(app, { selected: Boolean(selectedSession), supervisor, projectDir: selectedHubSession?.project_dir, host: selected?.label, machineId: selectedSession ? selected?.id : undefined, loaded: machineCatalogLoaded, paired: machines.size > 0, searchQuery: conversationSearchQuery, keyboardHint: keyboardHintOffered(), launch: launchAvailability() });
  } else {
    // The way back to the conversation is first in the workspace's Tab order:
    // after the header, the pane controls and the terminal (which keeps Tab)
    // a keyboard could not reach it. It is still drawn at the foot, where it
    // was, by flex order (cas-7eaf); a header copy would crowd the controls
    // off a 1280 px header.
    const returnControl = app.querySelector<HTMLButtonElement>("#talk-supervisor");
    if (returnControl) { returnControl.id = "conversation-return"; returnControl.textContent = "Conversations"; returnControl.closest("main")?.prepend(returnControl); }
    else app.querySelector(".session-identity")?.insertAdjacentHTML("afterbegin", '<button id="conversation-return" type="button">Conversations</button>');
  }
  if (preservedGrid) document.querySelector<HTMLElement>("#pane-grid")!.replaceWith(preservedGrid);
  // A toast raised before the shell changed (a conversation opening while
  // "connected" is up) follows the new layout rather than covering a heading.
  const visibleToast = document.querySelector<HTMLElement>("#toast.visible");
  if (visibleToast) placeToastClearOfBanner(visibleToast);
  const focusWinner = composerFocusWinner({ composerWasFocused, terminalWasFocused });
  if (focusWinner === "terminal") queueMicrotask(() => activePaneContext()?.surface.focus());
  restoreMessageDraft();
  if (focusWinner === "composer") queueMicrotask(() => document.querySelector<HTMLTextAreaElement>("#message-text")?.focus());
  if (threadWasFocused && focusWinner !== "composer" && focusWinner !== "terminal") landFocus([focusTargets.thread], { keep: true, waitMs: 500 });
  if (threadControl && focusWinner !== "composer" && focusWinner !== "terminal") landFocus([() => threadControl], { keep: true, waitMs: 500 });
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
  // After the regions, not before: a control can be hidden in fresh shell
  // markup until its region shows it (the phone Attention badge, cas-a5c6),
  // and focus() on a hidden control does nothing, so a rebuild left focus on
  // the page.
  if (focusWinner === "none" && focusedControl && (document.activeElement === document.body || document.activeElement === null)) {
    document.getElementById(focusedControl)?.focus({ preventScroll: true });
  }
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
  const networkHelp = document.querySelector<HTMLElement>("#network-access-help");
  const machineState = context.selected ? connectionStates.get(context.selected.id) : undefined;
  const help = machineState?.authFailure ? undefined
    : context.connectionSnapshot?.networkAccessHelp ?? machineState?.networkAccessHelp;
  if (networkHelp) {
    networkHelp.hidden = !help;
    if (networkHelp.textContent !== (help ?? "")) networkHelp.textContent = help ?? "";
  }
  renderConversationList();
  renderMachineNavigation();
  renderSessionPicker();
  renderFleetBoard();
  const railCounts = document.querySelector("#attention-rail-counts");
  if (railCounts) {
    // One labelled figure; the detailed severity split belongs to the panel.
    railCounts.setAttribute("aria-label", `Open attention. ${attentionSummary(context.counts).description}`);
    const summary = renderAttentionSummary(context.counts);
    const label = summary.querySelector(".attention-summary-label");
    // "Clear" beside a reconnect banner is a claim the page contradicts (cas-edcd).
    if (label) label.textContent = attentionSummary(context.counts).total > 0 ? "Needs you" : attentionOutage()?.word ?? "Clear";
    railCounts.replaceChildren(summary);
  }
  renderAttention();
  renderStatus(context.status);
  syncConversationContext();
  applyLiveRegions(app, context.liveRegions);
  if (context.selected && context.session && context.connectionSnapshot) {
    renderConnectionSurface(context.selected.id, context.session, context.connectionSnapshot);
  }
  syncConnectionViewTicker();
  syncEarlyThread();
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
      connectionClass(machineFooterConnection(machine.id)),
      machineRailLabel(machine.id, machineFooterConnection(machine.id)),
      visibleSessions(machine.id).map((item) => item.name).join(","),
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
/** When this page first saw each session's listed activity stamp, while that stamp read in its future (cas-24fe). */
const listedActivitySeen = new Map<string, { stamp: number; seen: number }>();

/** A session's listed activity in this browser's time (cas-24fe, `machineActivityAt`). */
function listedActivityAt(machineId: string, session: string, stamp: number, now: number = Date.now()): number {
  const key = sessionKey(machineId, session);
  const { at, seen } = machineActivityAt(stamp, conversationHistories.get(key)?.machineLead(), listedActivitySeen.get(key), now);
  if (seen) listedActivitySeen.set(key, seen);
  return at;
}

/**
 * When a session last did anything, and between whom (cas-55a4): the hub's
 * newest queue row for it, or this page's last pane output when that is
 * newer. Undefined when neither is known.
 */
function sessionActivity(machineId: string, session: string): { at?: number; label?: string; terminal?: boolean } | undefined {
  const hubSession = sessions.get(machineId)?.find((item) => item.name === session);
  const stamp = hubSession?.last_activity_at ? Date.parse(hubSession.last_activity_at) : NaN;
  const listed = Number.isFinite(stamp) ? listedActivityAt(machineId, session, stamp) : NaN;
  const prefix = `${sessionKey(machineId, session)}:`;
  const pane = Math.max(-Infinity, ...[...paneLastActivity].flatMap(([key, at]) => key.startsWith(prefix) ? [at] : []));
  if (Number.isFinite(pane) && (!Number.isFinite(listed) || pane > listed)) return { at: pane, label: "terminal output", terminal: true };
  if (Number.isFinite(listed)) return { at: listed, ...(hubSession?.last_activity ? { label: hubSession.last_activity } : {}) };
  return undefined;
}

/** End a session from its row (cas-55a4), then read the catalog again. */
async function endConversationSession(row: ConversationRow): Promise<void> {
  const connection = connections.get(row.machineId);
  if (!connection) throw new Error("this machine is not connected");
  await connection.endSession(row.session);
  endedSessions.add(sessionKey(row.machineId, row.session));
  if (selectedMachineId === row.machineId && selectedSession === row.session) commitSelection({ machineId: row.machineId });
  await connection.refreshSessions().catch(() => undefined);
  render();
}

/** Each row's newest activity seen this visit, so its time never moves backwards (cas-6acf). */
const rowActivityHighWater = new Map<string, number>();
/** Empty the list search and its field, and show every row again (cas-537f). */
function clearConversationSearch(): void {
  conversationSearchQuery = "";
  const search = document.querySelector<HTMLInputElement>("#conversation-search");
  if (search) search.value = "";
  renderConversationList();
}

/**
 * The row Enter in the list search opens (cas-537f): the first row on screen
 * while there is a query. It is marked for the eye (data-enter-target) and
 * named to assistive tech as the field's active descendant.
 */
/** Re-picks the palette's Enter target from the live list; set while a palette is bound (cas-786a). */
let refreshPaletteEnterTarget: (() => void) | undefined;

/**
 * The conversation's list row shows an unread count or the waiting dot
 * (cas-786a). The rendered row is read, not a cached row model: an unread
 * reply updates the row in place, and the palette must agree with what the
 * list shows.
 */
function conversationNeedsYou(machineId: string, session: string): boolean {
  const key = sessionKey(machineId, session);
  const row = [...document.querySelectorAll<HTMLElement>(".conversation-row")].find((node) => node.dataset.threadKey === key);
  if (row) return row.dataset.waiting === "true" || Number(row.dataset.unread ?? 0) > 0;
  const model = conversationRows.find((candidate) => candidate.key === key);
  return Boolean(model && (model.attention > 0 || (model.unread ?? 0) > 0));
}

function markConversationEnterTarget(container: HTMLElement): HTMLButtonElement | undefined {
  const search = document.querySelector<HTMLInputElement>("#conversation-search");
  const target = conversationSearchQuery.trim() ? container.querySelector<HTMLButtonElement>(".conversation-row") ?? undefined : undefined;
  for (const row of container.querySelectorAll<HTMLElement>(".conversation-row[data-enter-target]")) if (row !== target) delete row.dataset.enterTarget;
  if (target) {
    target.dataset.enterTarget = "true";
    if (!target.id) target.id = `conversation-row-${(target.dataset.threadKey ?? "").replace(/[^A-Za-z0-9_-]/g, "-")}`;
    search?.setAttribute("aria-activedescendant", target.id);
  } else search?.removeAttribute("aria-activedescendant");
  return target;
}

/** Where the operator left the conversation list (cas-5d2c). */
let conversationListScroll = 0;
/** The selection last brought into view, so a heartbeat never fights the operator's scrolling. */
let revealedConversation: string | undefined;

/**
 * Opening a conversation rebuilds the shell, list included. The new list
 * starts where the old one was, and the open row is brought into view once
 * per selection (cas-5d2c), instead of the list jumping to its top and
 * leaving the row just opened below the fold.
 */
function keepConversationListPlace(container: HTMLElement, selectedKey: string | undefined): void {
  if (container.dataset.placeKept !== "true") {
    container.dataset.placeKept = "true";
    container.scrollTop = conversationListScroll;
    container.addEventListener("scroll", () => { conversationListScroll = container.scrollTop; }, { passive: true });
  }
  if (selectedKey === revealedConversation) return;
  if (!selectedKey) { revealedConversation = undefined; return; }
  const row = [...container.querySelectorAll<HTMLElement>(".conversation-row")].find((node) => node.dataset.threadKey === selectedKey);
  // A list that isn't on screen (a phone's open thread) reveals it when it is.
  if (!row || container.clientHeight === 0) return;
  revealedConversation = selectedKey;
  // Its End session line belongs to it, so it comes into view too.
  const end = row.nextElementSibling instanceof HTMLElement && row.nextElementSibling.classList.contains("conversation-end") ? row.nextElementSibling : row;
  const box = container.getBoundingClientRect();
  const top = row.getBoundingClientRect().top - box.top;
  const bottom = end.getBoundingClientRect().bottom - box.top;
  if (top < 0) container.scrollTop += top;
  else if (bottom > container.clientHeight) container.scrollTop += Math.min(top, bottom - container.clientHeight);
  conversationListScroll = container.scrollTop;
}

/**
 * Back from a conversation (a phone's list and thread take turns): the list
 * is where the operator left it, and focus returns to the row that was open,
 * brought into view if it is not, instead of the first row, whose focus
 * scrolled the list back to its top (cas-f50f). With that row gone, the
 * first row takes focus where the list already is.
 */
function returnToConversationRow(openedKey: string | undefined): void {
  const container = document.querySelector<HTMLElement>("#conversation-list");
  const rows = [...document.querySelectorAll<HTMLButtonElement>("#conversation-list .conversation-row")];
  const row = rows.find((node) => node.dataset.threadKey === openedKey) ?? rows[0];
  if (!row) return;
  if (container && container.scrollTop !== conversationListScroll) container.scrollTop = conversationListScroll;
  row.focus({ preventScroll: true });
  if (!container) return;
  const box = container.getBoundingClientRect();
  const rect = row.getBoundingClientRect();
  if (rect.top < box.top) container.scrollTop += rect.top - box.top;
  else if (rect.bottom > box.bottom) container.scrollTop += rect.bottom - box.bottom;
  conversationListScroll = container.scrollTop;
}

function renderConversationList(): void {
  renderMachineRegister();
  const container = document.querySelector<HTMLElement>("#conversation-list");
  if (!container) return;
  const rows: ConversationRow[] = [...machines.values()].flatMap((machine) => visibleSessions(machine.id).filter((session) => supervisorTarget(session)).map((session) => {
    const key = sessionKey(machine.id, session.name);
    const selected = machine.id === selectedMachineId && session.name === selectedSession;
    // Preview is the last turn this page has seen; unread counts supervisor
    // turns that arrived while the thread was not open. Opening it reads them.
    const events = conversationHistories.get(key)?.events ?? [];
    const replies = events.filter((event) => event.kind === "reply").length;
    if (selected) readReplies.set(key, replies);
    // Waiting (ochre dot, hot time) is driven by asks and blockers the operator has not answered.
    const waiting = conversationHistories.get(key)?.waiting().length ?? 0;
    // cas-55a4: each row's time is its own session's last activity (its
    // newest turn here, its newest queue row, or its panes' output), so
    // several sessions of one project never show one shared catalog time.
    const activity = sessionActivity(machine.id, session.name);
    // cas-b00c: only confirmed activity dates a row. A message still waiting,
    // not confirmed or not sent never reached the supervisor as far as this
    // page knows, so it must not make its row "Most recent" or "now".
    // cas-24fe: dated by the time the thread shows each turn, never the machine's stamp.
    const lastTurn = conversationHistories.get(key)?.lastActivityAt() ?? -Infinity;
    // cas-6acf: a row's time never runs backwards without new activity.
    const activityAt = Math.max(activity?.at ?? -Infinity, lastTurn, rowActivityHighWater.get(key) ?? -Infinity);
    const active = Number.isFinite(activityAt) ? activityAt : undefined;
    if (active !== undefined) rowActivityHighWater.set(key, active);
    // Only the session's own activity gives a row a time; the catalog check
    // that every poll refreshes made idle rows read "now" forever.
    const time = active === undefined ? undefined : activityTime(active);
    const started = session.started_at === undefined ? NaN : Date.parse(session.started_at);
    // cas-5d2c: the title's activity in plain words, never "supervisor → x".
    const plainLabel = activity?.terminal ? activity.label : plainActivity(activity?.label);
    const activityLabel = active === undefined ? undefined : emptyActivityText({ at: active, ...(active === activity?.at && plainLabel ? { label: plainLabel } : {}) }, Date.now());
    return { key, machineId: machine.id, session: session.name, supervisor: session.supervisor, projectDir: session.project_dir, host: machine.label, activityAt: active, ...(Number.isFinite(started) ? { startedAt: started } : {}), canEnd: fleetControlGate(machine.scopes, "end-session", location.origin).allowed, freshness: activityLabel ?? "No activity seen yet", when: time?.short, whenSpoken: time?.spoken, preview: conversationHistories.get(key)?.preview(), activityLine: plainActivity(session.last_activity), unreachable: Boolean(session.unreachable), connection: session.unreachable ? "Unreachable · message pending" : session.dormant ? "Dormant" : session.liveness === "live" ? conversationStatusLabel(machine.id, session.name) : "Session unavailable", interrupted: session.liveness === "live" && INTERRUPTED_LABELS.has(conversationStatusLabel(machine.id, session.name)), attention: waiting, unread: Math.max(0, replies - (readReplies.get(key) ?? 0)), selected };
  }));
  conversationRows = rows;
  // cas-55a4: a project's live sessions on one machine sit together, most
  // recent first and marked; End session is offered there and on dormant rows.
  const shown = groupConversationRows(filterConversationRows(rows, conversationSearchQuery)).map((row) => ({
    ...row,
    canEnd: row.canEnd === true && (row.group !== undefined || sessions.get(row.machineId)?.find((item) => item.name === row.session)?.dormant === true),
  }));
  conversationList.render(container, shown, (row, event) => {
    // cas-537f: a result opened from the list search is a jump, as Enter is:
    // the full list is back for the next visit.
    if (conversationSearchQuery) clearConversationSearch();
    landAfterOpen(openSession(row.machineId, row.session), event);
  }, endConversationSession);
  keepConversationListPlace(container, shown.find((row) => row.selected)?.key);
  markConversationEnterTarget(container);
  // cas-786a: an unread reply or a new wait changes what palette Enter picks.
  refreshPaletteEnterTarget?.();
  const empty = document.querySelector<HTMLElement>("#conversation-empty");
  if (empty) {
    empty.hidden = shown.length > 0;
    const listState = rows.length > 0 ? { kind: "text" as const, text: conversationNoMatchText(conversationSearchQuery) } : conversationListState(machineCatalogLoaded, [...machines.keys()].map((id) => ({ catalogReceived: fleetCatalogUpdatedAt.has(id), phase: connectionStates.get(id)?.phase })));
    const markup = listState.kind === "loading" ? conversationSkeletonMarkup() : "";
    if (listState.kind === "loading") { if (empty.dataset.state !== "loading") empty.innerHTML = markup; }
    else empty.textContent = listState.text;
    empty.dataset.state = listState.kind;
  }
  const state = document.querySelector<HTMLElement>("#conversation-connection");
  if (state && selectedMachineId) {
    const label = conversationHeaderLabel(selectedMachineId, selectedSession);
    // The dot separates it from the codename on screen; the status reads just
    // the state ("Live", not "· Live") (cas-17e3).
    // Journey F42: while the reconnect banner says what happened (in its own
    // live region), the header's one word is not a second announcement of
    // the same outage. It speaks again for the return to Live.
    state.setAttribute("aria-live", bannerStatesConnection(selectedMachineId, selectedSession) ? "off" : "polite");
    if (state.dataset.label !== label) {
      state.dataset.label = label;
      const separator = document.createElement("span"); separator.setAttribute("aria-hidden", "true"); separator.textContent = " · ";
      state.replaceChildren(separator, label);
    }
    applyTerminalOffer(document, terminalOfferReason(label, machines.get(selectedMachineId)?.label));
  }
  // The state's width changes the room the machine · codename line has.
  fitConversationHost(document);
}

/** The conversation header's connection words; the empty thread reads the same (cas-010f). */
function conversationHeaderLabel(machineId: string, session: string | undefined): string {
  return visibleSessions(machineId).find((item) => item.name === session)?.unreachable ? "Unreachable · message pending" : conversationStatusLabel(machineId, session);
}

/**
 * A conversation's connection words for its header and list row (cas-813a).
 * While the machine is connected and the conversation is still opening for
 * the first time (its attach in progress, or the quiet first retry), they
 * keep the machine's word ("Live") instead of flipping to "Connecting" and
 * back; the thread says "Opening the conversation…".
 */
function conversationStatusLabel(machineId: string, session: string | undefined): string {
  const machine = connectionStates.get(machineId);
  if (session && machine?.phase === "live") {
    const key = sessionKey(machineId, session);
    const attach = attachStates.get(key);
    if (attach && !sessionsEverLive.has(key) && (attachInProgress(attach) || firstAttachRetry(attach, false))) return fleetConnectionLabel(machine, machineId);
  }
  return fleetConnectionLabel(conversationConnection(machineId, session), machineId);
}

/**
 * A machine that has not been live in this visit is still on its first
 * connection, retries included: that is "Connecting", never "Reconnecting"
 * (journey F14).
 */
function fleetConnectionLabel(state: ConnectionState | undefined, machineId?: string): string {
  return machineConnectionLabel(state, machineId ? lastLiveAt.has(machineId) : true);
}

const fleetBoard = new FleetBoardRenderer();

function renderFleetBoard(): void {
  const board = document.querySelector<HTMLElement>("#fleet-board");
  const entries = sessionPickerEntries({
    machines: [...machines.values()].map((machine) => ({ id: machine.id, label: machine.label })),
    // The fleet board is a fifth list of sessions on the same screen as the
    // picker and its count, so it reads the same rule (cas-645e QA F01).
    sessions: visibleSessionMap(),
    includeDormant: revealDormant,
    selection: selectedMachineId ? { machineId: selectedMachineId } : undefined,
    summaries: sessionSummaries,
  });
  fleetBoard.render(board, {
    machines: [...machines.values()].map((machine) => ({
      id: machine.id,
      label: machine.label,
      state: connectionClass(connectionStates.get(machine.id)),
      phase: fleetConnectionLabel(connectionStates.get(machine.id), machine.id),
      selected: machine.id === selectedMachineId,
      hubVersion: machineInfo.get(machine.id)?.version,
      catalogUpdatedAt: fleetCatalogUpdatedAt.get(machine.id),
    })),
    sessions: entries.map((entry) => {
      const counts = attentionCounts(attention.filter((item) => item.machineId === entry.machineId && item.session === entry.session));
      return { ...entry, attentionSeverity: counts.critical ? "critical" as const : counts.warning ? "warning" as const : counts.info ? "info" as const : undefined };
    }),
  }, {
    open: (machineId, session) => { machineDrawerOpen = false; void openSession(machineId, session); },
  });
}

function compatibilityWarning(machineId: string): string | undefined {
  const info = machineInfo.get(machineId);
  if (!info) return "Compatibility check unavailable: this hub may be older or newer. Read-only discovery may work, but controls stay disabled until it reports capabilities.";
  const missing = ["session_index", "daemon_attach", "machine_events"].filter((capability) => !info.capabilities.includes(capability));
  if (info.schema_version !== 1 || missing.length > 0) {
    return `Hub ${info.version} is version-skewed (schema ${info.schema_version}; missing ${missing.join(", ") || "no required capabilities"}). Upgrade or use a compatible Cassy Cloud build; unsupported controls are disabled.`;
  }
  return undefined;
}

function selectMachine(machine: StoredMachine): void {
  commitSelection({ machineId: machine.id });
  machineDrawerOpen = true;
  render();
}

/** "live · 8ms" while live; otherwise the plain phase the footer and rows use ("Reconnecting"). */
function machineRailLabel(machineId: string, snapshot: ConnectionState | undefined): string {
  return !snapshot || snapshot.phase === "live" ? connectionLabel(snapshot) : fleetConnectionLabel(snapshot, machineId);
}

/** A machine's rail tag, unique across the paired fleet (journey F25). */
function fleetInitialsFor(machine: StoredMachine): string {
  return fleetMachineInitials(machines.values()).get(machine.id) ?? machineInitials(machine.label);
}

function machineRailButton(machine: StoredMachine): HTMLButtonElement {
  // The footer's view of the machine: a dropped session makes it reconnecting,
  // as the banner, header and row already say (cas-edcd).
  const snapshot = machineFooterConnection(machine.id);
  const state = connectionClass(snapshot);
  const button = document.createElement("button");
  button.className = `machine-icon ${machineAccentClass(machine.id)} ${machine.id === selectedMachineId ? "active" : ""}`;
  button.type = "button";
  // The dot leads so it can never be clipped by the chip's corner radius, and
  // the phone shows the machine's actual name instead of two initials.
  button.innerHTML = `<span class="machine-state ${state}"></span><span class="machine-initials">${escapeHtml(fleetInitialsFor(machine))}</span><span class="machine-name">${escapeHtml(machine.label)}</span>`;
  button.title = `${machine.label} · ${machineRailLabel(machine.id, snapshot)}`;
  button.setAttribute("aria-label", `${machine.label}, ${machineRailLabel(machine.id, snapshot)}`);
  button.onclick = () => selectMachine(machine);
  return button;
}

function machineTreeGroup(machine: StoredMachine): HTMLElement {
  const group = document.createElement("section");
  group.className = `machine-group ${machine.id === selectedMachineId ? "active" : ""}`;
  const machineRow = document.createElement("button");
  machineRow.className = "machine-row";
  machineRow.type = "button";
  const snapshot = machineFooterConnection(machine.id);
  const state = connectionClass(snapshot);
  machineRow.innerHTML = `<span class="machine-state ${state}"></span><strong>${escapeHtml(machine.label)}</strong><small>${escapeHtml(machineRailLabel(machine.id, snapshot))}</small>`;
  machineRow.onclick = () => selectMachine(machine);
  group.append(machineRow);
  if (machine.id === selectedMachineId) {
    const sessionList = document.createElement("div");
    sessionList.className = "session-tree";
    for (const session of visibleSessions(machine.id)) sessionList.append(sessionButton(machine.id, session));
    if (!sessionList.childElementCount) {
      const empty = document.createElement("p"); empty.className = "drawer-empty"; empty.textContent = "No live sessions."; sessionList.append(empty);
    }
    group.append(sessionList);
  }
  return group;
}

/**
 * A session's status word while its machine is down names the outage
 * ("Reconnecting"), not the hub index's last liveness ("live"), as the rail,
 * header and banner already do (cas-1730). A machine that has not been live in
 * this visit is still connecting, and keeps the index's word.
 */
function sessionStatusLabel(machineId: string, status: string): string {
  const snapshot = machineFooterConnection(machineId);
  if (!snapshot || snapshot.phase === "live" || snapshot.phase === "idle" || !lastLiveAt.has(machineId)) return status;
  return fleetConnectionLabel(snapshot, machineId);
}

function sessionButton(machineId: string, session: HubSession): HTMLButtonElement {
  const button = document.createElement("button"); button.className = `nav-item ${session.name === selectedSession ? "active" : ""}`;
  const summary = sessionSummaries.get(sessionKey(machineId, session.name));
  const stale = summary && summary.phase !== "idle" && Date.now() - Date.parse(summary.generated_at) > 10 * 60 * 1000;
  // Project first, codename once in the meta line (journey F1): the same
  // headline and meta helpers the session picker rows use.
  const entry: SessionPickerEntry = {
    machineId, machineLabel: machines.get(machineId)?.label ?? machineId, session: session.name,
    project: projectTitle(session.project_dir),
    role: session.supervisor ? "supervisor" : "session", supervisor: session.supervisor || undefined,
    workerCount: session.workers.length, status: sessionStatusLabel(machineId, session.liveness.replaceAll("_", " ")),
    current: session.name === selectedSession,
  };
  button.innerHTML = summary
    ? `<small class="session-name session-eyebrow">${escapeHtml(sessionPickerHeadline(entry))}</small><span class="session-summary-title">${escapeHtml(summary.title)}</span><span class="phase-chip phase-${escapeAttr(summary.phase)}">${escapeHtml(summary.phase)}</span><small class="session-summary-description${stale ? " stale" : ""}">${escapeHtml(summary.description)}</small>`
    : `<span class="session-name">${escapeHtml(sessionPickerHeadline(entry))}</span><small class="session-meta">${escapeHtml(sessionPickerRowMeta(entry))}</small>`;
  button.onclick = () => { machineDrawerOpen = false; void openSession(machineId, session.name); };
  return button;
}

function openSessionPicker(): void {
  const wasOpen = document.querySelector<HTMLDialogElement>("#session-picker")?.open === true;
  sessionPickerOpen = true;
  render();
  // Opening the picker never rebuilds the shell (its open state is not in the
  // shell signature, cas-00ad), so render() only refreshes the list region;
  // show the existing dialog here.
  const picker = document.querySelector<HTMLDialogElement>("#session-picker");
  if (picker && !picker.open) picker.showModal();
  // The reused dialog still carries the last filter text while the list may
  // show every session, so the two disagree (cas-6f39e). A fresh open starts
  // from an empty filter and the full list, as the command palette does.
  const query = document.querySelector<HTMLInputElement>("#session-picker-query");
  // Every fresh open also re-picks what Enter runs (cas-786a): a conversation
  // that started waiting since the dialog was built leads it.
  if (query && !wasOpen) {
    query.value = "";
    query.dispatchEvent(new Event("input"));
  }
  syncSessionPickerToggle();
  queueMicrotask(() => {
    // A phone keyboard over a three-row list hides the list. The filter is
    // there for a fleet, not for the four sessions a laptop usually has.
    if (!phoneLayout()) document.querySelector<HTMLInputElement>("#session-picker-query")?.focus();
    document.querySelector<HTMLElement>("#session-picker-list [aria-current='true']")?.scrollIntoView({ block: "nearest" });
  });
}

/** `landOnTitle`: Escape and × put focus back on the session title; choosing a session lands in it instead. */
function closeSessionPicker(landOnTitle: boolean): void {
  sessionPickerClosed(landOnTitle);
  document.querySelector<HTMLDialogElement>("#session-picker")?.close();
}

/**
 * Every way the picker closes (×, Escape, choosing a session) lands here. It
 * does not rebuild the shell, so the toggle's aria-expanded is set in place;
 * a stale "true" would also let the next rebuild pop the picker back open.
 */
function sessionPickerClosed(landOnTitle: boolean): void {
  sessionPickerOpen = false;
  syncSessionPickerToggle();
  // The first open rebuilds the shell, so the toggle that opened the picker
  // is gone and the dialog hands focus back to <body>. Escape and × land on
  // the session title every time, so Enter reopens it (cas-7eaf).
  // 2 s: on a loaded machine the modal can still be closing when 500 ms run
  // out, and a title behind a modal cannot take focus.
  if (landOnTitle) landFocus([focusTargets.sessionTitle], { keep: true, nextTask: true, waitMs: 2_000 });
}

function syncSessionPickerToggle(): void {
  document.querySelector<HTMLButtonElement>("#session-picker-toggle")?.setAttribute("aria-expanded", String(sessionPickerOpen));
}

function renderSessionPicker(): void {
  const list = document.querySelector<HTMLElement>("#session-picker-list");
  if (!list) return;
  const entries = sessionPickerEntries({
    machines: [...machines.values()].map((machine) => ({ id: machine.id, label: machine.label })),
    // The same rule as the list, the palette and the count (cas-645e).
    sessions: visibleSessionMap(),
    includeDormant: revealDormant,
    selection: selection.current ?? (selectedMachineId ? { machineId: selectedMachineId, session: selectedSession } : undefined),
    summaries: sessionSummaries,
  }).map((entry) => entry.status === "dormant" ? entry : { ...entry, status: sessionStatusLabel(entry.machineId, entry.status) });
  // Every render lands here, including the latency tick. Rebuilding unchanged
  // entries would throw away the entry a keyboard user is on (and the filter's
  // hidden rows), so only a real change rebuilds the list.
  const signature = JSON.stringify([machines.size, entries]);
  if (list.dataset.renderedEntries === signature) return;
  list.dataset.renderedEntries = signature;
  const focused = document.activeElement instanceof HTMLElement && list.contains(document.activeElement) ? document.activeElement : undefined;
  const focusedKey = focused ? { machineId: focused.dataset.pickerMachine, session: focused.dataset.pickerSession } : undefined;
  try {
    rebuildSessionPickerList(list, entries);
  } finally {
    // The rebuilt rows come back unfiltered; re-run the live filter first.
    const query = document.querySelector<HTMLInputElement>("#session-picker-query");
    if (query?.value) query.dispatchEvent(new Event("input"));
    if (focusedKey) restoreSessionPickerFocus(list, focusedKey, query ?? undefined);
  }
}

/**
 * Put focus back on the rebuilt entry the keyboard user was on. When that
 * entry is gone or filtered out, keep focus inside the picker on its filter
 * rather than letting it fall to <body> behind the modal.
 */
function restoreSessionPickerFocus(list: HTMLElement, key: { readonly machineId?: string; readonly session?: string }, query: HTMLInputElement | undefined): void {
  const entry = [...list.querySelectorAll<HTMLButtonElement>(".session-picker-entry")]
    .find((candidate) => candidate.dataset.pickerMachine === key.machineId && candidate.dataset.pickerSession === key.session && !candidate.hidden);
  if (entry) entry.focus();
  else query?.focus();
}

function rebuildSessionPickerList(list: HTMLElement, entries: readonly SessionPickerEntry[]): void {
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
    button.dataset.searchText = `${entry.machineLabel} ${entry.project ?? ""} ${entry.session} ${entry.supervisor ?? ""} ${entry.title ?? ""} ${entry.status}`;
    if (entry.current) button.setAttribute("aria-current", "true");
    // The project leads, as in the conversation list and the palette; the
    // generated codename is secondary on the line beneath it, with the role
    // and status that tell one supervisor from another (3.30.0 journey F2).
    // The hub derives the roster from the live agent registry, so the count
    // is stated rather than hidden.
    button.innerHTML = `<span class="session-name">${escapeHtml(sessionPickerHeadline(entry))}</span><small class="session-meta">${escapeHtml(sessionPickerRowMeta(entry))}</small>${entry.title ? `<span class="session-summary-title">${escapeHtml(entry.title)}</span>` : ""}${entry.phase ? `<span class="phase-chip phase-${escapeAttr(entry.phase)}">${escapeHtml(entry.phase)}</span>` : ""}${entry.current ? '<span class="session-picker-current">Open</span>' : ""}`;
    button.onclick = (event) => {
      closeSessionPicker(false);
      machineDrawerOpen = false;
      landAfterOpen(openSession(entry.machineId, entry.session), event);
    };
    list.append(button);
  }
}

function renderAttention(): void {
  const container = document.querySelector<HTMLElement>("#attention-panel");
  if (!container) return;
  const visibleAttention = hubPresentation === "conversation" ? attention.filter((item) => item.machineId === selectedMachineId && (!item.session || item.session === selectedSession)) : attention;
  contextAttention = groupAttention(visibleAttention).length;
  renderAttentionPanel(container, visibleAttention, {
    dismiss: acknowledgeAttentionGroup,
    act: performAttentionAction,
    copy: async (payload) => {
      await navigator.clipboard.writeText(payload);
      // cas-177c: Copy copies the Details text (readable since cas-ed87), so say so.
      toast("Details copied");
    },
  }, {
    animateIds: newCriticalAttentionIds, reclassifyIds: reclassifiedAttentionIds, outage: attentionOutage()?.text,
    sessionLabel: (item) => {
      const session = sessions.get(item.machineId)?.find((session) => session.name === item.session);
      return session ? [projectTitle(session.project_dir), session.supervisor].filter(Boolean).join(" · ") || undefined : undefined;
    },
  });
  // After the panel is drawn, so the sheet can hand focus back into it (cas-a5c6).
  syncConversationAttention(hubPresentation === "conversation" && selectedSession ? coalesceAttention(visibleAttention).length : 0);
}

/**
 * The phone's Attention badge and sheet for the open session (cas-5c22). The
 * badge shows while the session has an open item; tapping it lays the context
 * rail over the thread with only its Attention section. It closes on Close,
 * Escape or once nothing is left, and focus returns to the badge or thread.
 */
let attentionSheetOpen = false;
let progressSheetSession: string | undefined;
const progressSheetOpen = () => progressSheetSession !== undefined;
const contextSheetOpen = () => attentionSheetOpen || progressSheetOpen();
function syncConversationAttention(count: number): void {
  const badge = document.querySelector<HTMLButtonElement>("#conversation-attention");
  if (badge) {
    const view = conversationAttentionBadge(count);
    badge.hidden = view.hidden;
    badge.setAttribute("aria-label", view.label);
    badge.setAttribute("aria-expanded", String(attentionSheetOpen && count > 0));
    const text = badge.querySelector(".conversation-attention-count");
    if (text && text.textContent !== view.text) text.textContent = view.text;
  }
  // Nothing left (the last item dismissed in the sheet) closes it with focus
  // back in the conversation; leaving the conversation, or the viewport
  // becoming a desktop where the rail is a side panel again (cas-a5c6),
  // closes it quietly.
  if (attentionSheetOpen && (count < 1 || !phoneLayout())) {
    if (count < 1 && hubPresentation === "conversation" && selectedSession) closeAttentionSheet();
    else { attentionSheetOpen = false; badge?.setAttribute("aria-expanded", "false"); }
  }
  if (progressSheetOpen() && (!phoneLayout() || !selectedSession || progressSheetSession !== sessionKey(selectedMachineId ?? "", selectedSession) || hubPresentation !== "conversation")) progressSheetSession = undefined;
  applyAttentionSheet();
}
/** The sheet control that last held focus, so a redraw that moves it hands focus back (cas-a5c6). */
let sheetFocus: HTMLElement | undefined;
/** The same control's redraw-proof key (cas-a5c6 QA F03). */
let sheetFocusKey: string | undefined;
function applyAttentionSheet(): void {
  applySheetSemantics(document.querySelector<HTMLElement>(".conversation-shell"), contextSheetOpen(), progressSheetOpen() ? "progress" : "attention");
  const fleetEntry = document.querySelector<HTMLButtonElement>("#conversation-fleet");
  fleetEntry?.setAttribute("aria-expanded", String(progressSheetOpen()));
  const close = document.querySelector<HTMLButtonElement>(".conversation-context .context-sheet-close");
  close?.setAttribute("aria-label", progressSheetOpen() ? "Close tasks & progress" : "Close attention");
  const rail = document.querySelector<HTMLElement>(".conversation-context");
  if (progressSheetOpen() && rail) {
    rail.dataset.open = "true"; rail.removeAttribute("aria-hidden");
    const progress = rail.querySelector<HTMLElement>('[data-section="progress"]');
    if (progress) progress.hidden = false;
  }
  placeFleetUndo();
  if (!contextSheetOpen()) { sheetFocus = undefined; sheetFocusKey = undefined; return; }
  // A redraw (a catalog poll, a new turn) rebuilds the shell and moves the
  // rail's panel, which drops focus to the page. Put it back where it was.
  const sheet = document.querySelector<HTMLElement>(".conversation-shell.attention-sheet-open > .conversation-context");
  if (!sheet || sheet.contains(document.activeElement) || layerAboveSheet(sheet)) return;
  // The same element if it survived; else the same control in the redrawn
  // panel; only when that control is gone, the sheet's first stop.
  const back = (sheetFocus?.isConnected && sheet.contains(sheetFocus) ? sheetFocus : undefined)
    ?? (sheetFocusKey === undefined ? undefined : findByFocusKey(sheet, sheetFocusKey))
    ?? sheetFocusables(sheet)[0];
  back?.focus({ preventScroll: true });
}
// cas-a5c6: while the sheet is modal, Escape closes it from anywhere and Tab
// stays inside it; focus that lands behind it is brought back.
document.addEventListener("keydown", (event) => {
  if (!contextSheetOpen()) return;
  const sheet = document.querySelector<HTMLElement>(".conversation-shell.attention-sheet-open > .conversation-context");
  if (sheet && sheetKeydown(event, sheet, document.activeElement, closeContextSheet)) {
    event.preventDefault();
    // Escape is the sheet's alone while it is open.
    if (event.key === "Escape") event.stopPropagation();
  }
}, true);
// cas-0739: a Paired machines register that closes is put back in order
// (machines that aren't connected first) for its next opening; while it was
// open, status ticks left its rows where they were.
document.addEventListener("close", (event) => {
  const dialog = event.target as HTMLDialogElement | null;
  if (dialog?.id !== "paired-machines-dialog") return;
  renderMachineRegister();
  restorePairedMachinesOpener(dialog);
}, true);

/**
 * cas-460a: a shell rebuild while Paired machines is open (a pairing revoked,
 * a machine added) replaces the footer and re-shows the dialog, so the
 * browser's own focus restoration has no live opener to return to and focus
 * fell to the page. When it did, focus goes back to the control that opened
 * the register, or the footer's Paired machines control when that one is gone
 * (the palette's entry closes with the palette). Focus still on a control
 * inside the closed dialog (the re-shown dialog's ×) counts as lost: the
 * browser drops it to the page a moment later. A close that hands focus on
 * (Pair a machine opens its own dialog) is left alone.
 */
function restorePairedMachinesOpener(dialog: HTMLDialogElement): void {
  const opener = pairedMachinesOpener;
  pairedMachinesOpener = undefined;
  const active = document.activeElement;
  if (active && active !== document.body && active.isConnected && !dialog.contains(active)) return;
  if (document.querySelector("dialog[open]")) return;
  const visible = (element: HTMLElement | null): element is HTMLElement => element !== null && element.getClientRects().length > 0;
  const fromOpener = opener ? document.getElementById(opener) : null;
  const target = visible(fromOpener) ? fromOpener : document.getElementById("paired-machines-toggle");
  if (visible(target)) target.focus();
}

// A layer that was over the sheet (the palette) has closed: focus goes back
// into the sheet, to the control it left, rather than to the page.
document.addEventListener("close", () => {
  if (!contextSheetOpen()) return;
  window.setTimeout(() => { if (contextSheetOpen()) applyAttentionSheet(); }, 0);
}, true);
document.addEventListener("focusin", (event) => {
  if (!contextSheetOpen()) return;
  const sheet = document.querySelector<HTMLElement>(".conversation-shell.attention-sheet-open > .conversation-context");
  if (!sheet || !(event.target instanceof HTMLElement)) return;
  if (sheet.contains(event.target)) { sheetFocus = event.target; sheetFocusKey = focusKey(sheet, event.target); }
  // A palette opened over the sheet keeps its focus (cas-a5c6 QA F02).
  else if (layerAboveSheet(sheet)) return;
  else sheetFocusables(sheet)[0]?.focus();
});
function openAttentionSheet(): void {
  progressSheetSession = undefined;
  attentionSheetOpen = true;
  applyAttentionSheet();
  document.querySelector<HTMLButtonElement>("#conversation-attention")?.setAttribute("aria-expanded", "true");
  document.querySelector<HTMLButtonElement>(".conversation-context .context-sheet-close")?.focus();
}
function openProgressSheet(): void {
  if (!selectedMachineId || !selectedSession) return;
  attentionSheetOpen = false;
  progressSheetSession = sessionKey(selectedMachineId, selectedSession);
  applyAttentionSheet();
  document.querySelector<HTMLButtonElement>(".conversation-context .context-sheet-close")?.focus();
}
function closeContextSheet(): void {
  if (!progressSheetOpen()) { closeAttentionSheet(); return; }
  dismissFleetPanel(false);
  progressSheetSession = undefined;
  syncConversationContext();
  applyAttentionSheet();
  document.querySelector<HTMLButtonElement>("#conversation-fleet")?.focus();
}

function closeAttentionSheet(): void {
  if (!attentionSheetOpen) return;
  attentionSheetOpen = false;
  applyAttentionSheet();
  const badge = document.querySelector<HTMLButtonElement>("#conversation-attention");
  badge?.setAttribute("aria-expanded", "false");
  if (badge && !badge.hidden) badge.focus(); else landFocus([focusTargets.thread]);
}

/**
 * The outage the Attention rail sits beside, in the words the rest of the page
 * uses, or undefined when everything it covers is live (cas-edcd). The
 * conversation's rail covers its own session; Terminal view's rail covers the
 * fleet. A machine that has never been live in this visit is still
 * connecting, not an outage.
 */
function attentionOutage(): { readonly text: string; readonly word: string } | undefined {
  // cas-a6f0 (journey F9): an unsteady machine is not "All clear" either.
  const down = (hubPresentation === "conversation" && selectedMachineId
    ? [[selectedMachineId, conversationConnection(selectedMachineId, selectedSession)] as const]
    : [...machines.keys()].map((id) => [id, machineFooterConnection(id)] as const))
    .filter(([id, state]) => lastLiveAt.has(id) && state !== undefined && (state.phase === "live" ? state.degraded : state.phase !== "idle"))
    .map(([id, state]) => {
      const label = machines.get(id)?.label ?? "A machine";
      const phase = fleetConnectionLabel(state, id);
      return {
        phase,
        fatal: connectionStates.get(id)?.fatal === true,
        text: phase === "Reconnecting" ? `${label} is reconnecting` : phase === UNSTEADY ? `${label}: ${UNSTEADY_SENTENCE.toLowerCase().replace(/…$/, "")}` : `${label}: ${phase}`,
      };
    });
  if (!down.length) return undefined;
  return { text: `Not all clear. ${down.map((item) => item.text).join("; ")}.`, word: down.every((item) => item.phase === UNSTEADY) ? UNSTEADY : down.every((item) => item.fatal) ? "Unreachable" : "Reconnecting" };
}

async function performAttentionAction(item: AttentionItem, action: AttentionAction): Promise<void> {
  if (action === "repair") {
    openRepairDialog(item.machineId);
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

/*
 * Fleet operations from the rail (cas-a474, fleet-operations brief S5). One
 * state per page; a session switch closes its menus. The rail is rebuilt only
 * when what it shows changes, so a heartbeat never closes a menu or moves focus.
 */
const fleetOps = new FleetOpsState();
const fleetAsked = new Map<string, number>();
let fleetHeaderPanel: "add" | "focus" | undefined;
let fleetFocusNext: string | undefined;
let fleetUndoTimer: number | undefined;

/**
 * The one live region for fleet results. It exists, empty, from the moment
 * the rail draws its controls, so a screen reader is already watching it when
 * the first sentence lands (cas-a474 QA N2).
 */
function fleetAnnouncer(): HTMLElement {
  let region = document.querySelector<HTMLElement>("#fleet-ops-announcer");
  if (!region) {
    region = document.createElement("p");
    region.id = "fleet-ops-announcer";
    region.className = "sr-only";
    region.setAttribute("role", "status");
    document.body.append(region);
  }
  return region;
}

function fleetAnnounce(text: string): void {
  const region = fleetAnnouncer();
  if (region.textContent !== text) region.textContent = text;
}

function syncFleetSelection(): void {
  const key = selectedMachineId && selectedSession ? sessionKey(selectedMachineId, selectedSession) : "";
  if (!fleetOps.select(key)) return;
  fleetHeaderPanel = undefined; fleetFocusNext = undefined;
  window.clearTimeout(fleetUndoTimer);
  document.getElementById("fleet-phone-undo")?.remove();
  fleetAnnounce("");
}

async function runFleetAction(rowKey: string, action: FleetAction): Promise<void> {
  const machineId = selectedMachineId;
  const session = selectedSession;
  const connection = machineId ? connections.get(machineId) : undefined;
  if (!machineId || !session || !connection) return;
  syncFleetSelection();
  const epoch = fleetOps.selectionEpoch;
  const result = await runFleetOperation(fleetOps, rowKey, action, (request) => connection.operation(session, request), () => {
    fleetAnnounce(action.progress);
    fleetFocusNext = `${rowKey}:progress`;
    renderStatus(statuses.get(sessionKey(machineId, session)));
  });
  if (!result || fleetOps.selectionEpoch !== epoch || selectedMachineId !== machineId || selectedSession !== session) return;
  if (result === "succeeded") {
    if (action.request.op.kind === "request_merge") fleetAsked.set(String(action.request.op.task_id), Date.now());
    fleetFocusNext = action.inverse ? "undo" : rowKey.startsWith("agent:") ? `${rowKey}:trigger` : undefined;
    window.clearTimeout(fleetUndoTimer);
    if (fleetOps.undo) fleetUndoTimer = window.setTimeout(() => { if (selectedMachineId && selectedSession) renderStatus(statuses.get(sessionKey(selectedMachineId, selectedSession))); }, UNDO_WINDOW_MS + 50);
  } else {
    fleetFocusNext = `${rowKey}:note`;
  }
  fleetAnnounce(fleetOps.announcement);
  renderStatus(statuses.get(sessionKey(machineId, session)));
  // The operation's own response drives the refresh; FleetChanged does for other devices.
  void loadStatus(machineId, session);
}

function fleetOpsContext(status: Record<string, unknown>): FleetOpsViewContext | undefined {
  const machine = selectedMachineId ? machines.get(selectedMachineId) : undefined;
  if (!machine || !selectedSession) return undefined;
  const rerender = (focus?: string) => { fleetFocusNext = focus; renderStatus(status); };
  const epics = ((status.epics as any[]) ?? []).map((epic) => String(epic?.id ?? epic)).filter(Boolean);
  const currentEpic = typeof status.focused_epic === "string" ? status.focused_epic : (((status.epics as any[]) ?? []).find((epic) => epic?.focused)?.id ?? null);
  return {
    state: fleetOps,
    phone: phoneLayout(),
    scopes: machine.scopes,
    origin: location.origin,
    now: Date.now(),
    agents: ((status.agents as any[]) ?? []) as FleetAgent[],
    tasks: [...((status.tasks_in_progress as any[]) ?? []), ...((status.tasks_ready as any[]) ?? [])] as FleetTask[],
    epics,
    currentEpic,
    asked: fleetAsked,
    relative: (at) => { const label = relativeTimestamp(at); return label === "now" ? "just now" : `${label} ago`; },
    on: {
      toggleMenu: (rowKey) => { const opening = fleetOps.menuFor !== rowKey; fleetOps.toggleMenu(rowKey); rerender(opening ? `${rowKey}:first-item` : `${rowKey}:trigger`); },
      choose: (rowKey, action) => {
        const run = fleetOps.choose(rowKey, action);
        if (rowKey === "header") fleetHeaderPanel = undefined;
        if (run) void runFleetAction(rowKey, run);
        else rerender(`${rowKey}:cancel`);
      },
      confirm: () => { const rowKey = fleetOps.confirm?.rowKey; const run = fleetOps.confirmed(); if (rowKey && run) void runFleetAction(rowKey, run); },
      cancelConfirm: () => { const rowKey = fleetOps.confirm?.rowKey; fleetOps.cancelConfirm(); rerender(rowKey ? `${rowKey}:trigger` : undefined); },
      openPreview: (rowKey, task) => { fleetOps.openPreview(rowKey, task); rerender(`${rowKey}:preview-cancel`); },
      sendMerge: (rowKey, task) => { fleetOps.closeMenus(); void runFleetAction(rowKey, fleetMergeAction(task)); },
      closePanels: () => { if (phoneLayout()) { dismissFleetPanel(); return; } const rowKey = fleetOps.preview?.rowKey ?? fleetOps.assignFor; fleetOps.closeMenus(); fleetHeaderPanel = undefined; rerender(rowKey ? `${rowKey}:ask` : undefined); },
      toggleAssign: (rowKey) => { const opening = fleetOps.assignFor !== rowKey; fleetOps.closeMenus(); fleetOps.assignFor = opening ? rowKey : undefined; rerender(opening ? phoneLayout() ? `${rowKey}:search` : `${rowKey}:first-item` : `${rowKey}:assign`); },
      toggleHeader: (panel) => { fleetOps.closeMenus(); fleetHeaderPanel = fleetHeaderPanel === panel ? undefined : panel; rerender(fleetHeaderPanel === "focus" ? phoneLayout() ? "header:search" : "header:first-item" : fleetHeaderPanel === "add" ? "header:add-go" : `header:${panel}`); },
      undo: () => { const run = fleetOps.takeUndo(Date.now()); if (run) void runFleetAction(run.request.op.kind === "assign_task" ? `task:${String(run.request.op.task_id)}` : run.request.op.kind === "focus_epic" ? "header" : `agent:${String(run.request.op.worker)}`, run); },
      dismissNotice: () => rerender(progressSheetOpen() ? "header:add" : "phone-notice-dismiss"),
    },
  };
}

function placeFleetUndo(): void {
  const region = document.getElementById("fleet-ops-announcer");
  if (region) (progressSheetOpen() ? document.querySelector(".conversation-context") ?? document.body : document.body).append(region);
  const undo = document.getElementById("fleet-phone-undo");
  if (!undo) return;
  // Reserve its actual height above the composer, or after the sheet's tasks.
  // Retained requests still use inline feedback in Terminal view.
  if (!phoneLayout() || hubPresentation !== "conversation") { undo.remove(); return; }
  undo.hidden = attentionSheetOpen;
  const rail = progressSheetOpen() ? document.querySelector(".conversation-context") : null;
  const parent = rail ?? document.getElementById("conversation-composer-slot");
  // Re-inserting even into the same parent drops a descendant's keyboard focus.
  if (parent && undo.parentElement !== parent) {
    if (rail) parent.append(undo); else parent.prepend(undo);
  }
}
function dismissFleetPanel(redraw = true): void {
  const row = fleetOps.confirm?.rowKey ?? fleetOps.menuFor ?? fleetOps.assignFor ?? fleetOps.preview?.rowKey;
  const opener = row ? phoneLayout() || row.startsWith("agent:") ? `${row}:trigger` : `${row}:${fleetOps.assignFor ? "assign" : "ask"}` : fleetHeaderPanel ? `header:${fleetHeaderPanel}` : undefined;
  fleetOps.closeMenus(); fleetOps.cancelConfirm(); fleetHeaderPanel = undefined;
  if (redraw) { fleetFocusNext = opener; renderStatus(selectedMachineId && selectedSession ? statuses.get(sessionKey(selectedMachineId, selectedSession)) : undefined); }
}

function fleetMergeAction(task: FleetTask): FleetAction {
  return requestMergeActionFor(task);
}

/** What the rail shows, for skipping a rebuild that would change nothing. */
function fleetOpsSignature(): string {
  return JSON.stringify([fleetOps.menuFor, fleetOps.confirm?.action.id, fleetOps.confirm?.rowKey, fleetOps.preview?.rowKey, fleetOps.assignFor, [...fleetOps.pending].map(([key, action]) => [key, action.id]), [...fleetOps.notes], fleetOps.currentUndo(Date.now())?.label, fleetHeaderPanel, [...fleetAsked].map(([id, at]) => [id, relativeTimestamp(at)])]);
}

function renderStatus(status?: Record<string, unknown>): void {
  const container = document.querySelector<HTMLElement>("#status-view");
  if (!container) return;
  const machine = selectedMachineId ? machines.get(selectedMachineId) : undefined;
  const signature = JSON.stringify([phoneLayout(), selectedMachineId, selectedSession, status ?? null, machine?.scopes ?? null, selectedMachineId && selectedSession ? sessionSummaries.get(sessionKey(selectedMachineId, selectedSession)) ?? null : null, statusPending.size, fleetOpsSignature()]);
  if (container.dataset.signature === signature && container.isConnected && fleetFocusNext === undefined) return;
  container.dataset.signature = signature;
  // A shell replacement preserves this selection's in-flight request ownership.
  syncFleetSelection();
  container.onkeydown = (event) => {
    if (event.key !== "Escape" || !container.querySelector(".fleet-ops-menu, .fleet-ops-confirm, .fleet-ops-preview, .fleet-ops-panel")) return;
    event.preventDefault(); event.stopPropagation(); dismissFleetPanel();
  };
  const undoFocused = document.activeElement instanceof HTMLElement && document.activeElement.dataset.fleetFocus === "undo";
  const hadFocus = document.activeElement instanceof HTMLElement && (container.contains(document.activeElement) || document.activeElement.closest("#fleet-phone-undo")) ? document.activeElement.dataset.fleetFocus : undefined;
  // The rows' triggers in their drawn order, so focus on a row that leaves
  // (a confirmed Stop) can move to its neighbour (cas-a474 QA N1).
  const priorTriggers = [...container.querySelectorAll<HTMLElement>('[data-fleet-focus$=":trigger"]')].map((node) => node.dataset.fleetFocus ?? "");
  // Region updates run against a container the shell rebuild is no longer
  // clearing for them, so this owns its own emptying.
  container.replaceChildren();
  contextProgress = false;
  const restoreFocus = (): void => {
    const want = fleetFocusNext ?? (undoFocused ? "undo" : hadFocus);
    fleetFocusNext = undefined;
    if (!want) return;
    const target = want.endsWith(":first-item")
      ? container.querySelector<HTMLElement>(`[data-fleet-focus^="${CSS.escape(want.replace(/:first-item$/, ""))}:item:"]`)
      : [...(phoneLayout() ? document : container).querySelectorAll<HTMLElement>(`[data-fleet-focus="${CSS.escape(want)}"]`)].find((node) => node.getClientRects().length > 0);
    if (target) { target.focus({ preventScroll: false }); return; }
    if (want === "undo") {
      const fallback = progressSheetOpen() || !phoneLayout() ? container.querySelector<HTMLElement>('[data-fleet-focus="header:add"]') : document.querySelector<HTMLElement>("#conversation-fleet");
      fallback?.focus(); return;
    }
    if (phoneLayout() && !progressSheetOpen()) { document.querySelector<HTMLElement>("#conversation-fleet")?.focus(); return; }
    // Its row is gone: the next row's ⋯, else the list itself.
    const at = priorTriggers.indexOf(`${want.slice(0, want.lastIndexOf(":"))}:trigger`);
    if (at < 0) return;
    const alive = (key: string) => container.querySelector<HTMLElement>(`[data-fleet-focus="${CSS.escape(key)}"]`);
    const neighbour = priorTriggers.slice(at + 1).map(alive).find(Boolean);
    if (neighbour) { neighbour.focus({ preventScroll: false }); return; }
    container.tabIndex = -1;
    container.focus({ preventScroll: false });
  };
  if (!status) {
    container.textContent = selectedSession ? "Waiting for project status…" : "Open a session for project status.";
    // cas-813a: while this session's status is on its way, the rail is open
    // already, so its arrival doesn't narrow the composer.
    contextProgress = Boolean(selectedMachineId && selectedSession && statusPending.has(sessionKey(selectedMachineId, selectedSession)));
    return;
  }
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
  const ops = fleetOpsContext(status);
  if (ops) {
    fleetAnnouncer();
    document.getElementById("fleet-phone-undo")?.remove();
    const phoneConversation = phoneLayout() && hubPresentation === "conversation";
    const undo = fleetOps.currentUndo(ops.now) ? undoBar(document, { ...ops, phone: phoneConversation }) : phoneConversation ? phoneFleetNotice(document, ops) : undefined;
    if (undo && phoneConversation) { undo.id = "fleet-phone-undo"; document.body.append(undo); placeFleetUndo(); }
    else if (undo) container.append(undo);
    container.append(headerControls(document, ops, fleetHeaderPanel));
  }
  if (agents.length > 0) container.append(sectionLabel("Agents", agents.length));
  for (const agent of agents) {
    const row = document.createElement("article"); row.className = "status-row status-agent";
    const line = document.createElement("div"); line.className = "status-line";
    line.append(identifier(agent.name), chip(agent.status));
    if (agent.current_task) line.append(identifier(agent.current_task));
    row.append(line);
    if (ops && agent.name && String(agent.role ?? "").toLowerCase() !== "supervisor") row.append(agentControls(document, ops, agent as FleetAgent));
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
    const controls = ops ? taskControls(document, ops, task as FleetTask) : undefined;
    if (controls) row.append(controls);
    container.append(row);
  }
  contextProgress = Boolean(summary) || agents.length > 0 || tasks.length > 0;
  if (!contextProgress) {
    const empty = document.createElement("p");
    empty.className = "status-empty";
    empty.textContent = "No agents or tasks reported for this session yet.";
    container.append(empty);
  }
  if (phoneLayout()) presentFleetSheet(container, dismissFleetPanel);
  restoreFocus();
}

async function toggleControl(selected: StoredMachine | undefined, lease: LeaseState | undefined): Promise<void> {
  if (!selected || !selectedSession) return;
  if (lease?.held_by_me) {
    await connections.get(selected.id)?.releaseLease(selectedSession);
    invalidateMachineLeases(selected.id, "released");
  } else {
    await connections.get(selected.id)?.requestControl(selectedSession, Boolean(lease?.controller_label && selected.scopes.includes("hub-admin")));
  }
  await loadLease(selected.id, selectedSession);
  noteControlTaken(selected.id, selectedSession);
  updateConversationViews();
}

/** `event` is the toggle's click; Ctrl/Cmd+K passes none (cas-990d). */
function openCommandPalette(event?: MouseEvent): void {
  const wasOpen = document.querySelector<HTMLDialogElement>("#command-palette")?.open === true;
  if (!wasOpen) commandPaletteOpenedByTouch = touchActivation(event);
  commandPaletteOpen = true;
  render();
  // Closing a dialog does not rebuild the shell. Reopening can therefore have
  // the same shell signature; open the existing dialog in that case too.
  const palette = document.querySelector<HTMLDialogElement>("#command-palette");
  if (palette && !palette.open) palette.showModal();
  // That reused dialog still carries the last filter, its hidden rows and the
  // no-match line. Every fresh open starts from the full list; Ctrl+K on an
  // already open palette keeps what is being typed.
  const query = document.querySelector<HTMLInputElement>("#command-palette-query");
  if (query && !wasOpen && query.value) {
    query.value = "";
    query.dispatchEvent(new Event("input"));
  }
  query?.focus();
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
    // The list's search field owns the shortcut wherever it is on screen
    // (journey F8); from the field itself, a second press opens the palette.
    const search = document.querySelector<HTMLInputElement>("#conversation-search");
    if (search && !commandPaletteOpen && document.activeElement !== search && search.getClientRects().length > 0) {
      search.focus();
      search.select();
      return;
    }
    openCommandPalette();
    return;
  }
  if (command && event.key.toLowerCase() === "f" && hubPresentation === "terminal" && activePaneContext()) {
    event.preventDefault();
    event.stopPropagation();
    openTerminalSearch();
    return;
  }
  if (command && hubPresentation === "terminal" && /^[1-9]$/.test(event.key)) {
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
  const attentionBadge = document.querySelector<HTMLButtonElement>("#conversation-attention");
  if (attentionBadge) attentionBadge.onclick = openAttentionSheet;
  const sheetClose = document.querySelector<HTMLButtonElement>(".conversation-context .context-sheet-close");
  if (sheetClose) sheetClose.onclick = closeContextSheet;
  const fleetEntry = document.querySelector<HTMLButtonElement>("#conversation-fleet");
  if (fleetEntry) fleetEntry.onclick = openProgressSheet;
  const conversationBack = document.querySelector<HTMLButtonElement>("#conversation-back");
  if (conversationBack) conversationBack.onclick = () => {
    attentionSheetOpen = false;
    // cas-f50f: back to the row that was open, not the list's first row.
    const opened = selectedMachineId && selectedSession ? sessionKey(selectedMachineId, selectedSession) : undefined;
    if (selectedMachineId) commitSelection({ machineId: selectedMachineId });
    render();
    queueMicrotask(() => returnToConversationRow(opened));
  };
  const terminal = document.querySelector<HTMLButtonElement>("#conversation-terminal");
  if (terminal) terminal.onclick = (event) => {
    const unavailable = terminal.dataset.disabledReason;
    if (unavailable) { toast(unavailable); return; }
    attentionSheetOpen = false; hubPresentation = "terminal"; const storage = paneLayoutStorage(); if (storage && selectedMachineId && selectedSession) saveTranscriptView(storage, sessionKey(selectedMachineId, selectedSession), "terminal"); render(); landInTerminalView(event); };
  const returning = document.querySelector<HTMLButtonElement>("#conversation-return");
  if (returning) returning.onclick = (event) => {
    hubPresentation = "conversation";
    render();
    // Back from the terminal workspace, land where the next keystroke belongs,
    // as a palette jump does (cas-9648): the reply box for the keyboard (Enter
    // arrives as a click with detail 0) and a mouse. A touch tap would raise a
    // soft keyboard over the thread, so it lands on the thread to read instead.
    // Either way, never on <body> (cas-cf8e).
    landFocus(touchActivation(event) ? [focusTargets.thread] : [focusTargets.composer, focusTargets.thread]);
  };
  // Phone compose FAB: open the thread that is waiting on the operator, else
  // the first one, and land in its composer; with nothing paired, pair.
  const compose = document.querySelector<HTMLButtonElement>("#compose-fab");
  if (compose) compose.onclick = () => {
    const row = conversationRows.find((candidate) => candidate.attention > 0) ?? conversationRows[0];
    if (!row) { openPairDialog(); return; }
    void openSession(row.machineId, row.session).then(() => queueMicrotask(() => document.querySelector<HTMLTextAreaElement>("#message-text")?.focus()));
  };
  // The list search (journey F8): typing filters rows by project, machine or
  // supervisor in place; Enter opens the leading match, ArrowDown steps onto
  // the rows, Escape clears.
  const search = document.querySelector<HTMLInputElement>("#conversation-search");
  if (search) {
    search.oninput = () => { conversationSearchQuery = search.value; renderConversationList(); };
    search.onkeydown = (event) => {
      if (event.key === "Escape" && search.value) {
        event.preventDefault();
        event.stopPropagation();
        search.value = "";
        conversationSearchQuery = "";
        renderConversationList();
        return;
      }
      if (event.key === "ArrowDown") {
        const first = document.querySelector<HTMLButtonElement>("#conversation-list .conversation-row");
        if (first) { event.preventDefault(); first.focus(); }
        return;
      }
      if (event.key !== "Enter" || event.isComposing) return;
      // cas-537f: Enter opens the marked row, the first on screen (grouped
      // order), and only for a query: an empty field opens nothing.
      const list = document.querySelector<HTMLElement>("#conversation-list");
      const target = list ? markConversationEnterTarget(list) : undefined;
      const row = target ? conversationRows.find((candidate) => candidate.key === target.dataset.threadKey) : undefined;
      if (!row) return;
      event.preventDefault();
      // The search was a jump: the full list is back for the next visit, and
      // the field lets go of focus first, because render() defers the shell
      // rebuild while an editable field in #app is focused.
      conversationSearchQuery = "";
      search.value = "";
      search.blur();
      const opened = openSession(row.machineId, row.session);
      // A keyboard lands in the reply box, as a palette jump does; a phone's
      // soft-keyboard Enter lands to read, with no keyboard raised again.
      if (window.matchMedia("(pointer: fine)").matches) focusJumpedComposer(opened);
    };
  }

  const paletteToggle = document.querySelector<HTMLButtonElement>("#command-palette-toggle");
  if (paletteToggle) paletteToggle.onclick = openCommandPalette;
  const palette = document.querySelector<HTMLDialogElement>("#command-palette")!;
  const closePalette = () => {
    commandPaletteOpen = false;
    palette.close();
    (paletteToggle ?? document.querySelector<HTMLButtonElement>("#session-picker-toggle"))?.focus();
  };
  document.querySelector<HTMLButtonElement>("#command-palette-close")!.onclick = closePalette;
  palette.oncancel = () => { commandPaletteOpen = false; };
  // Any other close settles the same flag, so a command that closes the
  // palette can never leave render() to reopen it (cas-dfc8). A dialog a
  // shell rebuild replaced is detached and must not reset it.
  palette.onclose = () => { if (palette.isConnected && !palette.open) commandPaletteOpen = false; };
  const paletteQuery = document.querySelector<HTMLInputElement>("#command-palette-query")!;
  const paletteAdvanced = palette.querySelector<HTMLDetailsElement>(".palette-advanced");
  // Commands in order, grouped Conversations / This conversation / Machines /
  // Appearance / Advanced (3.30.0 journey F4: lease and machine commands are
  // not conversations, and an empty "Dismiss all info" is not offered). A row
  // inside the collapsed Advanced group is not on screen, so Enter and
  // ArrowDown never pick it.
  const paletteRowShown = (command: HTMLElement) => !command.hidden && command.closest("details:not([open])") === null;
  paletteQuery.oninput = () => {
    const query = paletteQuery.value.trim().toLocaleLowerCase();
    // Every word must match somewhere in the row, as in the list search:
    // "gabber studio" finds the gabber-studio session on Studio Mac.
    const words = query.split(/\s+/).filter(Boolean);
    for (const command of palette.querySelectorAll<HTMLElement>(".palette-command")) {
      const searchable = `${command.textContent ?? ""} ${command.dataset.searchText ?? ""}`.toLocaleLowerCase();
      command.hidden = words.length > 0 && !words.every((word) => searchable.includes(word));
    }
    // A group with nothing left to offer steps aside, heading and all.
    for (const group of palette.querySelectorAll<HTMLElement>(".palette-group")) {
      group.hidden = query.length > 0 && ![...group.querySelectorAll<HTMLElement>(".palette-command")].some((command) => !command.hidden);
    }
    // A query that names an Advanced command opens the group to show it; the
    // group folds again once the query no longer needs it.
    if (paletteAdvanced) {
      const wanted = query.length > 0 && !paletteAdvanced.hidden;
      if (wanted && !paletteAdvanced.open) { paletteAdvanced.open = true; paletteAdvanced.dataset.autoOpened = "true"; }
      else if (!wanted && paletteAdvanced.dataset.autoOpened) { paletteAdvanced.open = false; delete paletteAdvanced.dataset.autoOpened; }
    }
    markPaletteEnterTarget();
    const noMatch = palette.querySelector<HTMLElement>("#palette-no-match");
    if (noMatch) {
      const anyVisible = [...palette.querySelectorAll<HTMLElement>(".palette-command")].some((command) => !command.hidden);
      noMatch.hidden = query.length === 0 || anyVisible;
      noMatch.textContent = noMatch.hidden ? "" : `No commands or conversations match “${paletteQuery.value.trim()}”.`;
    }
  };
  /**
   * The command Enter in the filter runs (cas-537f): the first one on screen,
   * or with no filter the next conversation that needs the operator, never
   * the one already open (cas-786a; see paletteEnterTarget). It is marked for
   * the eye and named as the filter's active descendant, so "light" visibly
   * lands on "Jump to lighthouse", not "Appearance · Light".
   */
  const markPaletteEnterTarget = (): HTMLButtonElement | undefined => {
    const commands = [...palette.querySelectorAll<HTMLButtonElement>(".palette-command")];
    // An unread reply or a new wait does not rebuild the shell, so the palette
    // re-reads them from the live list each time it picks (cas-786a).
    for (const command of commands) {
      const { paletteMachine, paletteSession } = command.dataset;
      if (paletteMachine === undefined || paletteSession === undefined) continue;
      if (conversationNeedsYou(paletteMachine, paletteSession)) command.dataset.paletteNeedsYou = "true"; else delete command.dataset.paletteNeedsYou;
    }
    const first = paletteEnterTarget(commands.filter((command) => paletteRowShown(command) && !command.disabled), paletteQuery.value);
    commands.forEach((command, index) => {
      if (!command.id) command.id = `palette-command-${index}`;
      if (command === first) command.dataset.enterTarget = "true"; else delete command.dataset.enterTarget;
    });
    if (first) paletteQuery.setAttribute("aria-activedescendant", first.id); else paletteQuery.removeAttribute("aria-activedescendant");
    return first;
  };
  markPaletteEnterTarget();
  refreshPaletteEnterTarget = () => { if (palette.isConnected) markPaletteEnterTarget(); };
  if (paletteAdvanced) paletteAdvanced.ontoggle = () => { if (!paletteAdvanced.open) delete paletteAdvanced.dataset.autoOpened; markPaletteEnterTarget(); };
  paletteQuery.onkeydown = (event) => {
    if (event.key !== "ArrowDown" && event.key !== "Enter") return;
    // Session "Jump to" rows lead the Conversations group, so a query that
    // names a session lands Enter and ArrowDown on the conversation.
    const first = markPaletteEnterTarget();
    if (!first) return;
    event.preventDefault();
    if (event.key === "Enter") first.click();
    else first.focus();
  };
  for (const command of palette.querySelectorAll<HTMLButtonElement>("[data-palette-machine]")) {
    command.onclick = (event) => {
      // Close the dialog itself, not just the flag: Enter in the filter
      // clicks this row with focus still in the input, and render() defers
      // the shell rebuild while an editable field inside #app has focus, so
      // the modal would stay up over the session it just opened. The toggle
      // is not refocused — the opened conversation's composer takes focus.
      commandPaletteOpen = false;
      palette.close();
      // close() hands focus back to whatever held it before the palette
      // opened. When that was the composer (Ctrl+K mid-draft), render() would
      // defer again, so release it before the session switch renders.
      const restored = document.activeElement;
      if (restored instanceof HTMLElement && isEditableElement(restored) && app.contains(restored)) restored.blur();
      const machineId = command.dataset.paletteMachine;
      const session = command.dataset.paletteSession;
      if (!machineId || !session) return;
      const opened = openSession(machineId, session);
      // openSession paints the conversation before its first await, so its
      // composer exists now. Land there: a jump from the keyboard ends where
      // the next keystroke belongs, not on <body>. A touch or pen tap is the
      // exception, as with the session picker on phones: it would raise a soft
      // keyboard over the conversation just opened, so the operator lands to
      // read it, exactly as a tap on a list row does. Keyboard activation
      // (Enter reaches this handler as a click with detail 0) and fine-pointer
      // clicks land in the reply box.
      if (event.detail > 0 && !finePointerClick(event)) return;
      // Enter in a filter the operator opened by touch came from a soft
      // keyboard, which is still up. Land to read, as a tap does, so it goes
      // away rather than staying over the conversation (cas-990d). A palette
      // opened by Ctrl/Cmd+K or a mouse has a hardware keyboard: reply box.
      if (commandPaletteOpenedByTouch) {
        if (document.activeElement instanceof HTMLElement && isEditableElement(document.activeElement)) document.activeElement.blur();
        landFocus([focusTargets.thread, focusTargets.sessionTitle], { keep: true, nextTask: true, waitMs: 2_000 });
        return;
      }
      focusJumpedComposer(opened);
    };
  }
  markAppearanceCommands(palette);
  for (const command of palette.querySelectorAll<HTMLButtonElement>("[data-palette-scheme]")) {
    command.onclick = () => { setScheme(command.dataset.paletteScheme as SchemePreference); markAppearanceCommands(palette); closePalette(); };
  }
  const paletteTerminal = palette.querySelector<HTMLButtonElement>("[data-palette-action=terminal-view]");
  if (paletteTerminal) paletteTerminal.onclick = (event) => { closePalette(); hubPresentation = "terminal"; render(); landInTerminalView(event); };
  const paletteWorkers = palette.querySelector<HTMLButtonElement>("[data-palette-action='workers']");
  if (paletteWorkers) paletteWorkers.onclick = () => { closePalette(); setWorkersRevealed(!revealWorkers); };
  const paletteDormant = palette.querySelector<HTMLButtonElement>("[data-palette-action='dormant']");
  if (paletteDormant) paletteDormant.onclick = () => { closePalette(); setDormantRevealed(!revealDormant); };
  const paletteControl = palette.querySelector<HTMLButtonElement>("[data-palette-action='control']");
  if (paletteControl) paletteControl.onclick = () => { closePalette(); void toggleControl(selected, lease); };
  const paletteLaunch = palette.querySelector<HTMLButtonElement>("[data-palette-action='new-session']");
  if (paletteLaunch) paletteLaunch.onclick = () => { closePalette(); launchSheet.open(); };
  const newSession = document.querySelector<HTMLButtonElement>("#new-session-toggle");
  if (newSession) newSession.onclick = () => launchSheet.open();
  const paletteDismiss = palette.querySelector<HTMLButtonElement>("[data-palette-action='dismiss-info']");
  if (paletteDismiss) paletteDismiss.onclick = () => { closePalette(); void acknowledgeAttentionGroup(dismissableInfoItems(attention)); };
  if (document.querySelector<HTMLButtonElement>("#session-picker-toggle")) document.querySelector<HTMLButtonElement>("#session-picker-toggle")!.onclick = openSessionPicker;
  const back = document.querySelector<HTMLButtonElement>("#session-back");
  if (back) back.onclick = goBack;
  const picker = document.querySelector<HTMLDialogElement>("#session-picker")!;
  picker.oncancel = () => sessionPickerClosed(true);
  // Any other close (a form, a browser close request) settles the same state.
  // A dialog replaced by a shell rebuild is detached and must not reset it.
  picker.onclose = () => { if (picker.isConnected && !picker.open) sessionPickerClosed(false); };
  document.querySelector<HTMLButtonElement>("#session-picker-close")!.onclick = () => closeSessionPicker(true);
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
    // A filter that hides every session says so, as the palette does, rather
    // than leaving an empty dialog (journey F18). With no sessions at all the
    // list carries its own empty line.
    const noMatch = picker.querySelector<HTMLElement>("#session-picker-no-match");
    if (noMatch) {
      const entries = [...picker.querySelectorAll<HTMLElement>(".session-picker-entry")];
      noMatch.hidden = query.length === 0 || entries.length === 0 || entries.some((entry) => !entry.hidden);
      noMatch.textContent = noMatch.hidden ? "" : `No sessions match “${pickerQuery.value.trim()}”.`;
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
  if (document.querySelector<HTMLButtonElement>("#pair-toggle")) document.querySelector<HTMLButtonElement>("#pair-toggle")!.onclick = () => (document.querySelector<HTMLDialogElement>("#pair-dialog")!).showModal();
  if (document.querySelector<HTMLButtonElement>("#machine-drawer-toggle")) document.querySelector<HTMLButtonElement>("#machine-drawer-toggle")!.onclick = () => { machineDrawerOpen = !machineDrawerOpen; render(); };
  if (document.querySelector<HTMLButtonElement>("#machine-drawer-close")) document.querySelector<HTMLButtonElement>("#machine-drawer-close")!.onclick = () => { machineDrawerOpen = false; render(); document.querySelector<HTMLButtonElement>("#machine-drawer-toggle")?.focus(); };
  const openMachines = document.querySelector<HTMLButtonElement>("#open-machines");
  if (openMachines) openMachines.onclick = () => { machineDrawerOpen = true; render(); };
  for (const pair of document.querySelectorAll<HTMLButtonElement>("#empty-pair, #drawer-pair")) {
    pair.onclick = () => document.querySelector<HTMLDialogElement>("#pair-dialog")!.showModal();
  }
  if (document.querySelector<HTMLButtonElement>("#attention-panel-toggle")) document.querySelector<HTMLButtonElement>("#attention-panel-toggle")!.onclick = () => { attentionPanelCollapsed = !attentionPanelCollapsed; render(); };
  if (document.querySelector<HTMLButtonElement>("#context-panel-close")) document.querySelector<HTMLButtonElement>("#context-panel-close")!.onclick = () => { attentionPanelCollapsed = true; render(); };
  if (document.querySelector<HTMLButtonElement>("#mobile-message-toggle")) document.querySelector<HTMLButtonElement>("#mobile-message-toggle")!.onclick = openSupervisorComposer;
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
  // cas-093d F02: the re-pair command's Copy, announced inside the dialog
  // (a toast outside a modal dialog is not read).
  for (const copy of document.querySelectorAll<HTMLButtonElement>("#pair-dialog .pair-command-copy")) {
    copy.onclick = () => {
      const status = copy.parentElement?.querySelector<HTMLElement>(".pair-command-status");
      // QA F03: the status is emptied first, so a second copy is announced again.
      const say = (text: string) => { if (!status) return; status.textContent = ""; window.setTimeout(() => { status.textContent = text; }, 50); };
      void navigator.clipboard.writeText(copy.dataset.pairCommand ?? "")
        .then(() => { copy.textContent = "Copied"; say("Command copied"); })
        .catch(() => { copy.textContent = "Copy failed"; say("Copy failed — select the command and copy it"); })
        .finally(() => { window.setTimeout(() => { copy.textContent = "Copy command"; }, 2000); });
    };
  }
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
  const usePageOrigin = document.querySelector<HTMLButtonElement>("#pair-use-page-origin");
  if (usePageOrigin) usePageOrigin.onclick = () => {
    const url = document.querySelector<HTMLInputElement>('#pair-form input[name="url"]');
    if (!url) return;
    url.value = usePageOrigin.dataset.pageOrigin ?? "";
    pairingDraft = { ...pairingDraft, hubUrl: url.value };
    url.focus();
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
    void pairMachine(pairForm).then((installed) => {
      if (!installed) return;
      // Saved and connected are announced at the installation seam inside
      // pairMachine; the handler only closes the dialog.
      document.querySelector<HTMLDialogElement>("#pair-dialog")?.close();
      // cas-71af (dfb2 QA F02): the dialog hands focus back to a Pair a
      // machine button the render already replaced, so it fell to <body>.
      // Land it on the conversation a phone opens for the new machine, else
      // on Pair a machine where the operator started; a thread, not its reply
      // box, so a phone raises no keyboard over it.
      // Only focus that is lost is re-landed: anything the operator focuses
      // meanwhile is theirs.
      const deadline = Date.now() + 3_000;
      const reland = (): void => {
        const active = document.activeElement;
        if (!active || active === document.body || !active.isConnected || active.closest("dialog:not([open])")) {
          for (const target of [focusTargets.thread, () => document.querySelector<HTMLElement>("#pair-toggle")]) {
            const element = target();
            if (element?.isConnected && element.getClientRects().length > 0) { element.focus(); break; }
          }
        }
        if (Date.now() < deadline) requestAnimationFrame(reland);
      };
      window.setTimeout(reland, 0);
    }).catch((error) => {
      // A pairing failure is stated inside the dialog beside Pair; a toast
      // behind the backdrop only duplicated it. Anything else still surfaces.
      if (error instanceof PairingExchangeError) return;
      toast(error instanceof Error ? error.message : "Pairing failed");
    });
  };
  const remove = document.querySelector<HTMLButtonElement>("#remove-machine");
  if (remove && selected) remove.onclick = () => { void forgetPairedMachine(selected.id); };
  const explainIfUnavailable = (button: HTMLButtonElement): boolean => {
    const reason = button.dataset.disabledReason;
    if (!reason) return false;
    // cas-71af (6929 QA F01): when the reason is already on screen under the
    // header, a toast only repeats it a third time. The click calls attention
    // to the sentence that is there instead.
    // During an outage the line holds only the reason's second half; the
    // banner holds the first (journey F42).
    const notice = document.querySelector<HTMLElement>("#session-controls-reason");
    const shown = notice?.textContent?.trim() ?? "";
    if (notice && !notice.hidden && shown && (shown.includes(reason) || reason.endsWith(shown))) {
      notice.classList.remove("called");
      void notice.offsetWidth;
      notice.classList.add("called");
      window.setTimeout(() => notice.classList.remove("called"), 1_200);
      return true;
    }
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
  if (document.querySelector<HTMLButtonElement>("#message-send")) document.querySelector<HTMLButtonElement>("#message-send")!.onclick = () => { void submitSupervisorMessage(); };
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
    // Arrowing from the palette filter onto its rows is still one palette
    // interaction: a rebuild here would replace the dialog, wipe the filter
    // and leave Enter to run whatever row now leads (cas-9648). The owed
    // rebuild runs when focus leaves the palette. Likewise the session picker.
    if (active instanceof HTMLElement && active.closest("#command-palette[open], #session-picker[open]")) return;
    deferredRender.focusLeft();
  });
});

// A tap on any artifact (the thread's sheet, the context rail, the operator
// thread) opens the hosted copy through a signed view URL (cassy issue 910).
app.addEventListener("click", (event) => {
  const link = artifactLinkFor(event.target);
  const artifactId = artifactIdFromHref(link?.getAttribute("href"));
  if (!link || !artifactId || event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
  event.preventDefault();
  const machineId = selectedMachineId;
  const session = selectedSession;
  const connection = machineId ? connections.get(machineId) : undefined;
  if (!machineId || !session || !connection) {
    toast("Open the conversation this file came from to view it.");
    return;
  }
  // Journey F6: a file the machine said never left it opens no tab again,
  // and every outcome is said on the card that was pressed.
  const localKey = `${machineId}:${artifactId}`;
  const onCard = link.matches("a.sheet");
  const machineLabel = machines.get(machineId)?.label ?? "that machine";
  void openArtifact({
    fetchView: () => connection.artifactView(session, artifactId),
    openWindow: () => window.open("about:blank", "_blank"),
    notify: (message, result) => {
      if (artifactIsLocalOnly(result)) localOnlyArtifacts.add(localKey);
      else if (result?.ok) localOnlyArtifacts.delete(localKey);
      // Journey F28: a note about reaching the machine follows the connection
      // the header shows, rather than the one it had at the click.
      const restate = artifactFailureFollowsConnection(result) ? () => artifactOpenFailure(result, machineLabel, artifactMachineReach(machineId)) : undefined;
      if (onCard && setAttachmentNote(document, artifactId, message, { machineId, transient: !artifactFailureIsAboutTheFile(result), ...(restate ? { restate } : {}) }) > 0) return;
      toast(message);
    },
    machineLabel,
    machineLive: () => artifactMachineReach(machineId),
    knownLocalOnly: localOnlyArtifacts.has(localKey),
    fileName: link.querySelector(".fname")?.textContent?.trim() || undefined,
  }).then((opened) => { if (opened) setAttachmentNote(document, artifactId, undefined); });
});
/** Files a machine said were never uploaded to Cloud, by machine and artifact (journey F6). */
const localOnlyArtifacts = new Set<string>();

window.addEventListener("keydown", globalShortcut, true);
// The conversation header's machine · codename line is fitted to its room
// (cas-71af): a width change can make the machine name step aside or return.
window.addEventListener("resize", () => fitConversationHost(document), { passive: true });
// Rotation changes the layout in CSS instantly, but which panes mount a
// terminal, whether the worker strip is collapsed and the PTY column floor are
// all decided in JS at render time. Without this, a phone turned on its side
// kept the composition it was mounted with until some hub event happened to
// redraw it.
for (const query of [PHONE_MEDIA_QUERY, COMPACT_MEDIA_QUERY]) {
  window.matchMedia(query).addEventListener("change", () => render());
}
// The search placeholder's Ctrl K hint follows the device too, without
// waiting for a shell rebuild (journey F10).
window.matchMedia(KEYBOARD_HINT_MEDIA_QUERY).addEventListener("change", () => {
  const search = document.querySelector<HTMLInputElement>("#conversation-search");
  if (search) search.placeholder = conversationSearchPlaceholder(keyboardHintOffered());
});
render(false);
void boot();

function pairedMachineRows(): PairedMachineRow[] {
  // cas-0739 (journey F10): machines that aren't connected come first.
  return orderPairedMachines([...machines.values()].map(machine => {
    const state = machineFooterConnection(machine.id);
    const updated = fleetCatalogUpdatedAt.get(machine.id);
    const fresh = Date.now() < (catalogExpiresAt.get(machine.id) ?? Infinity);
    return { id: machine.id, label: machine.label, address: new URL(machine.baseUrl).host,
      connection: state?.phase === "live" && !state.degraded && fresh ? "Connected" : fleetConnectionLabel(state, machine.id) === "Live" ? "Reconnecting" : fleetConnectionLabel(state, machine.id),
      connected: state?.phase === "live" && !state.degraded && fresh,
      everConnected: lastLiveAt.has(machine.id),
      connectionState: state,
      lastSeen: updated ? `Last seen ${relativeTimestamp(Date.parse(updated))} · ${clockLabel(Date.parse(updated))}` : 'Not yet seen in this visit',
      runtime: machineInfo.get(machine.id)?.version,
      // cas-d382: what this pairing may do to the fleet, and how to get the rest.
      fleet: { operate: fleetControlGate(machine.scopes, "add-workers", location.origin), manage: fleetControlGate(machine.scopes, "stop-worker", location.origin) } };
  }));
}

/**
 * The one-time "Allow managing workers" grant (cas-d382), as session launch
 * is allowed: the hub adds factory-operate to this device's credential.
 * A refusal is said in the register, beside the machine.
 */
async function allowManagingWorkers(machineId: string): Promise<void> {
  const error = document.querySelector<HTMLElement>("#paired-machines-error");
  try {
    await launchConnection(machineId).enableFactoryOperate();
    if (error) { error.hidden = true; error.textContent = ""; }
  } catch (failure) {
    if (error) { error.textContent = failure instanceof Error ? failure.message : "Could not allow managing workers."; error.hidden = false; }
  }
  renderMachineRegister();
}

function renderMachineRegister(): void {
  const rows = pairedMachineRows();
  const footer = document.querySelector<HTMLElement>('#hub-footer-badges');
  if (footer) {
    const markup = machineFooterMarkup(rows, [...machines.keys()].reduce((sum, id) => sum + visibleSessions(id).filter(session => supervisorTarget(session)).length, 0), __HUB_BUILD__, !machineCatalogLoaded);
    // Preserve the opener itself: dialog Escape must return focus after a catalog tick.
    const button = footer.querySelector<HTMLButtonElement>('#paired-machines-toggle');
    if (!button) footer.innerHTML = markup;
    else {
      const template = document.createElement('template'); template.innerHTML = markup;
      const nextButton = template.content.querySelector('button')!;
      if (button.innerHTML !== nextButton.innerHTML) button.innerHTML = nextButton.innerHTML;
      const meta = footer.querySelector('.hub-footer-meta')!;
      const nextMeta = template.content.querySelector('.hub-footer-meta')!;
      if (meta.innerHTML !== nextMeta.innerHTML) meta.innerHTML = nextMeta.innerHTML;
    }
  }
  const dialog = document.querySelector<HTMLDialogElement>('#paired-machines-dialog');
  if (!dialog) return;
  const list = dialog.querySelector<HTMLElement>('#paired-machines-list')!;
  // cas-0739: a closed register is kept in order (not connected first) and
  // scrolled to its top, so it opens on the machine the footer names; an open
  // one keeps its order so a status tick never moves a row under the operator.
  renderPairedMachines(list, rows, forgetPairedMachine, {
    reorder: !dialog.open,
    copy: (text) => navigator.clipboard.writeText(text),
    allowManagingWorkers: allowManagingWorkers,
  });
  if (!dialog.open) { list.scrollTop = 0; dialog.scrollTop = 0; }
  // Paired machines replaces the palette: clear its open flag too, or the
  // next render reopens it over whatever the operator opens next (cas-dfc8).
  const open = (opener: string) => { pairedMachinesOpener = opener; commandPaletteOpen = false; document.querySelector<HTMLDialogElement>('#command-palette')?.close(); dialog.showModal(); };
  for (const id of ['paired-machines-toggle', 'palette-paired-machines']) {
    const button = document.getElementById(id); if (button) button.onclick = () => open(id);
  }
  document.getElementById('paired-machines-close')!.onclick = () => dialog.close();
  document.getElementById('paired-machines-add')!.onclick = () => { dialog.close(); document.querySelector<HTMLDialogElement>('#pair-dialog')?.showModal(); };
}

async function forgetPairedMachine(id: string): Promise<void> {
  const error = document.getElementById('paired-machines-error');
  if (error) error.hidden = true;
  try { await catalog.remove(id); }
  catch { if (error) { error.hidden = false; error.textContent = 'Could not remove this pairing. Try again.'; } else toast('Could not remove this pairing. Try again.'); return; }
  connections.get(id)?.stop(); firstConnections.forget(id);
  connections.delete(id); machines.delete(id); sessions.delete(id);
  // cas-093d: a removed machine has no start-sessions permission to re-allow.
  if (launchDroppedMachines.delete(id)) saveLaunchDropped(launchDroppedStorage, launchDroppedMachines);
  // cas-7752: removing the pairing removes the operator's stored words for it.
  purgeMachineConversations(id, { forgetInMemory: true });
  window.clearTimeout(catalogExpiryTimers.get(id)); catalogExpiryTimers.delete(id); catalogExpiresAt.delete(id);
  selection = forgetMachine(selection, id);
  if (restoreTarget?.machineId === id) restoreTarget = undefined;
  if (openAfterPairing === id) openAfterPairing = undefined;
  if (selectedMachineId === id) {
    clearStoredSelection(selectionStorage());
    const next = machines.keys().next().value;
    if (next) commitSelection({ machineId: next }); else applySelection(undefined);
  }
  // Keep the open register stable after removal; close triggers shell reconciliation.
  const dialog = document.querySelector<HTMLDialogElement>('#paired-machines-dialog');
  if (dialog?.open) { renderMachineRegister(); dialog.addEventListener('close', () => render(), { once: true }); document.getElementById('paired-machines-close')?.focus(); }
  else render();
}
