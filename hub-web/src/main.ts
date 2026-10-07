import { openInstallationInventory } from "./installation-inventory";
import { InstallationAccess, watchInstallations } from "./installation-access";
import { installationStore } from "./storage";

import { statusClass, statusLabel, workerProgress } from "./progress-model";
import { presentFleetSheet } from "./fleet-sheet";
import { CAUSE_COPY } from "./connection-diagnostics";
import { withRequestDeadline } from "./request-deadline";
import { projectTitle } from "./cloud-brand";
import { CANT_REACH_RETRYING, machineFooterMarkup, orderPairedMachines, pairedMachinesDialogMarkup, renderPairedMachines, type PairedMachineRow } from "./paired-machines";
import { retainPendingSessions, visibleCatalog } from "./worker-visibility";
import "./styles.css";
// Glass, Commander's look (cas-675e): re-colours the house tokens, adds depth.
import "./glass.css";
import { activityTime, ConversationList, filterConversationRows, groupConversationRows, machineActivityAt, plainActivity, type ConversationRow } from "./conversation-list";
import { paletteEnterTarget, sessionJumpCommandMarkup } from "./palette-commands";
import { applyHistoryCursor, ConversationHistory, supervisorWorking } from "./conversation-history";
import { gridPlaceholder, threadBeforePanes } from "./early-thread";
import { arrivalStore, readMarkStore, draftStore, pendingSendStore, purgeConversations, type Arrivals, type Draft, type PendingSend } from "./conversation-store";
import { CommanderJournal, credentialFence, deliveryScope, scopeKey, type CredentialFence, type DeliveryScope } from "./commander-journal";
import { loadDismissedAsks, saveDismissedAsks, type DismissedAsksStorage } from "./dismissed-asks";
import { ConversationView, emptyActivityText } from "./conversation-view";
import { applySheetSemantics, findByFocusKey, focusKey, layerAboveSheet, sheetFocusables, sheetKeydown } from "./attention-sheet";
import { isOperatorNotice, NOTICE_KIND, noticeFingerprint, noticeTime, planNotice } from "./operator-notices";
import { REFUSED_SEE_ABOVE, refusalSentence, refusal } from "./refusal";
import { installAttentionObjects } from "./attention-objects";
import { clearTransientAttachmentNotes, installAttachmentSheet, restateAttachmentNotes, setAttachmentNote } from "./attachment-sheet";
import { artifactFailureFollowsConnection, artifactFailureIsAboutTheFile, artifactIdFromHref, artifactIsLocalOnly, artifactLinkFor, artifactOpenFailure, openArtifact, type ArtifactMachineReach } from "./artifact-open";
import { applyActionAvailability, arrangeConversationShell, contextSheetWhere, ensureConversationStage, rawOutputDrawerMarkup, type ConversationRegions, bindKeyboardViewport, conversationAttentionBadge, keyboardViewportHeight, conversationListState, conversationNoMatchText, conversationSearchPlaceholder, conversationSkeletonMarkup, KEYBOARD_HINT_MEDIA_QUERY, fitConversationHost } from "./conversation-shell";
import { clockLabel } from "./thread-model";
import { syncContextRail, waitingOnOperator } from "./context-rail";
import { applyScheme, markAppearanceCommands, setScheme, type SchemePreference } from "./scheme";
import { applyAttentionEnrichment, attentionUrl, coalesceAttention, createAttentionItem, dismissableInfoItems, groupAttention, machineEventAttention, mergeAttentionItem, type AttentionAction, type AttentionContent, type AttentionEnrichment } from "./attention";
import { renderAttentionPanel } from "./attention-view";
import { HubConnectionSupervisor, type ConnectionState, type HubMachineInfo } from "./connection";
import { BROWSER_BLOCKED, BROWSER_UNSUPPORTED, machineConnectionLabel, UNSTEADY, UNSTEADY_SENTENCE, type AttachSnapshot } from "./connection-state";
import { CONVERSATION_OPENING, OPENING_MOTION_DELAY_MS, attachInProgress, showOpeningInto, disconnectedView, fatalConnectionRecovery, lostConnectionBanner, outageControlsReason, outageRefusal, pairingControlsReason, pairingLostBanner, pairingRefusal, unsteadyBanner, renderConnectionSurfaceInto, sessionOutageControlsReason, sessionReconnectingBanner, shouldRetainDisconnectedFrame, transportFailureNeedsAttention } from "./connection-state-view";
import { ensureMachineConnection, replaceMachineConnection } from "./connection-lifecycle";
import { createDeviceKey } from "./dpop";
import { readPairingFragment, watchPairingFragment } from "./fragment";
import { createPairingDraft, updatePairingDraft, type PairingStep } from "./pairing-draft";
import { bindPairingDialogCancel, focusPairingFeedback } from "./pairing-dialog";
import { EXPIRED_PAIRING_INVITATION_MESSAGE, INVALID_PAIRING_LINK_MESSAGE, cancellationOutcome, pairingCleanupFailureUpdate, pairingStorageClearFailureMessage, type CleanupStepContext } from "./pairing-cleanup";
import { PairingCleanupError, PairingExchangeError, PairingStorageError } from "./pairing-exchange";
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
import { firstAttachRetry, machineConnection, sessionConnection } from "./session-connection";
import { toastPlacementInThread, toastTopAboveAction, toastTopClearOfBanner } from "./toast-placement";
import { relativeTimestamp } from "./time";
import { fleetControlGate } from "./fleet-permissions";
import { FleetOpsState, UNDO_WINDOW_MS, requestMergeAction as requestMergeActionFor, type FleetAction, type FleetAgent, type FleetTask } from "./fleet-ops";
import { phoneFleetNotice, agentControls, headerControls, taskControls, undoBar, resultBar, type FleetOpsViewContext } from "./fleet-ops-view";
import { runFleetOperation } from "./fleet-ops-request";
import { detectSpeechInput, focusAfterDictation, SpeechDictationController, type SpeechInputCapability, type SpeechInputState } from "./speech-input";
import { clearStoredSelection, forgetMachine, loadStoredSelection, pairedSessionToOpen, restorableSession, saveStoredSelection, selectionAfterPairing, selectSelection, type SelectionState, type SelectionStorage, type SessionSelection } from "./session-selection";
import { planSupervisorSend, sendsOnEnter, supervisorMessage, supervisorTarget } from "./supervisor-message";
import { readablePanes } from "./worker-visibility";
import { dormantCommandLabel, dormantRevealed, dormantRoute, saveDormantRevealed } from "./dormant-visibility";
import { setMachineAccentFleet, storageAccentStore } from "./machine-accent";
import { PHONE_MEDIA_QUERY } from "./viewport";
import { TranscriptView } from "./transcript-view";
import { applyLiveRegions, type LiveRegionView } from "./live-regions";
import { DeferredRenderScheduler } from "./deferred-render";
import { FirstConnectionAnnouncer, installPairedMachine } from "./first-connection";
import { isEditableElement, renderDecision, shellSignature } from "./render-model";
import { applyDraftNote, applyMicState, composerMarkup } from "./composer-markup";
import { countdownLabel, nextCountdown, pairDialogMarkup as renderPairDialogMarkup } from "./pair-dialog-markup";
import { OperatorInboxController, startInboxLoop } from "./inbox/controller";
import { InboxView, browserLabel } from "./inbox/inbox-view";
import { enrollPairedInstallation } from "./inbox/hub-enrollment";
import { inboxThreads } from "./inbox/projection";
import { IndexedDbInboxStore } from "./inbox/store";
import { operatorOrigin } from "./inbox/wire";
import type { AttentionItem, ConversationHistoryPage, HubSession, LeaseState, OperatorReply, Scope, SessionCardSummary, SessionState, StoredMachine } from "./types";

applyScheme();

const pendingPairingStore = pendingPairingStoreFor(window);
const relayOrigin = pairingRelayOrigin(document.querySelector<HTMLMetaElement>('meta[name="cas-pairing-relay-origin"]')?.content ?? null);
// cas-9b7d: the account's durable operator inbox. It loads on its own, with
// no machine connection, so retained supervisor messages read on a new
// device while every hub is off.
const operatorInbox = new OperatorInboxController({
  origin: operatorOrigin(document.querySelector<HTMLMetaElement>('meta[name="cas-operator-inbox-origin"]')?.content ?? relayOrigin),
  store: new IndexedDbInboxStore(window.indexedDB),
  pageOrigin: window.location.origin,
  locks: navigator.locks ?? null,
  channel: typeof BroadcastChannel === "function" ? new BroadcastChannel("cas-operator-inbox") : null,
});
const inboxView = new InboxView(operatorInbox, { defaultLabel: browserLabel(navigator.userAgent) });
let inboxLoop: AbortController | null = null;
function syncInboxLoop(ready: boolean): void {
  if (ready && !inboxLoop) {
    inboxLoop = new AbortController();
    startInboxLoop(operatorInbox, inboxLoop.signal);
  } else if (!ready && inboxLoop) {
    inboxLoop.abort();
    inboxLoop = null;
  }
}
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
const installationAccess = new InstallationAccess(installationStore, catalog);
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
const INTERRUPTED_LABELS = new Set(["Reconnecting", "Unreachable", "Needs pairing", UNSTEADY, CANT_REACH_RETRYING, BROWSER_BLOCKED, BROWSER_UNSUPPORTED]);
const machineInfo = new Map<string, HubMachineInfo | undefined>();
const statuses = new Map<string, Record<string, unknown>>();
/** Sessions whose first status is on its way: the context rail holds its place for them (cas-813a). */
const statusPending = new Set<string>();
const leases = new Map<string, LeaseState>();
const surfaces = new Map<string, TerminalSurface>();
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
  // A tap's click follows its lifted finger at once; a long press or a lift
  // that produces none releases the rebuild after this.
  touchWindow: (run) => window.setTimeout(run, 600),
});
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
const leaseHeartbeats = new Map<string, number>();
const leaseExpiryTimers = new Map<string, number>();
/** How often an open session held by another device is re-checked for release (journey F5). */
const FOREIGN_LEASE_RECHECK_MS = 5_000;
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
const revealDormant = dormantRevealed(location.search, workerVisibilityStorage());
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

// One phone definition shared by the stylesheet and the layout state — see
// viewport.ts. Rotation must not put the CSS and this logic in different
// modes, which a width-only breakpoint guaranteed it would.
function phoneLayout(): boolean { return window.matchMedia(PHONE_MEDIA_QUERY).matches; }
function keyboardHintOffered(): boolean { return window.matchMedia(KEYBOARD_HINT_MEDIA_QUERY).matches; }

/**
 * A session's supervisor pane is read through one hidden terminal surface
 * (cas-0546): it keeps the attach, the keyframes and the raw text the Raw
 * output drawer shows. Releasing it takes the thread it fed with it.
 */
function releaseSurface(key: string, surface: TerminalSurface): void {
  conversationViews.get(key)?.dispose();
  conversationViews.delete(key);
  surface.dispose();
  surfaces.delete(key);
  syncRawOutput();
}

/**
 * The conversation thread for a session's supervisor pane, in the visible
 * thread slot beside (never inside) the hidden pane host. It goes up as soon
 * as the pane is known — before the hidden surface finishes loading — so
 * opening a conversation never shows a bare panel (cas-04ee).
 */
function mountConversation(key: string, mount: HTMLElement): void {
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
      cancelMessage: (send) => { void cancelWaitingMessage(threadMachineId, threadSession, send.id); },
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
      // is doing (its newest queue row, else its panes' last output) instead
      // of another session's thread.
      activity: () => sessionActivity(threadMachineId, threadSession),
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
    && threadBeforePanes({ placeholder: gridPlaceholder(grid!), history: conversationHistories.get(threadKey!) });
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
    mount.className = "conversation-mount conversation-early";
    grid!.prepend(mount);
    grid!.classList.add("has-early-thread");
  }
  mountConversation(earlyThreadKey(threadKey!), mount);
}

function sessionKey(machineId: string, session: string): string { return `${machineId}:${session}`; }
function paneKey(machineId: string, session: string, pane: string): string { return `${machineId}:${session}:${pane}`; }
function activeConnection(): HubConnectionSupervisor | undefined { return selectedMachineId ? connections.get(selectedMachineId) : undefined; }

function workerVisibilityStorage(): SelectionStorage | undefined {
  try { return window.localStorage; } catch { return undefined; }
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

async function boot(): Promise<void> {
  const remotePending = await installationAccess.recover(window.fetch.bind(window));
  const stored = remotePending ? await catalog.snapshot() : await catalog.recoverPending();
  const pendingHubs = new Set((await installationStore.list()).filter((r) => r.pending).map((r) => r.id.split("@")[0]));
  for (const machine of stored.machines) if (!pendingHubs.has(machine.id)) machines.set(machine.id, machine);
  watchInstallations((hubId) => {
    // The same hub ID at another URL is a separate trust boundary. Never
    // expose a staged prior while remote cancellation is still unresolved.
    void installationStore.list().then(async (records) => {
      if (records.some((r) => r.pending && r.id.split("@")[0] === hubId)) return;
      const { machines: stored } = await catalog.snapshot();
      const current = machines.get(hubId);
      const accepted = stored.find((m) => m.id === hubId && m.baseUrl === current?.baseUrl);
      if (accepted && current && (accepted.credentialGeneration ?? 0) >= (current.credentialGeneration ?? 0)) Object.assign(current, accepted);
    }).catch(() => { /* Durable storage remains authoritative; refusal recovery retries adoption. */ });
  });
  machineCatalogLoaded = true;
  void operatorInbox.snapshot().then((snapshot) => hydrateInboxThreads(snapshot.events)).catch(() => undefined);
  if (stored.pendingCleanup > 0 || remotePending > 0) {
    pairingCleanupFailed = true;
    pairingCleanupContext = { cause: "cancel", storeOpen: false, rollbackPending: true };
    pairingCancellations.begin(undefined);
    pairingStatus = "Pairing cleanup needs confirmation from the hub. Retry cleanup to restore the previous access.";
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
      // cas-387e: the composer's waiting line follows the connection, and
      // clears once nothing is held.
      if (messageStatus?.held && messageStatus.session?.startsWith(`${machine.id}:`) && selectedMachineId === machine.id && selectedSession) settleHeldComposerStatus(machine.id, selectedSession);
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
        // cas-7b31: control does not come back by itself now.
        for (const key of [...controlLostToOutage]) if (key.startsWith(`${machine.id}:`)) controlLostToOutage.delete(key);
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
    onCredentialRefreshed: async (refreshed) => {
      await catalog.put(refreshed);
      const accepted = (await catalog.snapshot()).machines.find((m) => m.id === refreshed.id);
      if (accepted) {
        Object.assign(refreshed, accepted); machines.set(refreshed.id, refreshed);
        for (const record of await installationStore.list()) {
          if (!record.pending && record.id === `${accepted.id}@${new URL(accepted.baseUrl).origin}` && accepted.credentialGeneration !== undefined) {
            record.known = { deviceId: accepted.deviceId, credentialGeneration: accepted.credentialGeneration };
            await installationStore.put(record);
          }
        }
      }
    },
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
    },
    onPaneSize: (session, pane, cols, rows) => {
      applyPaneAuthority(machine.id, session, pane, cols, rows);
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
    },
    onMessageQueued: (session, receipt, frameFence) => {
      const accepted = machines.get(machine.id);
      if (frameFence && (!accepted || accepted.deviceId !== machine.deviceId || accepted.baseUrl !== machine.baseUrl || accepted.credentialId !== frameFence.credentialId || credentialFence(accepted).generation !== frameFence.generation)) return;
      conversationHistory(sessionKey(machine.id, session)).acknowledge(receipt);
      if (accepted) journalWrites = journalWrites.then(async () => {
        await sendJournal.acknowledge(deliveryScope(accepted, session), receipt, credentialFence(accepted));
      }).catch(() => { /* A failed save never authorizes another wire write. */ });
      if (messageDelivery?.session === sessionKey(machine.id, session) && messageDelivery.clientRef === receipt.client_ref) { messageDelivery = undefined; document.querySelector<HTMLElement>("#message-delivery")?.setAttribute("hidden", ""); }
      settleHeldComposerStatus(machine.id, session);
      updateConversationViews(); renderConversationList();
    },
    onOperatorMessage: (session, message) => {
      conversationHistory(sessionKey(machine.id, session), session).hydrateSend(message);
      updateConversationViews(); renderConversationList();
      if (selectedMachineId === machine.id && selectedSession === session) render();
    },
    onMessageRejected: async (session, clientRef, detail, rejection) => {
      const key = sessionKey(machine.id, session);
      // cas-0653: the hub could not reach the session's daemon, so the
      // message never arrived there. It waits in the thread and goes out once
      // on the next live attach (the connection reattaches for it), instead
      // of reading "Not sent".
      const settled = await sendJournal.refuse(deliveryScope(machine, session), clientRef, credentialFence(machine), detail, rejection?.retryable === true);
      // A late refusal for a prior attempt cannot overwrite a Retry or receipt.
      if (settled === "stale") return;
      if (settled === "held" && reholdRefusedSend(machine, session, clientRef)) {
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
      const trackedDelivery = messageDelivery?.session === key && messageDelivery.clientRef === clientRef;
      if (trackedDelivery) {
        messageDelivery = undefined;
        document.querySelector<HTMLElement>("#message-delivery")?.setAttribute("hidden", "");
      }
      if (selectedMachineId === machine.id && selectedSession === session && (onBubble || trackedDelivery)) {
        // The journal owns send state; this refusal need not have a legacy
        // messageDelivery tracker. A refused message never promises a retry.
        // The bubble names the reason once; the composer points at it (cas-4d92).
        showComposerStatus(onBubble ? REFUSED_SEE_ABOVE : refusalSentence(detail), "error");
      }
      updateConversationViews(); renderConversationList();
    },
    onOperatorReply: (session, reply, frameFence) => {
      // A notice belongs only to attention, including its durable resolution.
      if (isOperatorNotice(reply)) { applyOperatorNotice(machine, session, reply); return; }
      void persistDeviceReply(machine, session, reply, frameFence);
      conversationHistory(sessionKey(machine.id, session), session).receive({ ...reply, device_persisted: false }, Date.now(), session);
      // A later supervisor turn shortens an unreceipted send's wait (cas-1622).
      scheduleReceiptCheck(sessionKey(machine.id, session));
      updateConversationViews(); renderConversationList();
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
    onConversationHistory: (session, page: ConversationHistoryPage, frameFence) => {
      const key = sessionKey(machine.id, session);
      const cursor = conversationHistoryPage(key);
      applyHistoryCursor(cursor, page);
      const history = conversationHistory(key, session);
      // cas-55a4: the thread is this session's own turns. Other sessions'
      // turns (the page's earlier_* section, or a project-wide page from an
      // older daemon) are filed beside it by session, never into it.
      for (const message of [...page.messages, ...(page.earlier_messages ?? [])]) history.hydrateSend(message);
      for (const reply of [...page.replies, ...(page.earlier_replies ?? [])]) {
        // cas-e829: this session's notices raise or retire attention; another
        // session's are its own business.
        if (isOperatorNotice(reply)) {
          if (reply.session === undefined || reply.session === session) applyOperatorNotice(machine, session, reply);
          continue;
        }
        if (reply.session === undefined || reply.session === session) void persistDeviceReply(machine, session, reply, frameFence);
        // History has reached this device, but the asynchronous journal commit
        // has not proved storage yet. Render its forwarded receipt immediately,
        // as for live replies, so committing it does not insert a new line above
        // the reader's position after Load earlier (HUB-J4).
        history.hydrateReply(reply.session === undefined || reply.session === session ? { ...reply, device_persisted: false } : reply);
      }
      updateConversationViews();
      renderConversationList();
      if (selectedMachineId === machine.id && selectedSession === session) render();
    },
    onSessionSummary: (session, summary) => {
      sessionSummaries.set(sessionKey(machine.id, session), summary);
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
  return sessionConnection(machine, attachStates.get(key), sessionsEverLive.has(key), connections.get(machineId)?.hasLiveAttach(session));
}

function machineFooterConnection(machineId: string): ConnectionState | undefined {
  const prefix = `${machineId}:`;
  // The catalog removes ended peers; their cached retry lifecycle must not
  // keep a machine with a responding conversation labelled Reconnecting.
  const listed = new Set((sessions.get(machineId) ?? []).map(session => session.name));
  const attached = [...attachStates].filter(([key, attach]) => key.startsWith(prefix) && listed.has(attach.session)).map(([key, attach]) => ({ attach, wasLive: sessionsEverLive.has(key), responding: connections.get(machineId)?.hasLiveAttach(attach.session) }));
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
  const browserName = form.querySelector<HTMLInputElement>('input[name="device"]');
  if (browserName) {
    browserName.value = browserName.value.trim();
    browserName.setCustomValidity(browserName.value ? "" : "Enter a name for this browser.");
    if (!browserName.reportValidity()) return false;
  }
  const values = new FormData(form);
  pairingDraft = updatePairingDraft(pairingDraft, values.entries(), !invitation.hubUrl);
  const operation = pairingOperations.begin();
  pairingExchangeInFlight = true;
  exchangeOperationGeneration = operation.generation;
  pairingStatus = "Updating this browser installation… Cancel restores its previous access.";
  render();
  // Pair is now disabled; keep the keyboard in the dialog on its available
  // next action rather than letting the browser drop focus to the body.
  document.querySelector<HTMLButtonElement>("#pair-dialog #pair-cancel")?.focus({ preventScroll: true });
  let machine: StoredMachine;
  try {
    machine = await installationAccess.pair({
      invitation,
      controllerOrigin: location.origin,
      legacyHubUrl: invitation.hubUrl ? undefined : String(values.get("url")),
      machineLabel: String(values.get("label")),
      rotateKey: values.get("rotate-key") === "on",
      deviceLabel: String(values.get("device")),
      operatorLabel: String(values.get("operator")),
      // The link form lists every scope box (its hub:admin box is the consent
      // beside the name fields). The relay form has no scope list, so its
      // invitation's scopes stand, except hub:admin, which is held only when
      // its consent box is ticked (cas-5e53 F08).
      requestedScopes: (() => {
        const chosen = values.getAll("scope") as Scope[];
        if (form.querySelector(".pair-scope-list")) return chosen;
        if (!invitation.scopes) return undefined;
        return [...invitation.scopes.filter((scope) => scope !== "hub-admin"), ...(chosen.includes("hub-admin") ? ["hub-admin" as Scope] : [])];
      })(),
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
      focusPairingFeedback(document.querySelector<HTMLDialogElement>("#pair-dialog"));
      throw error;
    }
    if (error instanceof PairingExchangeError) {
      pairingOperations.invalidate();
      const cleared = pendingPairingStore.clear();
      pendingPairing = null;
      pairingDraft = createPairingDraft(location.origin);
      pairingStatus = pairingStorageClearFailureMessage(error.message, cleared);
      render(false);
      focusPairingFeedback(document.querySelector<HTMLDialogElement>("#pair-dialog"));
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
          await withRequestDeadline(signal => fetch(new URL("/v1/health", hubUrl), {
            method: "GET",
            mode: "no-cors",
            cache: "no-store",
            credentials: "omit",
            signal,
          }), operation.signal, 3_000);
          if (!pairingOperations.isCurrent(operation) || pendingPairing?.kind !== "invitation") return;
          // The heading already says "Machine authorized"; the status only names the next step (cas-b2e4 F01).
          pairingStatus = "Add your name, then press Pair.";
        } catch {
          if (!pairingOperations.isCurrent(operation) || pendingPairing?.kind !== "invitation") return;
          const machine = result.invitation.machineLabel ?? result.invitation.hubId;
          pairingStatus = `Approved — this browser's reachability check for ${machine} failed. Check Tailscale (VPN), browser site permissions (Local network access), and Private DNS or secure DNS, then press Pair to try this approved invitation.`;
          render();
          focusPairingFeedback(document.querySelector<HTMLDialogElement>("#pair-dialog"));
          return;
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
    const remotePending = await installationAccess.recover(window.fetch.bind(window));
    recovery = { pendingCleanup: remotePending || (await catalog.recoverPending()).pendingCleanup };
  } catch {
    recovery = { failed: true };
  }
  if (!pairingCancellations.finishRetry(ticket)) return;
  const outcome = cleanupRetryOutcome(cleared, recovery);
  pairingStatus = outcome.status;
  if (outcome.done) {
    // Boot quarantines hubs with uncertain remote rollback. Only confirmed
    // recovery may repopulate them and restart their connections.
    const restored = await catalog.snapshot();
    for (const machine of restored.machines) {
      if (!machines.has(machine.id)) {
        machines.set(machine.id, machine);
        ensureConnection(machine);
      }
    }
    selectedMachineId ??= machines.keys().next().value;
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
  showOpeningInto(placeholder, CONVERSATION_OPENING, openingDelay(sessionKey(machineId, session)));
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
  grid.style.removeProperty("--outage-banner-space");
  grid.querySelector(".terminal-disconnected-banner")?.remove();
  const shown = document.querySelector<HTMLElement>("#toast");
  if (shown) placeToastClearOfBanner(shown);
}

/** The open Connection log's summary writer, run with every header update (cas-d043 G04). */
let refreshConnectionLog: (() => void) | undefined;
function openConnectionLog(machineId: string): void {
  let dialog = document.querySelector<HTMLDialogElement>("#connection-log");
  if (!dialog) {
    dialog = document.createElement("dialog");
    dialog.id = "connection-log";
    dialog.className = "connection-log";
    dialog.setAttribute("aria-label", "Connection log");
    dialog.innerHTML = '<section><header><div><p class="connection-log-eyebrow"></p><h2>Connection log</h2></div><form method="dialog"><button type="submit" aria-label="Close connection log">×</button></form></header><p class="connection-log-summary" aria-live="off"></p><button type="button" class="connection-log-export" disabled>Export safe diagnostics</button><details class="connection-log-technical"><summary>Technical details</summary><p class="connection-log-evidence"></p><pre>Running diagnostics…</pre></details></section>';
    document.body.append(dialog);
  }
  const output = dialog.querySelector("pre")!;
  const summary = dialog.querySelector<HTMLElement>(".connection-log-summary")!;
  const technical = dialog.querySelector<HTMLElement>(".connection-log-evidence")!;
  const machineLabel = machines.get(machineId)?.label ?? "this machine";
  // cas-d043 G04: the eyebrow names the machine; "Evidence ledger" was jargon.
  dialog.querySelector<HTMLElement>(".connection-log-eyebrow")!.textContent = machines.get(machineId)?.label ?? "Machine";
  const download = dialog.querySelector<HTMLButtonElement>(".connection-log-export")!;
  download.disabled = true;
  const update = () => {
    // cas-d043 G04: the state the header and footer show, from the same
    // source, so the log never says "Connecting now" beside a Live header.
    const state = machineId === selectedMachineId && selectedSession ? conversationStatusState(machineId, selectedSession) : machineFooterConnection(machineId);
    const cause = state?.cause;
    // cas-97d58 F08: what happens next, in the live state's own words; never
    // "No retry scheduled" while the rail says it is checking.
    const retry = state?.nextRetryAt !== undefined ? `Next retry in ${Math.max(0, Math.ceil((state.nextRetryAt - Date.now()) / 1000))}s.`
      : state?.phase === "live" ? (state.degraded ? "Checking the connection now." : "Connected now.")
        : state?.phase === "failed" ? (state.fatal === true || state.authFailure ? "Cassy won't retry this by itself." : "Retrying.")
          : "Connecting now.";
    const evidence = cause ? `${cause.status === undefined ? "" : ` Measured HTTP status: ${cause.status}.`}${cause.closeCode === undefined ? "" : ` Measured socket close: ${cause.closeCode}.`}${cause.permission === undefined || cause.permission === "unknown" ? "" : ` Measured local-network permission: ${cause.permission}.`}` : "";
    // One plain sentence that names the machine and the next step leads; the
    // layer, measured codes and engineering notes sit behind Technical details.
    summary.textContent = `${cause ? `${machineLabel}: ${CAUSE_COPY[cause.code].title}. ${CAUSE_COPY[cause.code].action}` : `${machineLabel}: nothing is failing now.`} ${retry} Last successful connection: ${state?.lastSuccessAt ? new Date(state.lastSuccessAt).toLocaleTimeString() : "not measured in this visit"}.`;
    const technicalText = cause ? `Layer: ${cause.layer}.${evidence}${CAUSE_COPY[cause.code].detail ? ` ${CAUSE_COPY[cause.code].detail}` : ""}` : "";
    if (technical.textContent !== technicalText) technical.textContent = technicalText;
    technical.hidden = technicalText === "";
  };
  update();
  // The countdown ticks each second; a change of state is written on the
  // same render that changes the header's word (renderConversationList).
  const timer = window.setInterval(update, 1000);
  refreshConnectionLog = update;
  dialog.addEventListener("close", () => { window.clearInterval(timer); if (refreshConnectionLog === update) refreshConnectionLog = undefined; }, { once: true });
  output.textContent = "Running diagnostics…";
  dialog.showModal();
  void connections.get(machineId)?.diagnose().then((result) => {
    const json = JSON.stringify(result, null, 2);
    output.textContent = json;
    download.disabled = new TextEncoder().encode(json).byteLength > 65_536;
    download.onclick = () => {
      const url = URL.createObjectURL(new Blob([json], { type: "application/json" }));
      const link = document.createElement("a");
      link.href = url; link.download = "commander-connection-diagnostics.json";
      link.click();
      window.setTimeout(() => URL.revokeObjectURL(url), 0);
    };
  }).catch((error) => {
    output.textContent = "Diagnostics unavailable. The connection cause above remains available.";
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
      // Announce the cause once; adjacent recovery controls stay discoverable
      // without becoming part of the live-region announcement.
      words.setAttribute("role", "status");
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
    if (!banner.querySelector(".banner-diagnose")) {
      const details = document.createElement("button");
      details.type = "button"; details.className = "banner-diagnose";
      details.textContent = "Details"; details.setAttribute("aria-label", "Connection details");
      details.onclick = () => openConnectionLog(machineId);
      banner.append(details);
    }
    banner.dataset.attempt = String(view.attempt);
    grid.classList.add("terminal-disconnected");
    // cas-d043 G14: the thread's top spacer, the banner's height and its inset.
    const space = `${Math.ceil(banner.getBoundingClientRect().height + 2 * 8)}px`;
    if (grid.style.getPropertyValue("--outage-banner-space") !== space) grid.style.setProperty("--outage-banner-space", space);
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
    openingTitle: CONVERSATION_OPENING,
    quietOpening: true,
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
  message.textContent = `Can't open this conversation: ${detail}`;
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
    // cas-0546: a task waiting to merge, or blocked, is progress, not
    // something waiting on the operator. Tasks & progress shows its state;
    // only the supervisor's own asks and blockers wait on the operator.
    statuses.set(sessionKey(machineId, session), status);
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
      leases.set(key, state);
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
 * The PTY geometry the daemon reports for each pane (cas-37f8). Commander
 * never sizes a pane (cas-0546): its one surface per session is hidden, so it
 * is pinned to the pane's real grid, and the raw output reads the lines the
 * supervisor's terminal actually wrapped.
 */
const paneGeometry = new Map<string, { cols: number; rows: number }>();

/** A pane with no reported size yet reads at the session's own size, else a standard terminal. */
const FALLBACK_PANE_GEOMETRY = { cols: 80, rows: 24 } as const;

function paneGrid(key: string, state: SessionState | undefined): { cols: number; rows: number } {
  const reported = paneGeometry.get(key);
  if (reported) return reported;
  if (state && state.cols > 0 && state.rows > 0) return { cols: state.cols, rows: state.rows };
  return FALLBACK_PANE_GEOMETRY;
}

function applyPaneAuthority(machineId: string, session: string, paneId: string, cols: number, rows: number): void {
  if (!(cols > 0 && rows > 0)) return;
  const key = paneKey(machineId, session, paneId);
  paneGeometry.set(key, { cols, rows });
  surfaces.get(key)?.setAuthoritativeSize({ cols, rows });
}

/**
 * The pane the conversation reads (cas-0546): the session's supervisor,
 * chosen deterministically, never by layout or focus. Workers and the
 * director are never mounted.
 */
function supervisorPane(machineId: string, session: string, state: SessionState | undefined = sessionStates.get(sessionKey(machineId, session))): string | undefined {
  if (!state) return undefined;
  const visible = readablePanes(state.panes);
  return (visible.find((pane) => pane.kind === "Supervisor") ?? visible[0])?.id;
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
  const paneId = supervisorPane(machineId, session, state);
  if (paneId === undefined) {
    for (const [key, surface] of surfaces) {
      if (!key.startsWith(`${machineId}:${session}:`)) continue;
      releaseSurface(key, surface);
    }
    const empty = document.createElement("div");
    empty.className = "empty empty-pane-slot";
    const emptyTitle = document.createElement("p");
    emptyTitle.className = "empty-title";
    emptyTitle.textContent = "The supervisor hasn't started yet";
    const emptyHint = document.createElement("p");
    emptyHint.className = "empty-hint";
    emptyHint.textContent = "The conversation opens here as soon as the session starts its supervisor.";
    empty.replaceChildren(emptyTitle, emptyHint);
    grid.replaceChildren(empty);
    syncEarlyThread();
    syncConversationActions();
    return;
  }
  // cas-fc2c: the pane is about to replace the thread shown before it. A
  // reader in it lands in the pane's thread, as a reader on the connecting
  // card does, rather than on the page once the grid is rebuilt.
  const earlyActive = document.activeElement;
  if (earlyActive instanceof HTMLElement && grid.querySelector(":scope > .conversation-early")?.contains(earlyActive)) {
    landFocus([focusTargets.thread], { keep: true, nextTask: true, waitMs: 2_000, since: earlyActive });
  }
  // Only the grid's own placeholder: a bare ".empty" also matched the
  // conversation thread's empty state and deleted it, leaving a re-opened
  // conversation on a blank panel (cas-04ee).
  grid.querySelector(":scope > .empty")?.remove();
  // cas-0546: the thread has its own visible slot; the pane's surface lives in
  // a host that renders nothing (hidden, inert, out of the accessibility tree)
  // and never contains the thread.
  const { slot, host } = ensureConversationStage(grid);
  // cas-fc2c: the pane is up, so the thread shown before it gives way to the
  // one mounted for the supervisor pane below.
  syncEarlyThread();
  const key = paneKey(machineId, session, paneId);
  for (const [other, surface] of surfaces) {
    if (other.startsWith(`${machineId}:${session}:`) && other !== key) releaseSurface(other, surface);
  }
  let card = host.querySelector<HTMLElement>(`:scope > .pane[data-pane-id="${CSS.escape(paneId)}"]`);
  let mount = card?.querySelector<HTMLElement>(".terminal-mount");
  if (!card || !mount) {
    card = document.createElement("section");
    card.className = "pane";
    card.dataset.paneId = paneId;
    card.dataset.paneRole = state.panes.find((pane) => pane.id === paneId)?.kind.toLowerCase() ?? "supervisor";
    mount = document.createElement("div"); mount.className = "terminal-mount";
    card.append(mount);
    host.replaceChildren(card);
  }
  const existingSurface = surfaces.get(key);
  if (existingSurface && (existingSurface.element !== mount || !existingSurface.element.isConnected)) releaseSurface(key, existingSurface);
  // The thread goes up before the surface loads in the hidden host, so the
  // reader sees the conversation at once.
  mountConversation(key, slot);
  if (!surfaces.has(key)) {
    const surface = await createTerminalSurface(mount, {
      // Read-only (cas-0546): Commander never types into a pane or sizes it.
      onData: () => undefined,
      onResize: () => undefined,
      // The thread and the raw output follow the emulator's own tick
      // instead of polling it.
      onRender: () => { conversationViews.get(key)?.update(); if (rawOutput?.key === key) rawOutput.view.update(); },
    });
    const currentMount = document.querySelector<HTMLElement>(`#pane-grid .pane-host [data-pane-id="${CSS.escape(paneId)}"] .terminal-mount`);
    if (selectedMachineId !== machineId || selectedSession !== session || !mount.isConnected || currentMount !== mount) {
      surface.dispose();
      return;
    }
    surfaces.set(key, surface);
    surface.setCanvasPainting(false);
    // Pinned before the replay, so the buffer is never written at a grid
    // that is about to change (cas-37f8).
    surface.setAuthoritativeSize(paneGrid(key, state));
    const buffered = paneBuffers.get(key);
    if (buffered) surface.write(new Uint8Array(buffered));
    syncRawOutput();
  }
  if (authoritativeSessions.has(selectedKey) && !paneKeyframesReady.has(key)) {
    connections.get(machineId)?.requestPaneKeyframe(session, paneId);
  }
  syncConversationActions();
}

/**
 * Why the open conversation's actions wait while it is down (cas-1730,
 * cas-0546), or undefined when it is not an outage. A first connection is
 * not an outage: it reads "once the conversation is connected" instead.
 */
function conversationOutageReason(machine: StoredMachine, session: string): string | undefined {
  const state = conversationConnection(machine.id, session);
  if (state === undefined || state.phase === "live") return undefined;
  const machineState = connectionStates.get(machine.id);
  const outage = sessionsEverLive.has(sessionKey(machine.id, session))
    || (machineState !== undefined && machineState.phase !== "live" && lastLiveAt.has(machine.id));
  if (!outage) return undefined;
  // cas-7b31 (journey F2): a refused pairing does not reconnect, so it does not promise to.
  if (machineState?.authFailure) return pairingControlsReason(machine.label);
  if (sessionOnlyDrop(machine.id, session)) return sessionOutageControlsReason(conversationLabel(machine.id, session));
  if (machineState?.fatal === true) return `${fatalConnectionRecovery(machineState.reason)} Interrupt and raw output wait until then.`;
  return outageControlsReason(machine.label);
}

/** Why Interrupt can't run for the open conversation, in plain words, or undefined when it can. */
function interruptUnavailableReason(): string | undefined {
  const machine = selectedMachineId ? machines.get(selectedMachineId) : undefined;
  const session = selectedSession;
  if (!machine || !session) return "Open a conversation to interrupt its supervisor.";
  const outage = conversationOutageReason(machine, session);
  if (outage) return outage;
  if (conversationConnection(machine.id, session)?.phase !== "live" || !machineInfo.has(machine.id)) return "Interrupt works once the conversation is connected.";
  if (!hubSupports(machine.id, "daemon_attach")) return `The hub on ${machine.label} is too old to take interrupts from Cassy Cloud. Upgrade it, then reconnect this machine.`;
  if (!machine.scopes.includes("pane-interrupt")) return `This browser was paired without permission to interrupt. Run cas hub pair --origin ${location.origin} on ${machine.label}, open the new pairing link here, and approve control access.`;
  if (!supervisorPane(machine.id, session)) return "The supervisor hasn't started yet, so there is nothing to interrupt.";
  return undefined;
}

/** Why Raw output can't open for the open conversation, or undefined when it can. */
function rawOutputUnavailableReason(): string | undefined {
  const machine = selectedMachineId ? machines.get(selectedMachineId) : undefined;
  const session = selectedSession;
  if (!machine || !session) return "Open a conversation to read its raw output.";
  const outage = conversationOutageReason(machine, session);
  if (outage) return outage;
  if (!selectedSurface()) return "Raw output appears once the conversation is connected.";
  return undefined;
}

/** The open conversation's one pane surface, keyed by pane, if it is up. */
function selectedSurface(): { key: string; surface: TerminalSurface } | undefined {
  if (!selectedMachineId || !selectedSession) return undefined;
  const paneId = supervisorPane(selectedMachineId, selectedSession);
  const key = paneId === undefined ? undefined : paneKey(selectedMachineId, selectedSession, paneId);
  const surface = key ? surfaces.get(key) : undefined;
  return key && surface ? { key, surface } : undefined;
}

/** The header's Interrupt and Raw output follow what they can do now, without a shell rebuild. */
/**
 * cas-d043 H15: the pair dialog's action bar frosts only while fields scroll
 * on beneath it; with nothing under it, it is the sheet itself (glass.css).
 */
const pairBarBound = new WeakSet<HTMLElement>();
function bindPairBarFrost(dialog: HTMLDialogElement): void {
  const scroller = dialog.querySelector<HTMLElement>(":scope > .pair-flow, :scope > #pair-form");
  const bar = scroller?.querySelector<HTMLElement>(":scope > .dialog-actions");
  if (!scroller || !bar || pairBarBound.has(scroller)) return;
  pairBarBound.add(scroller);
  const sync = () => bar.classList.toggle("fields-beneath", scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight > 1);
  scroller.addEventListener("scroll", sync, { passive: true });
  if (typeof ResizeObserver !== "undefined") {
    const observer = new ResizeObserver(sync);
    observer.observe(scroller);
    for (const child of scroller.children) observer.observe(child);
  }
  sync();
}

/** The skip link's target: the open conversation's first visible control (cas-d043 G02/H11). */
function skipToConversation(): void {
  const main = document.querySelector<HTMLElement>(".conversation-main");
  const first = [...(main?.querySelectorAll<HTMLElement>("button, [href], input, textarea, [tabindex]:not([tabindex='-1'])") ?? [])]
    .find((node) => node.getClientRects().length > 0 && !(node as HTMLButtonElement).disabled && !node.closest("[hidden], [inert]"));
  first?.focus();
}

function syncConversationActions(): void {
  retireStaleToast();
  syncControlHolder();
  applyActionAvailability(document.querySelector<HTMLButtonElement>("#conversation-interrupt"), document.querySelector<HTMLElement>("#conversation-interrupt-reason"), interruptUnavailableReason());
  applyActionAvailability(document.querySelector<HTMLButtonElement>("#conversation-raw-output"), document.querySelector<HTMLElement>("#conversation-raw-output-reason"), rawOutputUnavailableReason());
}

/**
 * cas-d043 G02: while another device holds control of the open session, the
 * header says which ("· Studio iPad in control"), so Interrupt taking it over,
 * or being refused, is never the first the operator hears of it.
 */
function syncControlHolder(): void {
  const host = document.querySelector<HTMLElement>(".conversation-identity .conversation-host");
  if (!host) return;
  const key = selectedMachineId && selectedSession ? sessionKey(selectedMachineId, selectedSession) : undefined;
  const lease = key ? leases.get(key) : undefined;
  const holder = lease && !lease.held_by_me ? lease.controller_label || undefined : undefined;
  let note = host.querySelector<HTMLElement>(":scope > #conversation-control");
  if (!holder) {
    if (note) { note.remove(); fitConversationHost(document); }
    return;
  }
  if (!note) { note = document.createElement("span"); note.id = "conversation-control"; host.append(note); }
  if (note.dataset.holder === holder) return;
  note.dataset.holder = holder;
  const separator = document.createElement("span"); separator.setAttribute("aria-hidden", "true"); separator.textContent = " · ";
  note.replaceChildren(separator, `${holder} in control`);
  fitConversationHost(document);
}

const interruptsInFlight = new Set<string>();

/**
 * Interrupt the open conversation's supervisor (cas-0546, moved from the
 * Terminal header). Interrupting is a leased action, as sending is, so
 * control is taken for it the same way; taking it from another device is
 * never silent. Every outcome is said in a toast.
 */
async function interruptSupervisor(): Promise<void> {
  const reason = interruptUnavailableReason();
  const thread = selectedMachineId && selectedSession ? sessionKey(selectedMachineId, selectedSession) : undefined;
  if (reason) { toast(reason, { thread, until: () => interruptUnavailableReason() !== reason }); return; }
  const machine = machines.get(selectedMachineId!)!;
  const session = selectedSession!;
  const key = sessionKey(machine.id, session);
  const paneId = supervisorPane(machine.id, session);
  if (paneId === undefined || interruptsInFlight.has(key)) return;
  interruptsInFlight.add(key);
  try {
    const lease = leases.get(key);
    const holder = lease && !lease.held_by_me ? lease.controller_label ?? undefined : undefined;
    if (lease?.held_by_me !== true) {
      const force = Boolean(holder && machine.scopes.includes("hub-admin"));
      if (!await takeControlForMessage(machine, session, force)) {
        toast(holder
          ? `${holder} is in control of this session. Interrupt works once it releases control, or from a pairing with administrator access, which can take over.`
          : "Couldn't take control of this session to interrupt it. Check that it's live, then try again.", { thread: key });
        return;
      }
    }
    const took = holder ? `Took control from ${holder}. ` : "";
    if (!sendControl(machine.id, session, { InterruptPane: { pane_id: paneId } })) {
      if (took) toast(`${took}The interrupt didn't go through; try again once the conversation is live.`, { thread: key });
      return;
    }
    toast(`${took}Interrupted ${supervisorPhrase(machine.id, session)}.`, { thread: key });
  } finally {
    interruptsInFlight.delete(key);
  }
}

/** The transcript the open Raw output drawer shows, and the thread it was opened for. */
let rawOutput: { key: string; view: TranscriptView } | undefined;
let rawOutputThread: string | undefined;

/** The drawer lives on document.body, so a shell rebuild never closes it. */
function rawOutputDialog(): HTMLDialogElement {
  let dialog = document.querySelector<HTMLDialogElement>("#raw-output");
  if (dialog) return dialog;
  dialog = document.createElement("dialog");
  dialog.id = "raw-output";
  dialog.className = "raw-output-drawer";
  dialog.setAttribute("aria-labelledby", "raw-output-title");
  dialog.setAttribute("aria-describedby", "raw-output-subject");
  dialog.innerHTML = rawOutputDrawerMarkup();
  document.body.append(dialog);
  const opened = dialog;
  opened.querySelector<HTMLButtonElement>(".raw-output-close")!.onclick = () => opened.close();
  // Escape and Close both land here: focus goes back to the header's Raw
  // output, the one a shell rebuild may have replaced since it opened.
  opened.addEventListener("close", () => {
    releaseRawOutputView();
    rawOutputThread = undefined;
    const toggle = document.querySelector<HTMLButtonElement>("#conversation-raw-output");
    toggle?.setAttribute("aria-expanded", "false");
    toggle?.focus();
  });
  return opened;
}

/** Open the read-only Raw output drawer over the open conversation (cas-0546). */
function openRawOutput(): void {
  const reason = rawOutputUnavailableReason();
  if (reason) {
    const thread = selectedMachineId && selectedSession ? sessionKey(selectedMachineId, selectedSession) : undefined;
    toast(reason, { thread, until: () => rawOutputUnavailableReason() !== reason });
    return;
  }
  if (!selectedMachineId || !selectedSession) return;
  const dialog = rawOutputDialog();
  rawOutputThread = sessionKey(selectedMachineId, selectedSession);
  const phrase = supervisorPhrase(selectedMachineId, selectedSession);
  dialog.querySelector<HTMLElement>("#raw-output-subject")!.textContent = `What ${phrase}'s terminal shows, as text. Read-only.`;
  if (!dialog.open) dialog.showModal();
  document.querySelector<HTMLButtonElement>("#conversation-raw-output")?.setAttribute("aria-expanded", "true");
  syncRawOutput();
}

function releaseRawOutputView(): void {
  if (!rawOutput) return;
  rawOutput.view.dispose();
  rawOutput.view.element.remove();
  rawOutput = undefined;
}

/**
 * Keep the open drawer on the open conversation's surface: it attaches once
 * the surface is up, follows a remount, and closes when the operator moves
 * to another conversation.
 */
function syncRawOutput(): void {
  const dialog = document.querySelector<HTMLDialogElement>("#raw-output");
  if (!dialog?.open) { releaseRawOutputView(); return; }
  const thread = selectedMachineId && selectedSession ? sessionKey(selectedMachineId, selectedSession) : undefined;
  if (thread !== rawOutputThread) { dialog.close(); return; }
  const current = selectedSurface();
  if (rawOutput && rawOutput.key !== current?.key) releaseRawOutputView();
  dialog.querySelector<HTMLElement>(".raw-output-empty")!.hidden = current !== undefined;
  if (!current) return;
  if (!rawOutput) {
    const view = new TranscriptView(document, current.surface.transcript);
    view.element.setAttribute("aria-label", "Raw output");
    dialog.querySelector<HTMLElement>(".raw-output-body")!.append(view.element);
    rawOutput = { key: current.key, view };
  }
  rawOutput.view.update();
}

function hubSupports(machineId: string, capability: string): boolean {
  return machineInfo.get(machineId)?.capabilities.includes(capability) === true;
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
  if (!attach || attach.phase === "live" || attach.phase === "idle") toast("The conversation is reconnecting", { thread: sessionKey(machineId, session) });
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
 * (cas-7b31, journey F2). Control is taken back once the session is live
 * again, unless another device took it meanwhile (the hub refuses that take)
 * or the pairing was refused.
 */
const controlLostToOutage = new Set<string>();

function invalidateMachineLeases(machineId: string): void {
  for (const [key, timer] of leaseHeartbeats) {
    if (key.startsWith(`${machineId}:`)) { window.clearInterval(timer); leaseHeartbeats.delete(key); }
  }
  for (const [key, lease] of leases) {
    if (!key.startsWith(`${machineId}:`)) continue;
    if (lease.held_by_me) controlLostToOutage.add(key);
    // The controller identity was learned over the connection that just died.
    // Keeping it told the operator that another controller — in fact this very
    // browser — was holding the session against them.
    leases.set(key, { ...lease, held_by_me: false, controller_label: undefined, controller_device_id: undefined });
  }
  // cas-7b31 (journey F2): nothing is said here. The header, the banner and
  // the conversation's controls already say the session is down, and control
  // comes back by itself.
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
/**
 * cas-97d58 F10/F11: a toast about one conversation, or about a state that
 * can end (an outage's "returns when it reconnects"), belongs to it. It is
 * retired as soon as the operator opens another conversation or the state
 * ends, instead of outliving it on screen or in the accessibility tree.
 */
let toastScope: { thread?: string; until?: () => boolean } | undefined;

function dismissToast(): void {
  const output = document.querySelector<HTMLElement>("#toast");
  if (toastTimer !== undefined) window.clearTimeout(toastTimer);
  toastTimer = undefined;
  toastScope = undefined;
  if (!output) return;
  output.classList.remove("visible");
  // Hidden text in a status region is still read and still in the tree.
  output.textContent = "";
}

/** Retire a scoped toast whose conversation is no longer open or whose state has ended. */
function retireStaleToast(): void {
  if (!toastScope) return;
  const open = selectedMachineId && selectedSession ? sessionKey(selectedMachineId, selectedSession) : undefined;
  if ((toastScope.thread !== undefined && toastScope.thread !== open) || toastScope.until?.()) dismissToast();
}

function toast(message: string, scope?: { thread?: string; until?: () => boolean }): void {
  let output = document.querySelector<HTMLElement>("#toast");
  if (!output) {
    output = document.createElement("div");
    output.id = "toast";
    output.setAttribute("role", "status");
    document.body.append(output);
  }
  output.textContent = message;
  toastScope = scope;
  placeToastClearOfBanner(output);
  output.classList.add("visible");
  if (toastTimer !== undefined) window.clearTimeout(toastTimer);
  // Timing out only hides it, as before; a scoped toast is also emptied once
  // its conversation closes or its state ends (retireStaleToast).
  toastTimer = window.setTimeout(() => output.classList.remove("visible"), 3200);
}

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
const readMarkStorage = readMarkStore(conversationStorage);
const readMarks: Map<string, number> = readMarkStorage.load();
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
  const machine = machines.get(machineId);
  if (machine) void sendJournal.purge(machineId, credentialFence(machine)).catch(() => { showComposerStatus("Browser storage refused to remove this machine's saved conversation. Retry removal before using it again.", "error"); });
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
  conversationBack: (() => document.querySelector<HTMLElement>("#conversation-back")) as FocusTarget,
};
function landFocus(targets: readonly FocusTarget[], options: { keep?: boolean; nextTask?: boolean; waitMs?: number; since?: Element | null } = {}): void {
  // `nextTask` waits a task, not a microtask: a dialog's cancel event runs
  // before the dialog hands focus back, and a tap's deferred render runs in
  // the task after its click (DeferredRenderScheduler.afterGesture).
  // `waitMs` keeps trying, a frame at a time, while the target is still
  // mounting (a thread whose history is loading, a session still attaching),
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
 * thread. Focus anywhere else is left alone (cas-9a96).
 */
function handFocusFromConnectionCard(grid: HTMLElement): void {
  const card = grid.querySelector<HTMLElement>(":scope > .empty:is(.terminal-state, .conversation-opening)");
  const active = document.activeElement;
  if (!card || !(active instanceof HTMLElement) || !card.contains(active)) return;
  // After this render replaces the card; `since` is the card's control, so a
  // control the operator moves to meanwhile is theirs and is not taken back.
  landFocus([focusTargets.composer, focusTargets.thread], { keep: true, nextTask: true, waitMs: 2_000, since: active });
}

/** A tap from a touch screen or pen; keyboard (detail 0) and mouse are not. */
function touchActivation(event: MouseEvent | undefined): boolean {
  return Boolean(event && event.detail > 0 && !finePointerClick(event));
}

/**
 * After opening a conversation from a list row: the keyboard and the mouse
 * land where the next keystroke belongs, as a palette jump does; a touch
 * lands on the thread to read, without raising a soft keyboard (cas-7eaf).
 */
function landAfterOpen(opened: Promise<void>, event: MouseEvent | undefined): void {
  if (!touchActivation(event)) { focusJumpedComposer(opened); return; }
  // The thread may only mount once the session's history loads, so land
  // again when the open settles unless the operator has moved on.
  landFocus([focusTargets.thread, focusTargets.conversationBack], { keep: true, nextTask: true, waitMs: 2_000 });
}

/** After a palette jump, hand focus to the opened conversation's composer
 * (restoreMessageDraft already put its caret back). Where the composer cannot
 * take focus yet, the thread takes it once the attach settles, unless the
 * operator has moved focus themselves. */
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
    // The composer or the thread once they are up (attaching takes a
    // moment): never <body> (cas-7eaf). Focus the operator moved themselves
    // is kept.
    landFocus([focusTargets.composer, focusTargets.thread, focusTargets.conversationBack], { keep: true, waitMs: 1_500, since: pickedFrom });
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
  if (leases.get(key)?.held_by_me) controlTakenAfterRefusal.add(key); else controlTakenAfterRefusal.delete(key);
}

async function takeControlForMessage(machine: StoredMachine, session: string, force = false): Promise<boolean> {
  try {
    await connections.get(machine.id)?.requestControl(session, force);
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
  const deadline = history?.nextReceiptCheck(Date.now());
  const cue = history?.nextConfirmCue(Date.now());
  const wait = deadline === undefined ? cue : cue === undefined ? deadline : Math.min(deadline, cue);
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
    } else updateConversationViews(); // a due confirmation cue (cas-97d58 F18)
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
  if (heldSends.get(key)?.some((held) => held.clientRef === clientRef)) return;
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
const sendStore = pendingSendStore(conversationStorage); // one-time legacy import only
const storedSends: Map<string, PendingSend[]> = sendStore.load();
const persistedSends = new Map<string, PendingSend[]>();
const pendingThreadScopes = new Map<string, string>();
const restoredMachines = new Map<string, Promise<void>>();
let journalWrites = Promise.resolve();
const sendJournal = new CommanderJournal(window.indexedDB, async (scope: DeliveryScope) => {
  if (conversationPersistenceBlocked.has(scope.hub)) return undefined;
  const snapshot = await catalog.snapshot();
  const machine = snapshot.machines.find((item) => item.id === scope.hub && item.baseUrl === scope.baseUrl && item.deviceId === scope.device);
  return machine && deliveryScope(machine, scope.session).accountState !== "unsupported" ? credentialFence(machine) : undefined;
});
let journalSync = Promise.resolve();
sendJournal.onChange = () => {
  journalSync = journalSync.then(async () => {
    await journalWrites;
    for (const machine of machines.values()) {
      if (conversationPersistenceBlocked.has(machine.id)) continue;
      await restoredMachines.get(machine.id);
      for (const scope of await sendJournal.scopes(machine)) {
        const key = sessionKey(machine.id, scope.session);
        const snapshot = await sendJournal.read(scope);
        pendingThreadScopes.set(key, scopeKey(scope));
        persistedSends.set(key, snapshot.sends);
        const history = conversationHistory(key, scope.session);
        const current = new Map(snapshot.sends.map((send) => [send.id, send]));
        const queue = heldSends.get(key) ?? [];
        for (const held of [...queue]) if (current.get(held.clientRef)?.state !== "held") {
          clearTimeout(held.expiry); queue.splice(queue.indexOf(held), 1);
        }
        if (!queue.length) heldSends.delete(key);
        settleHeldComposerStatus(machine.id, scope.session);
        for (const held of history.synchronizePending(snapshot.sends, Date.now(), snapshot.receipts)) {
          const remaining = HELD_SEND_MS - (Date.now() - (held.heldAt ?? held.at));
          if (remaining <= 0) expireHeldSend(machine, key, held.id);
          else queueHeldSend(machine, key, held.id, held.target, held.text, held.replyTo, remaining);
        }
        scheduleReceiptCheck(key);
        for (const row of snapshot.replies) {
          if (isOperatorNotice(row.reply)) continue;
          history.hydrateKeptReply({ ...row.reply, device_persisted: true, at: new Date(row.persistedAt).toISOString(), session: scope.session });
        }
        if (sessionIsUp(machine.id, scope.session)) void flushHeldSends(machine, scope.session);
      }
    }
    updateConversationViews(); renderConversationList();
  }).catch(() => { /* a later broadcast retries; no storage failure grants dispatch */ });
};

function persistPendingSends(): Promise<void> {
  for (const [key, history] of conversationHistories) {
    if (storedSends.has(key)) continue;
    const separator = key.indexOf(":");
    const machine = machines.get(key.slice(0, separator));
    if (!machine || conversationPersistenceBlocked.has(machine.id)) continue;
    const session = key.slice(separator + 1);
    if (pendingThreadScopes.has(key) && pendingThreadScopes.get(key) !== scopeKey(deliveryScope(machine, session))) continue;
    const after = history.pendingSends().map((send) => send.state === "held" ? { ...send, heldAt: heldSince.get(send.id) ?? send.at } : send);
    journalWrites = journalWrites.then(async () => {
      // Revocation can occur while this write waits behind another transaction.
      // The privacy fence is checked when it executes, not only when scheduled.
      if (conversationPersistenceBlocked.has(machine.id)) return;
      const before = persistedSends.get(key) ?? [];
      for (const event of history.events) {
        if (event.kind !== "send" || event.value.notificationId === undefined || !before.some(send => send.id === event.value.id)) continue;
        await sendJournal.acknowledge(deliveryScope(machine, session), {
          client_ref: event.value.id, notification_id: event.value.notificationId,
          target: event.value.target, stamped: event.value.stamped ?? false,
          ...(event.value.deviceLabel === undefined ? {} : { device_label: event.value.deviceLabel }),
        }, credentialFence(machine));
      }
      if (JSON.stringify(before) === JSON.stringify(after)) return;
      const result = await sendJournal.reconcile(deliveryScope(machine, session), before, after, credentialFence(machine));
      // A revoked scope is deliberately refused, not a browser storage error.
      if (conversationPersistenceBlocked.has(machine.id)) return;
      if (result === "kept") persistedSends.set(key, after);
      else if (selectedMachineId === machine.id && selectedSession === session) showComposerStatus(result === "too-long"
        ? "This message is too long to keep in browser storage. Edit it before sending."
        : "Browser storage could not keep this message. It has not been sent; keep this page open and retry.", "error");
    }).catch(() => { /* the next explicit operation reports storage failure */ });
  }
  return journalWrites;
}

function restoreStoredSends(machine: StoredMachine): void {
  if (restoredMachines.has(machine.id)) return;
  const restore = (async () => {
    for (const [key, sends] of [...storedSends]) {
      if (!key.startsWith(`${machine.id}:`) || conversationPersistenceBlocked.has(machine.id)) continue;
      const session = key.slice(machine.id.length + 1);
      await sendJournal.importLegacy(deliveryScope(machine, session), sends, credentialFence(machine));
      // Delete the legacy namespace only after the import transaction commits.
      sendStore.save(key, []);
      storedSends.delete(key);
    }
    for (const scope of await sendJournal.scopes(machine)) {
      if (conversationPersistenceBlocked.has(machine.id)) return;
      const snapshot = await sendJournal.read(scope);
      const key = sessionKey(machine.id, scope.session);
      pendingThreadScopes.set(key, scopeKey(scope));
      persistedSends.set(key, snapshot.sends);
      const history = conversationHistory(key, scope.session);
      for (const row of snapshot.replies) {
        if (isOperatorNotice(row.reply)) continue;
        history.hydrateKeptReply({ ...row.reply, device_persisted: true, at: new Date(row.persistedAt).toISOString(), session: scope.session });
      }
      for (const held of history.synchronizePending(snapshot.sends, Date.now(), snapshot.receipts)) {
        const since = held.heldAt ?? held.at;
        heldSince.set(held.id, since);
        const remaining = HELD_SEND_MS - (Date.now() - since);
        if (remaining <= 0) expireHeldSend(machine, key, held.id);
        else queueHeldSend(machine, key, held.id, held.target, held.text, held.replyTo, remaining);
      }
      scheduleReceiptCheck(key);
    }
  })().catch(() => { showComposerStatus("Browser storage could not restore kept messages. Retry before sending.", "error"); });
  restoredMachines.set(machine.id, restore);
  void restore.then(() => {
    updateConversationViews(); renderConversationList();
    for (const session of sessions.get(machine.id) ?? []) if (sessionIsUp(machine.id, session.name)) void flushHeldSends(machine, session.name);
  });
}

async function persistDeviceReply(machine: StoredMachine, session: string, reply: OperatorReply, frameFence?: CredentialFence): Promise<void> {
  // Never authorize a conversation reply ACK for an attention-only notice.
  if (isOperatorNotice(reply)) return;
  try {
  const fence = frameFence ?? credentialFence(machine);
  const scope = deliveryScope(machine, session);
  if (!await sendJournal.persistReply(scope, reply, fence)) { markReplyNotKept(machine, session, reply); return; }
  const current = (await catalog.snapshot()).machines.find((item) => item.id === machine.id && item.deviceId === machine.deviceId && item.baseUrl === machine.baseUrl);
  if (!current || current.credentialId !== fence.credentialId || credentialFence(current).generation !== fence.generation || conversationPersistenceBlocked.has(machine.id)) return;
  conversationHistory(sessionKey(machine.id, session)).markReplyPersisted(reply.notification_id);
  // Device identity is derived by the hub, never trusted from this frame.
  connections.get(machine.id)?.send(session, { OperatorReplyPersisted: { notification_id: reply.notification_id, device_id: "" } });
  updateConversationViews();
  } catch { markReplyNotKept(machine, session, reply); /* Display remains forwarded; failed storage cannot authorize ACK. */ }
}

/** cas-97d58 F05: say "Not kept on this device yet" only after storing it actually failed. */
function markReplyNotKept(machine: StoredMachine, session: string, reply: OperatorReply): void {
  conversationHistory(sessionKey(machine.id, session)).markReplyStoreFailed(reply.notification_id);
  updateConversationViews();
}

async function cancelWaitingMessage(machineId: string, session: string, id: string): Promise<void> {
  const machine = machines.get(machineId);
  if (!machine) return;
  try {
    await journalWrites;
    if (!await sendJournal.cancel(deliveryScope(machine, session), id)) {
      showComposerStatus("This message may already have been sent. Wait for its receipt before retrying.", "info");
      return;
    }
    const key = sessionKey(machineId, session);
    const queue = heldSends.get(key) ?? [];
    for (const held of [...queue]) if (held.clientRef === id) { clearTimeout(held.expiry); queue.splice(queue.indexOf(held), 1); }
    if (!queue.length) heldSends.delete(key);
    heldSince.delete(id);
    // cas-97d58 F15: a cancel is the operator's own decision, not a failure.
    // The message leaves the thread (as it does after a reload) instead of
    // turning into a red "unsent message" chip; the composer says it was not sent.
    conversationHistory(key).reject(id, "Cancelled — not sent.");
    conversationHistory(key).discardRefused(id);
    updateConversationViews(); renderConversationList();
    showComposerStatus("Waiting message cancelled. It was not sent.", "info");
    document.querySelector<HTMLElement>("#message-text")?.focus({ preventScroll: true });
  } catch { showComposerStatus("Browser storage could not cancel this waiting message. Keep this page open and retry.", "error"); }
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

/**
 * cas-387e: the "will go out by itself" line belongs to messages still held
 * for this conversation. Once none is held (flushed, Delivered, refused, or
 * sent by another tab and seen through the journal), it clears; while some
 * are, it follows the connection's wording. `live` clears it outright: the
 * session is up and nothing waits on the connection.
 */
function settleHeldComposerStatus(machineId: string, session: string, live = false): void {
  const key = sessionKey(machineId, session);
  if (!messageStatus?.held || messageStatus.session !== key) return;
  if (live || !heldSends.get(key)?.length) {
    clearComposerStatus();
    return;
  }
  if (selectedMachineId === machineId && selectedSession === session && heldSendStatus(machineId, session) !== messageStatus.text) showHeldSendStatus(machineId, session);
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
  await restoredMachines.get(machine.id);
  await persistPendingSends();
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
      const accepted = machines.get(machine.id);
      if (!accepted || scopeKey(deliveryScope(accepted, session)) !== scopeKey(deliveryScope(machine, session))) break;
      const result = await sendJournal.dispatch(deliveryScope(machine, session), held.clientRef, credentialFence(accepted),
        () => !conversationPersistenceBlocked.has(machine.id) && !!connections.get(machine.id)?.send(session, supervisorMessage(held.supervisor, held.text, held.clientRef, held.replyTo)));
      if (result === "waiting" || result === "not-saved") break;
      queue.shift();
      clearTimeout(held.expiry);
      history.release(held.clientRef);
      if (result === "expired") history.reject(held.clientRef, outageRefusal(machine.label));
      else if (result !== "written") {
        // Another tab may own this send. A lost claim is not a failed wire
        // write: project its durable state and original receipt deadline.
        const snapshot = await sendJournal.read(deliveryScope(machine, session));
        history.synchronizePending(snapshot.sends, Date.now(), snapshot.receipts);
      }
    }
    if (queue.length === 0) heldSends.delete(key);
    scheduleReceiptCheck(key);
  } finally {
    flushingHeldSends.delete(key);
    settleHeldComposerStatus(machine.id, session);
    updateConversationViews(); renderConversationList();
  }
}

/** "the cas-src supervisor", never the generated codename (cas-71f4, journey F20). */
function supervisorPhrase(machineId: string, session: string): string {
  const project = projectTitle(visibleSessions(machineId).find((item) => item.name === session)?.project_dir);
  return project ? `the ${project} supervisor` : "the supervisor";
}

async function deliverSupervisorMessage(machine: StoredMachine, session: string, supervisor: string, text: string, replyTo?: number, retryOf?: string, editOf?: string): Promise<void> {
  await restoredMachines.get(machine.id);
  if (!sessionIsUp(machine.id, session) && !machineWillReconnect(machine.id)) {
    showComposerStatus(connectionStates.get(machine.id)?.authFailure ? `${pairingRefusal(machine.label)} Your message is kept; re-pair, then send it.` : outageRefusal(machine.label), "error", true);
    return;
  }
  const key = sessionKey(machine.id, session);
  const history = conversationHistory(key, session);
  if (pendingThreadScopes.has(key) && pendingThreadScopes.get(key) !== scopeKey(deliveryScope(machine, session))) {
    showComposerStatus("This conversation's kept messages belong to another pairing. Reopen it before sending from this device.", "error");
    return;
  }
  pendingThreadScopes.set(key, scopeKey(deliveryScope(machine, session)));
  await journalWrites;
  const scope = deliveryScope(machine, session);
  if (retryOf) {
    const snapshot = await sendJournal.read(scope);
    history.synchronizePending(snapshot.sends, Date.now(), snapshot.receipts);
    const previous = history.events.find(event => event.kind === "send" && event.value.id === retryOf);
    if (previous?.kind !== "send" || !history.canRetrySend(previous.value)) {
      updateConversationViews(); renderConversationList();
      return;
    }
  }
  const clientRef = retryOf ?? crypto.randomUUID();
  const at = Date.now();
  const send: PendingSend = { id: clientRef, target: supervisor, text, state: "held", at, heldAt: at, session, ...(replyTo === undefined ? {} : { replyTo }) };
  const result = retryOf
    ? await sendJournal.retry(scope, retryOf, credentialFence(machine), send)
    : await sendJournal.reconcile(scope, [], [send], credentialFence(machine));
  if (result === "delivered") {
    const snapshot = await sendJournal.read(scope);
    history.synchronizePending(snapshot.sends, Date.now(), snapshot.receipts);
    updateConversationViews(); renderConversationList();
    return;
  }
  if (result !== "kept") {
    showComposerStatus(retryOf ? "This message's delivery state could not be confirmed for a retry. Wait for its receipt before retrying."
      : result === "too-long" ? "This message is too long to keep in browser storage. Edit it before sending." : "Browser storage could not keep this message. It has not been sent; keep this page open and retry.", "error");
    return;
  }
  if (conversationPersistenceBlocked.has(machine.id)) return;
  if (retryOf) {
    // A live receipt can arrive while the Retry transaction commits.
    const previous = history.events.find(event => event.kind === "send" && event.value.id === retryOf);
    if (previous?.kind !== "send" || !history.canRetrySend(previous.value)) {
      updateConversationViews(); renderConversationList();
      return;
    }
    history.discardRefused(retryOf);
  }
  if (editOf) { history.retireRefused(editOf); editingRefused = undefined; }
  // cas-97d58 F10: "Not sent — see the message above" pointed at the refused
  // message this send replaces. With no refused message left to point at,
  // it would sit under the new Sending… bubble, so it goes.
  if ((retryOf || editOf) && messageStatus?.session === key && messageStatus.text === REFUSED_SEE_ABOVE
    && !history.visibleEvents().some((event) => event.kind === "send" && history.isFailedSend(event.value))) clearComposerStatus();
  holdSupervisorMessage(machine, session, clientRef, supervisor, text, replyTo);
  updateConversationViews(); renderConversationList();
  if (selectedMachineId === machine.id && selectedSession === session) {
    const composer = document.querySelector<HTMLTextAreaElement>("#message-text");
    if (composer && composer.value.trim() === text) composer.value = "";
    rememberDraft(key, undefined);
    messageDraft = composer?.value ?? "";
    messageDraftSelection = messageDraft.length;
    // cas-387e: only a send that actually waits says it will go out by
    // itself; on a live session it goes now, and the bubble's own Sending…
    // and Delivered say so.
    if (sessionIsUp(machine.id, session)) settleHeldComposerStatus(machine.id, session, true);
    else showHeldSendStatus(machine.id, session);
    composer?.focus();
  }
  if (sessionIsUp(machine.id, session)) await flushHeldSends(machine, session);
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
    // cas-cff2: control is held now, so "Taking control…" is done. The send
    // itself speaks next: the bubble's Sending… and Delivered, or the
    // waiting line if the connection holds it. Left up, it outlived the
    // reply beside a Delivered message.
    if (messageStatus?.text === plan.notice) clearComposerStatus();
  }
  await deliverSupervisorMessage(machine, session, supervisor, text, replyTo, quick?.retryOf, editOf);
  } finally {
    pendingSubmissions.delete(submissionKey);
  }
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

interface PairDialogPlace { readonly scroll: readonly number[]; readonly focus?: { readonly index: number; readonly tag: string } }
/** Where the operator is in the open pairing dialog: its scroll and the control they are on. */
function pairDialogPlace(): PairDialogPlace | undefined {
  const dialog = document.querySelector<HTMLDialogElement>("#pair-dialog");
  const form = dialog?.querySelector<HTMLFormElement>("#pair-form");
  if (!dialog || !form) return undefined;
  const active = document.activeElement;
  const index = active && dialog.contains(active) ? [...dialog.querySelectorAll("*")].indexOf(active) : -1;
  return { scroll: [dialog.scrollTop, form.scrollTop], ...(active && index >= 0 ? { focus: { index, tag: active.tagName } } : {}) };
}
/** The same step's markup is rebuilt node for node, so the control at the same place is the one the operator was on. */
function restorePairDialogPlace(place: PairDialogPlace): void {
  const dialog = document.querySelector<HTMLDialogElement>("#pair-dialog");
  const form = dialog?.querySelector<HTMLFormElement>("#pair-form");
  if (!dialog || !form) return;
  const target = place.focus ? dialog.querySelectorAll("*")[place.focus.index] : undefined;
  if (target instanceof HTMLElement && target.tagName === place.focus?.tag) target.focus({ preventScroll: true });
  [dialog.scrollTop, form.scrollTop] = place.scroll;
}

/** Each machine's accent, recorded when it first pairs so later pairings never re-colour it (cas-50a7). */
const machineAccentStore = storageAccentStore((() => { try { return window.localStorage; } catch { return undefined; } })());

function render(captureDraft = true): void {
  // Every row, header and accent below reads its colour from this fleet.
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
  // (a header button, the list search's neighbours) would drop to <body> and
  // their next Enter would do nothing, so the rebuilt control with the same
  // id takes focus back. Dialogs and the composer have their own rules below.
  const focusedControl = document.activeElement instanceof HTMLElement && document.activeElement.id
    && document.activeElement.id !== "message-text" && app.contains(document.activeElement)
    && !document.activeElement.closest("dialog") ? document.activeElement.id : undefined;
  const selected = selectedMachineId ? machines.get(selectedMachineId) : undefined;
  const status = selected && selectedSession ? statuses.get(sessionKey(selected.id, selectedSession)) : undefined;
  const compatibility = selected ? compatibilityWarning(selected.id) : undefined;
  const machineConnectionSnapshot = selected ? connectionStates.get(selected.id) : undefined;
  const attachSnapshot = selected && selectedSession ? attachStates.get(sessionKey(selected.id, selectedSession)) : undefined;
  const connectionSnapshot = attachSnapshot ?? machineConnectionSnapshot;
  // The header reads the same one connection state as the banner, the row and
  // the footer (cas-a447).
  const headerConnection = selected && selectedSession ? conversationConnection(selected.id, selectedSession) : machineConnectionSnapshot;
  const sessionDown = Boolean(selected && selectedSession) && headerConnection !== undefined && headerConnection.phase !== "live";
  const selectedHubSession = selected && selectedSession
    ? sessions.get(selected.id)?.find((item) => item.name === selectedSession)
    : undefined;
  const supervisor = supervisorTarget(selectedHubSession);
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
  // Workers and tasks keep rendering the last snapshot while a hub is
  // unreachable. Presented unlabelled, that reads as current truth.
  const statusIsStale = Boolean(selected) && machineConnectionSnapshot !== undefined
    && ((machineConnectionSnapshot.phase !== "live" && headerConnection?.phase !== "live") || machineConnectionSnapshot.degraded);
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
    || (attachSnapshot?.authFailure !== undefined));
  // cas-a6f0 (journey F8): a machine that still reads live with its
  // heartbeats unanswered is unsteady, not reconnecting, as the header says.
  const unsteadyHere = machineConnectionSnapshot?.phase === "live" && machineConnectionSnapshot.degraded && !sessionDown;
  const staleStatusText = statusIsStale
    ? (pairingLostHere ? `Not live — this browser needs pairing again.${staleStatusTail}`
      : machineConnectionSnapshot?.fatal === true ? `Not live — ${fatalConnectionRecovery(machineConnectionSnapshot.reason)}${staleStatusTail}`
      : unsteadyHere ? `${UNSTEADY_SENTENCE}${staleStatusTail}`
      : `Not live — reconnecting.${staleStatusTail}`)
    : undefined;
  const selectedThreadKey = selected && selectedSession ? sessionKey(selected.id, selectedSession) : undefined;
  const infoItems = dismissableInfoItems(attention);
  const sessionCommands = [...machines.values()].flatMap((machine) => visibleSessions(machine.id).map((session) => {
    // cas-786a (journey F30): the open conversation and those that need the
    // operator are marked, so palette Enter moves on to the next one that does.
    const current = machine.id === selectedMachineId && session.name === selectedSession;
    return sessionJumpCommandMarkup(machine, session, sessionSummaries.get(sessionKey(machine.id, session.name)), { current, needsYou: conversationNeedsYou(machine.id, session.name) });
  })).join("");
  // The browser tab and a screen reader's window title name the open
  // conversation too, not only the app (cas-3400 QA; cas-cf10 QA F03): the
  // project leads, the generated codename follows (3.30.0 journey F2).
  const selectedProject = projectTitle(selectedHubSession?.project_dir);
  const sessionTitleText = selectedSession ? [selectedProject ?? selectedSession, selectedProject ? selectedHubSession?.supervisor || selectedSession : undefined].filter(Boolean).join(" ") : "";
  const documentTitle = selectedSession ? `${sessionTitleText} — Cassy Cloud` : "Cassy Cloud";
  if (document.title !== documentTitle) document.title = documentTitle;
  const liveRegions: LiveRegionView = {
    ...(staleStatusText ? { staleNotice: staleStatusText } : {}),
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
    // header and the list read that label.
    machineIds: [...machines.values()].map((machine) => `${machine.id}:${machine.label}`),
    sessionKeys: [...machines.keys()].flatMap((id) => visibleSessions(id).map((item) => `${id}/${item.name}`)),
    catalogLoaded: machineCatalogLoaded,
    supervisor,
    compatibility,
    commandPaletteOpen,
    pairingView,
  }) + JSON.stringify([selectedHubSession?.project_dir, infoItems.length > 0, launchAvailability()]);
  const active = document.activeElement;
  // Focus anywhere inside the open palette counts as composing too: a rebuild
  // would replace the dialog under a focused row, wipe its filter and leave
  // Enter to run whatever command now leads (cas-9648 QA F02).
  const inOpenPalette = active instanceof HTMLElement && active.closest("#command-palette[open]") !== null;
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
    renderRegions({ selected, session: selectedSession, status, connectionSnapshot, liveRegions });
    return;
  }
  deferredRender.settled();
  // A lease/connection change can rebuild the shell while the same notice
  // is being copied. Keep that view's panel so payload reconciliation can
  // retain its actual Details and Copy nodes, not only restore their focus.
  const attentionPanel = document.querySelector<HTMLElement>("#attention-panel");
  const preservedAttention = attentionPanel?.dataset.viewScope === attentionViewScope() ? attentionPanel : undefined;
  const attentionFocus = preservedAttention?.contains(document.activeElement) ? document.activeElement as HTMLElement : undefined;
  preservedAttention?.remove();
  const currentGrid = document.querySelector<HTMLElement>("#pane-grid");
  const machineDialog = document.querySelector<HTMLDialogElement>("#paired-machines-dialog");
  const pairedDialogWasOpen = machineDialog?.open === true;
  if (pairedDialogWasOpen) machineDialog!.remove();
  const pairDialogWasOpen = document.querySelector<HTMLDialogElement>("#pair-dialog")?.open === true;
  const pairingFeedbackWasFocused = pairDialogWasOpen && document.activeElement?.matches("#pair-dialog .pair-status") === true;
  // A rebuild for something else (a machine connecting) redraws the same
  // pairing step; the operator keeps their place in it rather than the
  // dialog's autofocus pulling them back to the top (cas-207a).
  const pairPlace = pairDialogWasOpen && pairingView === lastPairingView ? pairDialogPlace() : undefined;
  // The open conversation's grid (its thread, connection card and hidden pane
  // host) survives a rebuild, so a heartbeat never remounts the thread.
  const preservedGrid = selectedThreadKey && currentGrid?.dataset.sessionKey === selectedThreadKey ? currentGrid : undefined;
  if (preservedGrid) {
    preservedGrid.remove();
  } else {
    for (const [key, surface] of surfaces) releaseSurface(key, surface);
  }
  app.innerHTML = `
    ${browserNotice ? `<p class="browser-unsupported" role="alert">${escapeHtml(browserNotice)}</p>` : ""}
    <div id="conversation-shell-anchor" hidden></div>
    <dialog id="command-palette" class="command-palette">
      <section>
        <header><strong>Commands</strong><button id="command-palette-close" type="button" aria-label="Close command palette">×</button></header>
        <input id="command-palette-query" type="search" aria-label="Filter commands" placeholder="Type a command or conversation">
        <div class="palette-commands">
          <section class="palette-group" data-palette-group="conversations" aria-labelledby="palette-group-conversations">
            <h3 id="palette-group-conversations" class="palette-group-heading">Conversations</h3>
            ${sessionCommands || '<p class="palette-empty">No live conversations yet.</p>'}
          </section>
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
            <button type="button" class="palette-command" data-palette-action="dormant"><span>${escapeHtml(dormantCommandLabel(revealDormant).title)}</span><small>${escapeHtml(dormantCommandLabel(revealDormant).hint)}</small></button>
          </details>
          <p class="palette-empty" id="palette-no-match" role="status" hidden></p>
        </div>
      </section>
    </dialog>
    ${pairedMachinesDialogMarkup()}
    ${pairDialogMarkup()}`;
  if (pairedDialogWasOpen && machineDialog) {
    document.querySelector('#paired-machines-dialog')?.replaceWith(machineDialog);
    machineDialog.close(); machineDialog.showModal();
  }
  // Only what the conversation shell shows is built (cas-0546): the thread's
  // grid, the composer, the session's status and its own attention.
  const regions = selectedSession ? conversationRegions(preservedGrid, selectedThreadKey, supervisor) : {};
  app.querySelector("#conversation-shell-anchor")!.replaceWith(arrangeConversationShell(document, { selected: Boolean(selectedSession), supervisor, projectDir: selectedHubSession?.project_dir, host: selected?.label, machineId: selectedSession ? selected?.id : undefined, loaded: machineCatalogLoaded, paired: machines.size > 0, searchQuery: conversationSearchQuery, keyboardHint: keyboardHintOffered(), launch: launchAvailability() }, regions));
  if (preservedAttention) document.querySelector<HTMLElement>("#attention-panel")?.replaceWith(preservedAttention);
  // A toast raised before the shell changed (a conversation opening while
  // "connected" is up) follows the new layout rather than covering a heading.
  const visibleToast = document.querySelector<HTMLElement>("#toast.visible");
  if (visibleToast) placeToastClearOfBanner(visibleToast);
  restoreMessageDraft();
  if (composerWasFocused) queueMicrotask(() => document.querySelector<HTMLTextAreaElement>("#message-text")?.focus());
  if (threadWasFocused && !composerWasFocused) landFocus([focusTargets.thread], { keep: true, waitMs: 500 });
  if (threadControl && !composerWasFocused) landFocus([() => threadControl], { keep: true, waitMs: 500 });
  lastShellSignature = signature;
  lastPairingView = pairingView;
  bindEvents();
  if (commandPaletteOpen) {
    document.querySelector<HTMLDialogElement>("#command-palette")?.showModal();
    queueMicrotask(() => document.querySelector<HTMLInputElement>("#command-palette-query")?.focus());
  }
  if (pairDialogWasOpen) {
    const dialog = document.querySelector<HTMLDialogElement>("#pair-dialog");
    dialog?.showModal();
    if (pairPlace) restorePairDialogPlace(pairPlace);
    if (pairingFeedbackWasFocused) focusPairingFeedback(dialog);
  }
  renderRegions({ selected, session: selectedSession, status, connectionSnapshot, liveRegions });
  if (attentionFocus?.isConnected && document.activeElement === document.body) attentionFocus.focus({ preventScroll: true });
  // After the regions, not before: a control can be hidden in fresh shell
  // markup until its region shows it (the phone Attention badge, cas-a5c6),
  // and focus() on a hidden control does nothing, so a rebuild left focus on
  // the page.
  if (!composerWasFocused && focusedControl && (document.activeElement === document.body || document.activeElement === null)) {
    document.getElementById(focusedControl)?.focus({ preventScroll: true });
  }
}

/**
 * The regions the conversation shell holds for an open thread: its grid (the
 * one already on screen, when it is this thread's), the composer, the
 * session's status and its attention. Each keeps the id its updater reads.
 */
function conversationRegions(preservedGrid: HTMLElement | undefined, threadKey: string | undefined, supervisor: string | undefined): ConversationRegions {
  const build = (markup: string): HTMLElement => {
    const template = document.createElement("template");
    template.innerHTML = markup;
    return template.content.firstElementChild as HTMLElement;
  };
  const grid = preservedGrid ?? build(`<section id="pane-grid" class="pane-grid"${threadKey ? ` data-session-key="${escapeAttr(threadKey)}"` : ""}><div class="empty"></div></section>`);
  return {
    grid,
    composer: build(composerMarkup(supervisor)),
    status: build('<div id="status-view"></div>'),
    attention: build('<section id="attention-panel" class="attention-panel"></section>'),
  };
}

interface RegionContext {
  readonly selected: StoredMachine | undefined;
  readonly session: string | undefined;
  readonly status: Record<string, unknown> | undefined;
  readonly connectionSnapshot: ConnectionState | undefined;
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
  renderAttention();
  renderStatus(context.status);
  syncConversationContext();
  applyLiveRegions(app, context.liveRegions);
  if (context.selected && context.session && context.connectionSnapshot) {
    renderConnectionSurface(context.selected.id, context.session, context.connectionSnapshot);
  }
  syncConnectionViewTicker();
  syncEarlyThread();
  syncConversationActions();
  if (context.selected && context.session) {
    const machineId = context.selected.id;
    const session = context.session;
    const state = sessionStates.get(sessionKey(machineId, session));
    if (state) queueMicrotask(() => void renderSessionState(machineId, session, state));
  }
  syncPairingCountdown();
}

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
    const replyIds = events.flatMap((event) => event.kind === "reply" ? [event.value.notification_id] : []);
    const replies = replyIds.length;
    if (selected) {
      readReplies.set(key, replies);
      // cas-97d58 F14: remember the newest reply read, across reloads.
      const newest = replyIds.reduce((max, id) => Number.isSafeInteger(id) && id > max ? id : max, readMarks.get(key) ?? -1);
      if (newest >= 0 && newest !== readMarks.get(key)) { readMarks.set(key, newest); readMarkStorage.save(key, newest); }
    }
    const mark = readMarks.get(key);
    const unreadCount = mark === undefined ? Math.max(0, replies - (readReplies.get(key) ?? 0)) : replyIds.filter((id) => id > mark).length;
    // Waiting (ochre dot, hot time) is driven by asks and blockers the
    // operator has not answered, never by attention events (cas-0546).
    const waiting = waitingOnOperator(conversationHistories.get(key));
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
    return { key, machineId: machine.id, session: session.name, supervisor: session.supervisor, projectDir: session.project_dir, host: machine.label, activityAt: active, ...(Number.isFinite(started) ? { startedAt: started } : {}), canEnd: fleetControlGate(machine.scopes, "end-session", location.origin).allowed, freshness: activityLabel ?? "No activity seen yet", when: time?.short, whenSpoken: time?.spoken, preview: conversationHistories.get(key)?.preview(), draft: conversationDrafts.get(key)?.text, activityLine: plainActivity(session.last_activity), unreachable: Boolean(session.unreachable), connection: session.unreachable ? "Unreachable · message pending" : session.dormant ? "Dormant" : session.liveness === "live" ? conversationStatusLabel(machine.id, session.name) : "Session unavailable", interrupted: session.liveness === "live" && INTERRUPTED_LABELS.has(conversationStatusLabel(machine.id, session.name)), attention: waiting, unread: unreadCount, selected };
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
    const listState = rows.length > 0 ? { kind: "text" as const, text: conversationNoMatchText(conversationSearchQuery) } : conversationListState(machineCatalogLoaded, [...machines.keys()].map((id) => ({ catalogReceived: fleetCatalogUpdatedAt.has(id), phase: connectionStates.get(id)?.phase, browserBlocked: Boolean(connectionStates.get(id)?.networkAccessHelp) })));
    const markup = listState.kind === "loading" ? conversationSkeletonMarkup() : "";
    if (listState.kind === "loading") { if (empty.dataset.state !== "loading") empty.innerHTML = markup; }
    else empty.textContent = rows.length > 0 ? listState.text : [listState.text, keptMessagesLine()].filter(Boolean).join(" ");
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
  }
  // The state's width changes the room the machine · codename line has.
  fitConversationHost(document);
  refreshConnectionLog?.();
}

/**
 * cas-d043 G03: messages this browser kept during an outage survive a reload,
 * but until the machine answers there is no conversation to show them in, so
 * the empty list says they are kept and will go out.
 */
function keptMessagesLine(): string {
  const kept = new Map<string, number>();
  for (const [key, queue] of heldSends) {
    const machine = [...machines.values()].find((item) => key.startsWith(`${item.id}:`));
    if (machine && queue.length && !fleetCatalogUpdatedAt.has(machine.id)) kept.set(machine.label, (kept.get(machine.label) ?? 0) + queue.length);
  }
  return [...kept].map(([label, count]) => `${count === 1 ? "1 message" : `${count} messages`} kept for ${label} will go out once it answers.`).join(" ");
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
  return fleetConnectionLabel(conversationStatusState(machineId, session), machineId);
}

/** The state behind the conversation's connection word, which the Connection log reads too (cas-d043 G04). */
function conversationStatusState(machineId: string, session: string | undefined): ConnectionState | undefined {
  const machine = connectionStates.get(machineId);
  if (session && machine?.phase === "live") {
    const key = sessionKey(machineId, session);
    const attach = attachStates.get(key);
    if (attach && !sessionsEverLive.has(key) && (attachInProgress(attach) || firstAttachRetry(attach, false))) return machine;
  }
  return conversationConnection(machineId, session);
}

/**
 * A machine that has not been live in this visit is still on its first
 * connection, retries included: that is "Connecting", never "Reconnecting"
 * (journey F14).
 */
function fleetConnectionLabel(state: ConnectionState | undefined, machineId?: string): string {
  return machineConnectionLabel(state, machineId ? lastLiveAt.has(machineId) : true);
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

function attentionViewScope(): string {
  const view = [selectedMachineId, selectedSession];
  // A changed roster is a structural rebuild: its new panel restores the
  // corresponding notice/control. Transient lease/connection changes reuse it.
  const roster = [...sessions].flatMap(([id, entries]) => entries.map(entry => [id, entry.name]));
  return JSON.stringify([view, roster]);
}

function renderAttention(): void {
  const container = document.querySelector<HTMLElement>("#attention-panel");
  if (!container) return;
  container.dataset.viewScope = attentionViewScope();
  // The open conversation's own attention: its machine's alarms and its session's events.
  const visibleAttention = attention.filter((item) => item.machineId === selectedMachineId && (!item.session || item.session === selectedSession));
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
    ...(selectedMachineId ? { openConversation: { machineId: selectedMachineId, ...(selectedSession ? { session: selectedSession } : {}) } } : {}),
    sessionLabel: (item) => {
      const session = sessions.get(item.machineId)?.find((session) => session.name === item.session);
      return session ? [projectTitle(session.project_dir), session.supervisor].filter(Boolean).join(" · ") || undefined : undefined;
    },
  });
  // After the panel is drawn, so the sheet can hand focus back into it (cas-a5c6).
  syncConversationAttention(selectedSession ? coalesceAttention(visibleAttention).length : 0);
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
    if (count < 1 && selectedSession) closeAttentionSheet();
    else { attentionSheetOpen = false; badge?.setAttribute("aria-expanded", "false"); }
  }
  if (progressSheetOpen() && (!phoneLayout() || !selectedSession || progressSheetSession !== sessionKey(selectedMachineId ?? "", selectedSession))) progressSheetSession = undefined;
  applyAttentionSheet();
}
/** The phone Tasks sheet names whose tasks it holds, while it covers the header that does (cas-d043 G01). */
function syncProgressSheetWhere(): void {
  const heading = document.querySelector<HTMLElement>("#context-progress-heading");
  let line = document.querySelector<HTMLElement>(".conversation-context .context-sheet-where");
  const machine = selectedMachineId ? machines.get(selectedMachineId) : undefined;
  const session = selectedMachineId && selectedSession ? sessions.get(selectedMachineId)?.find((item) => item.name === selectedSession) : undefined;
  const where = progressSheetOpen() ? contextSheetWhere({ projectDir: session?.project_dir, supervisor: session?.supervisor, host: machine?.label }) : "";
  if (!where || !heading) { line?.remove(); return; }
  if (!line) { line = document.createElement("p"); line.className = "context-sheet-where"; line.id = "context-sheet-where"; heading.after(line); }
  if (line.textContent !== where) line.textContent = where;
}
/** The sheet control that last held focus, so a redraw that moves it hands focus back (cas-a5c6). */
let sheetFocus: HTMLElement | undefined;
/** The same control's redraw-proof key (cas-a5c6 QA F03). */
let sheetFocusKey: string | undefined;
function applyAttentionSheet(): void {
  syncProgressSheetWhere();
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
  // An Undo dismissed from the floating notice lives in the sheet (cas-2796a F03).
  renderStatus(statuses.get(progressSheetSession));
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
 * conversation's rail covers its own session. A machine that has never been
 * live in this visit is still connecting, not an outage.
 */
function attentionOutage(): { readonly text: string; readonly word: string } | undefined {
  // cas-a6f0 (journey F9): an unsteady machine is not "All clear" either.
  const covered: (readonly [string, ConnectionState | undefined])[] = selectedMachineId ? [[selectedMachineId, conversationConnection(selectedMachineId, selectedSession)]] : [];
  const down = covered
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
  return { text: `Not all clear. ${down.map((item) => item.text).join("; ")}.`, word: down.every((item) => item.phase === UNSTEADY) ? UNSTEADY : down.every((item) => item.fatal) ? BROWSER_UNSUPPORTED : "Reconnecting" };
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
/**
 * When each awaiting-merge task was last asked about, keyed by machine,
 * session and task id: the same task id on another machine or project is
 * another task (cas-d043 G13).
 */
const fleetAsked = new Map<string, number>();
const fleetAskedKey = (machineId: string, session: string, taskId: string): string => JSON.stringify([machineId, session, taskId]);
/** The open conversation's asks, by task id, as the rail reads them. */
function fleetAskedFor(machineId: string | undefined, session: string | undefined): Map<string, number> {
  const asked = new Map<string, number>();
  if (!machineId || !session) return asked;
  for (const [key, at] of fleetAsked) {
    const [machine, owner, task] = JSON.parse(key) as [string, string, string];
    if (machine === machineId && owner === session) asked.set(task, at);
  }
  return asked;
}
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
    if (action.request.op.kind === "request_merge") fleetAsked.set(fleetAskedKey(machineId, session, String(action.request.op.task_id)), Date.now());
    // Focus stays with the rows (the next row's ⋯ after a Stop); a destructive
    // result with no row left to go to lands on its result line (cas-97d58 F12).
    fleetFocusNext = action.inverse && fleetOps.undo ? "undo" : rowKey.startsWith("agent:") ? `${rowKey}:trigger` : undefined;
    window.clearTimeout(fleetUndoTimer);
    if (fleetOps.undo || fleetOps.result) fleetUndoTimer = window.setTimeout(() => { if (selectedMachineId && selectedSession) renderStatus(statuses.get(sessionKey(selectedMachineId, selectedSession))); }, UNDO_WINDOW_MS + 50);
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
    asked: fleetAskedFor(selectedMachineId, selectedSession),
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
  if (!phoneLayout()) { undo.remove(); return; }
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
  return JSON.stringify([fleetOps.menuFor, fleetOps.confirm?.action.id, fleetOps.confirm?.rowKey, fleetOps.preview?.rowKey, fleetOps.assignFor, [...fleetOps.pending].map(([key, action]) => [key, action.id]), [...fleetOps.notes], fleetOps.currentUndo(Date.now())?.label, fleetHeaderPanel, [...fleetAskedFor(selectedMachineId, selectedSession)].map(([id, at]) => [id, relativeTimestamp(at)])]);
}

function renderStatus(status?: Record<string, unknown>): void {
  const container = document.querySelector<HTMLElement>("#status-view");
  if (!container) return;
  const machine = selectedMachineId ? machines.get(selectedMachineId) : undefined;
  const session = selectedMachineId && selectedSession ? sessions.get(selectedMachineId)?.find(item => item.name === selectedSession) : undefined;
  const signature = JSON.stringify([phoneLayout(), progressSheetOpen(), selectedMachineId, selectedSession, session?.workers, status ?? null, machine?.scopes ?? null, selectedMachineId && selectedSession ? sessionSummaries.get(sessionKey(selectedMachineId, selectedSession)) ?? null : null, statusPending.size, fleetOpsSignature()]);
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
    const result = container.querySelector<HTMLElement>('[data-fleet-focus="result"]');
    // cas-d043 G06: focus moves on to the next row, and the result line
    // ("swift-lark-3 stopped.") is brought into view too, instead of sitting
    // above the fold whenever another row follows.
    // The focused row is scrolled to last, so it stays in sight when both can't be.
    if (neighbour) { neighbour.focus({ preventScroll: false }); result?.scrollIntoView({ block: "nearest" }); neighbour.scrollIntoView({ block: "nearest" }); return; }
    if (result) { result.focus({ preventScroll: false }); return; }
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
  const agents = ((status.agents as FleetAgent[]) ?? []);
  const tasks = [...((status.tasks_in_progress as FleetTask[]) ?? []), ...((status.tasks_ready as FleetTask[]) ?? [])];
  const workers = workerProgress(session?.workers, agents, tasks, session?.supervisor);
  const identifier = (value: unknown): HTMLSpanElement => {
    const span = document.createElement("span");
    span.className = "status-identifier";
    span.textContent = String(value);
    return span;
  };
  const chip = (value: unknown): HTMLSpanElement => {
    const span = document.createElement("span");
    span.className = `status-chip status-chip--${statusClass(value)}`;
    span.textContent = statusLabel(value);
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
    const phoneConversation = phoneLayout();
    // cas-2796a F03: an Undo whose floating offer was dismissed is still
    // offered inside the open Tasks & progress sheet until it expires.
    const offer = fleetOps.currentUndo(ops.now);
    const dismissedUndo = Boolean(offer && phoneConversation && fleetOps.phoneNoticeDismissed(offer));
    if (dismissedUndo && progressSheetOpen()) { const inSheet = undoBar(document, { ...ops, phone: false }); if (inSheet) container.append(inSheet); }
    const undo = offer && !dismissedUndo ? undoBar(document, { ...ops, phone: phoneConversation })
      : offer ? undefined
      : !phoneConversation && fleetOps.currentResult(ops.now) ? resultBar(document, ops)
        : phoneConversation ? phoneFleetNotice(document, ops) : undefined;
    if (undo && phoneConversation) { undo.id = "fleet-phone-undo"; document.body.append(undo); placeFleetUndo(); }
    else if (undo) container.append(undo);
    container.append(headerControls(document, ops, fleetHeaderPanel));
  }
  if (workers.length > 0) container.append(sectionLabel("Workers", workers.length));
  for (const worker of workers) {
    const row = document.createElement("article"); row.className = "status-row status-agent";
    const line = document.createElement("div"); line.className = "status-line";
    line.append(identifier(worker.name), chip(worker.agent?.status));
    row.append(line);
    if (ops && worker.agent) row.append(agentControls(document, ops, worker.agent));
    const work = document.createElement("p"); work.className = "status-activity";
    if (worker.currentTask) work.append(identifier(worker.currentTask), " · ");
    work.append(document.createTextNode(worker.work)); row.append(work);
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
  contextProgress = Boolean(summary) || workers.length > 0 || tasks.length > 0;
  if (!contextProgress) {
    const empty = document.createElement("p");
    empty.className = "status-empty";
    empty.textContent = "No agents or tasks reported for this session yet.";
    container.append(empty);
  }
  if (phoneLayout()) presentFleetSheet(container, dismissFleetPanel);
  restoreFocus();
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
  }
}

function bindEvents(): void {
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
  const interrupt = document.querySelector<HTMLButtonElement>("#conversation-interrupt");
  if (interrupt) interrupt.onclick = () => { void interruptSupervisor(); };
  const rawOutputToggle = document.querySelector<HTMLButtonElement>("#conversation-raw-output");
  if (rawOutputToggle) rawOutputToggle.onclick = openRawOutput;
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
      else landFocus([focusTargets.thread, focusTargets.conversationBack], { keep: true, nextTask: true, waitMs: 2_000 });
    };
  }

  const paletteToggle = document.querySelector<HTMLButtonElement>("#command-palette-toggle");
  if (paletteToggle) paletteToggle.onclick = openCommandPalette;
  const palette = document.querySelector<HTMLDialogElement>("#command-palette")!;
  const closePalette = () => {
    commandPaletteOpen = false;
    palette.close();
    paletteToggle?.focus();
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
        landFocus([focusTargets.thread, focusTargets.conversationBack], { keep: true, nextTask: true, waitMs: 2_000 });
        return;
      }
      focusJumpedComposer(opened);
    };
  }
  markAppearanceCommands(palette);
  for (const command of palette.querySelectorAll<HTMLButtonElement>("[data-palette-scheme]")) {
    command.onclick = () => { setScheme(command.dataset.paletteScheme as SchemePreference); markAppearanceCommands(palette); closePalette(); };
  }
  const paletteDormant = palette.querySelector<HTMLButtonElement>("[data-palette-action='dormant']");
  if (paletteDormant) paletteDormant.onclick = () => { closePalette(); setDormantRevealed(!revealDormant); };
  const paletteLaunch = palette.querySelector<HTMLButtonElement>("[data-palette-action='new-session']");
  if (paletteLaunch) paletteLaunch.onclick = () => { closePalette(); launchSheet.open(); };
  const newSession = document.querySelector<HTMLButtonElement>("#new-session-toggle");
  if (newSession) newSession.onclick = () => launchSheet.open();
  const paletteDismiss = palette.querySelector<HTMLButtonElement>("[data-palette-action='dismiss-info']");
  if (paletteDismiss) paletteDismiss.onclick = () => { closePalette(); void acknowledgeAttentionGroup(dismissableInfoItems(attention)); };
  if (document.querySelector<HTMLButtonElement>("#pair-toggle")) document.querySelector<HTMLButtonElement>("#pair-toggle")!.onclick = () => (document.querySelector<HTMLDialogElement>("#pair-dialog")!).showModal();
  for (const button of document.querySelectorAll<HTMLButtonElement>("#inbox-toggle, #empty-inbox")) button.onclick = () => void inboxView.open();
  const skip = document.querySelector<HTMLButtonElement>("#skip-to-conversation");
  if (skip) skip.onclick = skipToConversation;
  for (const pair of document.querySelectorAll<HTMLButtonElement>("#empty-pair")) {
    pair.onclick = () => document.querySelector<HTMLDialogElement>("#pair-dialog")!.showModal();
  }
  const pairForm = document.querySelector<HTMLFormElement>("#pair-form");
  const pairCancel = document.querySelector<HTMLButtonElement>("#pair-cancel");
  const pairDevice = pairForm?.querySelector<HTMLInputElement>('input[name="device"]');
  if (pairDevice) pairDevice.addEventListener("input", () => pairDevice.setCustomValidity(""));
  const pairClose = document.querySelector<HTMLButtonElement>("#pair-close");
  const pairCreate = document.querySelector<HTMLButtonElement>("#pair-create");
  const pairDialog = document.querySelector<HTMLDialogElement>("#pair-dialog");
  if (pairDialog) bindPairBarFrost(pairDialog);
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
// A lifted finger's click comes in a later task, after its focus change.
app.addEventListener("pointerup", (event) => { if (event.pointerType === "touch") deferredRender.touchEnded(); else deferredRender.gestureEnded(); }, true);
app.addEventListener("click", () => deferredRender.clicked(), true);
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
    // rebuild runs when focus leaves the palette.
    if (active instanceof HTMLElement && active.closest("#command-palette[open]")) return;
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
// Rotation changes the layout in CSS instantly, but the phone's sheets (Tasks
// & progress, Attention) are decided in JS at render time. Without this, a
// phone turned on its side kept the composition it was mounted with until
// some hub event happened to redraw it.
window.matchMedia(PHONE_MEDIA_QUERY).addEventListener("change", () => render());
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
      ...(state?.networkAccessHelp ? { cause: "This browser is blocking the connection. Allow Local network access for this site in the browser's settings." }
        : state?.phase === "failed" && state.fatal === true ? { cause: `This browser can't connect. ${fatalConnectionRecovery(state.reason)}` } : {}),
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
    // cas-d043 G03: before any machine has answered with its sessions (a
    // reload during an outage), the count is unknown, not "0 conversations".
    const counted = machines.size === 0 || [...machines.keys()].some((id) => fleetCatalogUpdatedAt.has(id));
    const markup = machineFooterMarkup(rows, counted ? [...machines.keys()].reduce((sum, id) => sum + visibleSessions(id).filter(session => supervisorTarget(session)).length, 0) : undefined, __HUB_BUILD__, !machineCatalogLoaded);
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
    installations: (id) => {
      const machine = machines.get(id);
      const connection = connections.get(id);
      if (machine && connection) void openInstallationInventory(document, machine, connection, async () => {
        await installationAccess.forgetRevoked(machine.id, machine.baseUrl, machine.deviceId);
        await forgetPairedMachine(id);
        // cas-d043 G09: say what happened, once the inventory has closed, and
        // land on the next step instead of a silent first-run screen.
        window.setTimeout(() => {
          toast(`This browser's access to ${machine.label} was revoked.`);
          const visible = (node: HTMLElement | null): node is HTMLElement => node !== null && node.getClientRects().length > 0;
          // Paired machines may still be open over the page: its own Pair a machine, then.
          const open = document.querySelector<HTMLDialogElement>("dialog[open]");
          const next = open
            ? [open.querySelector<HTMLElement>("#paired-machines-add")].find(visible)
            : [document.getElementById("empty-pair"), document.getElementById("pair-toggle"), document.getElementById("paired-machines-toggle")].find(visible);
          next?.focus();
        }, 0);
      });
    },
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
  try {
    const machine = machines.get(id);
    if (machine) await sendJournal.purge(id, credentialFence(machine));
    await catalog.remove(id);
  }
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

/**
 * cas-9b7d: retained inbox turns of a paired machine join its conversation
 * threads, so a machine that is off still shows the supervisor's latest
 * words. A turn carries the machine's own durable notification id, so a copy
 * that also arrives directly is the same bubble, not a second one.
 */
function hydrateInboxThreads(events: Parameters<typeof inboxThreads>[0]): void {
  let changed = false;
  for (const machine of machines.values()) {
    for (const [session, turns] of inboxThreads(events, machine.id)) {
      const history = conversationHistory(sessionKey(machine.id, session), session);
      for (const turn of turns) {
        if (turn.kind === "reply") {
          history.hydrateReply({
            notification_id: turn.notificationId,
            reply_to: turn.replyTo,
            message: turn.message,
            summary: turn.summary,
            device_id: turn.deviceId,
            ...(turn.turnKind ? { kind: turn.turnKind as OperatorReply["kind"] } : {}),
            attachments: turn.attachments as OperatorReply["attachments"],
            session,
            at: turn.at,
          });
          changed = true;
        } else if (turn.kind === "message") {
          history.hydrateSend({
            notification_id: turn.notificationId,
            target: "supervisor",
            text: turn.text,
            state: "acknowledged",
            stamped: true,
            device_id: turn.deviceId,
            ...(turn.operatorLabel ? { operator_label: turn.operatorLabel } : {}),
            session,
            at: turn.at,
          });
          changed = true;
        }
      }
    }
  }
  if (changed) { updateConversationViews(); renderConversationList(); }
}

/**
 * cas-4634: once this profile is signed in to the inbox, ask each paired hub
 * that is a machine of the same account to verify this installation into it
 * (cloud contract §5.5). Once per machine per page; a hub outside the
 * account or an older hub without the route is left as it is.
 */
const installationEnrollmentTried = new Set<string>();
function enrollPairedInstallations(): void {
  for (const machine of machines.values()) {
    const connection = connections.get(machine.id);
    if (!connection || installationEnrollmentTried.has(machine.id) || connection.snapshot().phase !== "live") continue;
    if (machine.accountEnrollment?.state === "enrolled") continue;
    installationEnrollmentTried.add(machine.id);
    void enrollPairedInstallation(connection, operatorInbox.client, machine.publicKey)
      .then((outcome) => {
        if (outcome.kind === "enrolled") {
          machine.accountEnrollment = outcome.enrollment;
          void catalog.put(machine).catch(() => undefined);
        }
      })
      .catch(() => { /* retried on the next page load */ });
  }
}

operatorInbox.subscribe((snapshot) => {
  syncInboxLoop(snapshot.state.kind === "ready");
  hydrateInboxThreads(snapshot.events);
  if (snapshot.state.kind === "ready") enrollPairedInstallations();
});
void operatorInbox.load().catch(() => { /* the inbox dialog reports its own state */ });
