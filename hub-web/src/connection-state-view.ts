import { CAUSE_COPY } from "./connection-diagnostics";
import {
  attachElapsedSeconds,
  elapsedSeconds as stageElapsedSeconds,
  type AttachSnapshot,
  type ConnectionSnapshot,
} from "./connection-state";

/**
 * Structural consumer view of HubConnectionSupervisor's connection snapshot.
 * Keeping this interface at the view seam lets the lifecycle own all state and
 * timing while the UI owns only presentation thresholds.
 */
export type ConnectionSnapshotView = ConnectionSnapshot | AttachSnapshot;

export interface ConnectingView {
  readonly elapsedSeconds: number;
  readonly elapsedLabel: string;
  readonly step?: string;
  readonly actionsAvailable: boolean;
}

export interface DisconnectedView {
  readonly elapsedSeconds: number;
  readonly attempt: number;
  readonly retryLabel: string;
}

export interface ConnectionTimelineEntry {
  readonly label: string;
  readonly detail: string;
  readonly tone: "current" | "retry" | "failed" | "evidence";
}

export interface ConnectionSurfaceActions {
  readonly retry?: () => void;
  readonly diagnose?: () => void;
  readonly repair?: () => void;
}

export interface ConnectionSurfaceOptions {
  /** Title while the attach is in progress; defaults to "Connecting to <session>…". */
  readonly openingTitle?: string;
  /**
   * cas-28df: this snapshot is a never-live conversation's first retry. It
   * still reads as opening: the calm title, and the retry behind "Details".
   */
  readonly quietRetry?: boolean;
  /**
   * cas-813a: a conversation opens behind the one quiet line (openingLine),
   * not the verdict card; the card is kept for a failure.
   */
  readonly quietOpening?: boolean;
}

const STAGE_COPY: Record<ConnectionSnapshot["stage"], string> = {
  idle: "waiting to start the connection",
  resolving: "resolving the target node",
  dialing: "checking the machine's HTTP hub",
  auth: "checking session authorization",
  attaching: "opening the machine's event stream or terminal socket",
  live: "waiting for the terminal heartbeat",
};

/**
 * The lifecycle keeps the current attempt and its best diagnostic, rather than
 * a second client-side history. The presentation can still show an honest
 * annotated timeline: a count of earlier attempts, the current stage, and the
 * latest outcome without inventing timestamps or causes.
 */
export function connectionTimeline(snapshot: ConnectionSnapshotView): ConnectionTimelineEntry[] {
  const attempt = Math.max(1, snapshot.attempt);
  const entries: ConnectionTimelineEntry[] = [];
  if (attempt > 1) {
    entries.push({
      label: "Earlier attempts",
      detail: `${attempt - 1} attempt${attempt === 2 ? "" : "s"} did not reach a live session`,
      tone: "evidence",
    });
  }

  const failed = snapshot.fatal === true || snapshot.phase === "failed";
  const retrying = snapshot.phase === "backoff";
  entries.push({
    label: `Attempt ${attempt}`,
    detail: failed ? "Connection failed" : retrying ? "Retry scheduled" : STAGE_COPY[snapshot.stage],
    tone: failed ? "failed" : retrying ? "retry" : "current",
  });

  if (snapshot.reason) {
    entries.push({
      label: failed ? "Outcome" : "Diagnostic",
      detail: snapshot.reason,
      tone: failed ? "failed" : "evidence",
    });
  }
  if (snapshot.cause) {
    const cause = snapshot.cause;
    entries.push({ label: "Connection cause", detail: `${CAUSE_COPY[cause.code].title} · ${cause.layer}${cause.status === undefined ? "" : ` · HTTP ${cause.status}`}${cause.closeCode === undefined ? "" : ` · socket ${cause.closeCode}`}`, tone: "evidence" });
    entries.push({ label: "Recovery action", detail: CAUSE_COPY[cause.code].action, tone: "evidence" });
  }
  entries.push({ label: "Last successful connection", detail: snapshot.lastSuccessAt ? new Date(snapshot.lastSuccessAt).toLocaleTimeString() : "Not measured in this visit", tone: "evidence" });
  if (retrying && snapshot.retryInMs !== undefined) {
    const seconds = Math.max(0, Math.ceil((snapshot.nextRetryAt === undefined ? snapshot.retryInMs : snapshot.nextRetryAt - Date.now()) / 1_000));
    entries.push({
      label: "Next attempt",
      detail: `reconnecting in ${seconds}s`,
      tone: "retry",
    });
  }
  return entries;
}

/**
 * An attach still on its way to live: no failure, no scheduled retry. Its
 * default surface is one calm title (journey F3); the attempt and relay stage
 * wait behind "Details", and only after the quiet window below.
 */
export function attachInProgress(snapshot: ConnectionSnapshotView): boolean {
  return snapshot.fatal !== true && snapshot.phase !== "failed" && snapshot.phase !== "backoff" && snapshot.phase !== "live";
}

/** A conversation's attach title: calm, and free of relay vocabulary (journey F3). */
export const CONVERSATION_OPENING = "Opening the conversation…";

/** Quiet motion starts this long after a conversation begins to open (cas-813a). */
export const OPENING_MOTION_DELAY_MS = 1_000;

/**
 * The one way a conversation shows that it is opening (cas-813a): a body-size
 * line, centred where the thread will be, saying "Opening the conversation…".
 * Its dots stay still until `delayMs` has passed, so a quick open shows no
 * motion. The same line serves the attach and the first history page, so the
 * operator sees one wait, not two or three. `delayMs` is counted from when
 * the open began, so a line built later does not restart the clock.
 */
export function openingLine(document: Document, title: string, delayMs: number): HTMLElement {
  const line = document.createElement("p");
  line.className = "conversation-loading";
  line.setAttribute("role", "status");
  line.dataset.title = title;
  const dots = document.createElement("span");
  dots.className = "dots";
  dots.setAttribute("aria-hidden", "true");
  const delay = Math.max(0, Math.round(delayMs));
  for (const step of [0, 200, 400]) {
    const dot = document.createElement("i");
    dot.style.animationDelay = `${delay + step}ms`;
    dots.append(dot);
  }
  const text = document.createElement("span");
  text.textContent = title;
  line.append(dots, text);
  return line;
}

/**
 * Put the opening line in `target`, keeping the one already there: rebuilding
 * it on every repaint would restart its motion.
 */
export function showOpeningInto(target: HTMLElement, title: string, delayMs: number): HTMLElement {
  const existing = target.querySelector<HTMLElement>(":scope > .conversation-loading");
  const line = existing?.dataset.title === title ? existing : openingLine(target.ownerDocument, title, delayMs);
  target.className = "empty conversation-opening";
  for (const child of [...target.childNodes]) if (child !== line) child.remove();
  if (line.parentElement !== target) target.append(line);
  return line;
}

/** How long an attach shows its title alone before "Details" is offered. */
export const ATTACH_QUIET_MS = 400;

/** Milliseconds since this not-yet-live lifecycle began, on the same clocks as elapsedSeconds. */
export function elapsedMs(snapshot: ConnectionSnapshotView, now = Date.now()): number {
  const anchor = "session" in snapshot ? snapshot.attachSince ?? snapshot.since : snapshot.connectingSince ?? snapshot.since;
  return Math.max(0, now - anchor);
}

export function elapsedSeconds(snapshot: ConnectionSnapshotView, now = Date.now()): number {
  return "session" in snapshot
    ? attachElapsedSeconds(snapshot, now)
    : stageElapsedSeconds(snapshot, now);
}

export function formatElapsed(seconds: number): string {
  const minutes = Math.floor(seconds / 60);
  const remainder = seconds % 60;
  return minutes > 0 ? `${minutes}m ${String(remainder).padStart(2, "0")}s` : `${remainder}s`;
}

export function connectingView(snapshot: ConnectionSnapshotView, now = Date.now()): ConnectingView {
  const elapsed = elapsedSeconds(snapshot, now);
  // A failure retrying cannot fix has nothing to disclose progressively: say
  // what broke and offer the escape hatch on the first frame.
  const fatal = snapshot.fatal === true;
  return {
    elapsedSeconds: elapsed,
    elapsedLabel: formatElapsed(elapsed),
    step: fatal || elapsed >= 5 ? snapshot.reason ?? STAGE_COPY[snapshot.stage] : undefined,
    actionsAvailable: fatal || elapsed >= 15,
  };
}

export function disconnectedView(snapshot: ConnectionSnapshotView, now = Date.now()): DisconnectedView {
  const elapsed = elapsedSeconds(snapshot, now);
  const retrySeconds = snapshot.retryInMs === undefined ? undefined : Math.max(0, Math.ceil(snapshot.retryInMs / 1_000));
  return {
    elapsedSeconds: elapsed,
    attempt: Math.max(1, snapshot.attempt),
    // A failure retrying cannot fix has no next attempt to promise.
    retryLabel: snapshot.fatal === true
      ? "not retrying"
      : retrySeconds === undefined ? "reconnecting" : `reconnecting in ${retrySeconds}s`,
  };
}

/**
 * Render the connection verdict and attempt timeline shared by the live app
 * and the visual-QA fixture. Lifecycle retention and selection stay in the
 * caller; this seam owns only the presentation of a not-yet-live snapshot.
 */
export function renderConnectionSurfaceInto(
  target: HTMLElement,
  session: string,
  snapshot: ConnectionSnapshotView,
  actions: ConnectionSurfaceActions = {},
  now = Date.now(),
  options: ConnectionSurfaceOptions = {},
): void {
  const document = target.ownerDocument;
  const view = connectingView(snapshot, now);
  const fatal = snapshot.fatal === true;
  const opening = attachInProgress(snapshot) || (options.quietRetry === true && !fatal);
  const quiet = options.quietOpening === true && opening;
  // A repaint (the 1 Hz ticker, a hub push) keeps "Details" as the operator left it.
  const detailsOpen = target.querySelector<HTMLDetailsElement>(":scope > .connection-details")?.open === true;
  // cas-28df: the 1 Hz repaint rebuilds this card, so keyboard focus on
  // "Details" or an action was dropped to the page body every second. Note
  // which control held it and hand it to that control's replacement.
  const focused = document.activeElement instanceof HTMLElement && target.contains(document.activeElement) ? focusKey(document.activeElement) : undefined;
  if (quiet) {
    showOpeningInto(target, options.openingTitle ?? CONVERSATION_OPENING, OPENING_MOTION_DELAY_MS - elapsedMs(snapshot, now));
  } else {
    target.className = `empty terminal-state terminal-connecting${fatal ? " terminal-connect-failed" : ""}`;
    const title = document.createElement("p");
    title.className = "terminal-connecting-title";
    // A spinner and a rising counter over a failure that will never resolve is
    // the D3 overlay: it reads as progress. State the outcome instead.
    title.textContent = fatal
      ? "Connection failed — not retrying."
      : opening
        ? options.openingTitle ?? `Connecting to ${session}…`
      : snapshot.phase === "failed"
        ? snapshot.authFailure ? "Connection failed — re-pair required." : "Connection failed — retry available."
      : snapshot.phase === "backoff"
        ? "Connection interrupted — retrying."
        : options.openingTitle ?? `Connecting to ${session}…`;
    target.replaceChildren(title);
  }
  // An attach in progress shows its title alone for the quiet window: against
  // a quick relay nothing more ever appears (journey F3).
  // A quiet conversation open waits until its motion starts (cas-813a).
  if (opening && elapsedMs(snapshot, now) < (quiet ? OPENING_MOTION_DELAY_MS : ATTACH_QUIET_MS) && !view.actionsAvailable) return;

  const timeline = document.createElement("ol");
  timeline.className = "connection-timeline";
  timeline.setAttribute("aria-label", "Connection attempts");
  for (const entry of connectionTimeline(snapshot)) {
    const item = document.createElement("li");
    item.className = `connection-timeline-item ${entry.tone}`;
    const marker = document.createElement("span");
    marker.className = "connection-timeline-marker";
    marker.setAttribute("aria-hidden", "true");
    const content = document.createElement("div");
    const label = document.createElement("span");
    label.className = "connection-timeline-label";
    label.textContent = entry.label;
    const detail = document.createElement("span");
    detail.className = "connection-timeline-detail";
    detail.textContent = entry.detail;
    content.append(label, detail);
    item.append(marker, content);
    timeline.append(item);
  }
  if (opening) {
    // The attempt and relay stage are evidence for whoever asks, not the
    // default surface: they sit behind "Details".
    const details = document.createElement("details");
    details.className = "connection-details";
    details.open = detailsOpen;
    const summary = document.createElement("summary");
    summary.textContent = "Details";
    details.append(summary, timeline);
    target.append(details);
  } else {
    target.append(timeline);
  }

  if (view.actionsAvailable) {
    const actionRow = document.createElement("div");
    actionRow.className = "terminal-connecting-actions";
    const addAction = (label: string, action: (() => void) | undefined): void => {
      if (!action) return;
      const button = document.createElement("button");
      button.type = "button";
      button.textContent = label;
      button.onclick = action;
      actionRow.append(button);
    };
    addAction("Retry", actions.retry);
    addAction("Diagnose", actions.diagnose);
    if (snapshot.authFailure === "revoked" || snapshot.authFailure === "scope-mismatch" || snapshot.authFailure === "needs-pairing") {
      addAction("Re-pair", actions.repair);
    }
    if (actionRow.childElementCount > 0) target.append(actionRow);
  }
  if (focused) [...target.querySelectorAll<HTMLElement>("summary, button")].find((element) => focusKey(element) === focused)?.focus();
}

function focusKey(element: HTMLElement): string | undefined {
  if (element.tagName === "SUMMARY") return "summary";
  if (element.tagName === "BUTTON") return `button:${element.textContent ?? ""}`;
  return undefined;
}

export function shouldRetainDisconnectedFrame(snapshot: ConnectionSnapshotView): boolean {
  return snapshot.degraded || snapshot.phase !== "live";
}

/**
 * Whether a terminal socket failure earns its own attention card. A failure
 * that retries is already told in plain words by the banner, the header, the
 * row and the footer ("Reconnecting…"); a rail card beside them repeated it in
 * transport terms with counts that disagreed (cas-90d4). A pairing loss has its
 * own machine card. Only a failure that will not retry needs the rail.
 */
export function transportFailureNeedsAttention(attach: ConnectionSnapshotView | undefined, machine?: ConnectionSnapshotView): boolean {
  return attach?.fatal === true && !attach.authFailure && (!machine || machine.phase === "live");
}

/**
 * One outage, one vocabulary (journey F9): the reconnect banner, a send the
 * outage refused and the controls it disabled all name the machine the way
 * the banner does, so the operator never reads two descriptions of one drop.
 */
/** Fatal transport failures are unsupported-browser errors, never network guesses. */
export function fatalConnectionRecovery(reason?: string): string {
  if (!reason) return "This browser cannot make this connection. Update your browser, then reload this page.";
  // Keep the supported browser versions in the actual reason, but explain the
  // missing API in human terms; the full diagnostic remains in Details.
  return `${reason.replace(/^This browser is missing .+, which Cassy Cloud needs\./, "This browser is missing a feature Cassy Cloud needs.")} Then reload this page.`;
}

export function lostConnectionBanner(machineLabel: string, fatal: boolean, reason?: string): string {
  // cas-97d58 F16: a fatal failure is this browser's (an unsupported API),
  // not the machine's, so it says so first.
  return fatal ? `This browser can't connect to ${machineLabel}. ${fatalConnectionRecovery(reason)}` : `Lost connection to ${machineLabel}. Reconnecting…`;
}

/**
 * cas-a6f0 (journey F8/F9): the machine still reads live but its heartbeats go
 * unanswered. It is not lost yet, so it is not "Lost connection"; the banner,
 * the composer and the Tasks panel say what the header's "Unsteady" means.
 */
export function unsteadyBanner(machineLabel: string): string {
  return `Connection to ${machineLabel} unsteady — checking…`;
}

/**
 * cas-a6f0 (journey F35): a send held for a machine whose pairing the hub then
 * refused. It never left this browser and will not go by itself.
 */
export function pairingRefusal(machineLabel: string): string {
  return `Not sent: ${machineLabel} needs pairing again.`;
}

/**
 * cas-d15c: a pairing the hub refused (revoked, unknown key) is not
 * reconnecting, so the banner beside "Needs pairing" must not say it is.
 */
export function pairingLostBanner(machineLabel: string): string {
  return `${machineLabel} needs pairing again.`;
}

/**
 * cas-d15c: one session's daemon link dropped while its machine stayed
 * connected (the hub closed that session's stream after upstream_unavailable).
 * The banner names the conversation, and says the machine is fine, instead of
 * "Lost connection to <machine>", which is kept for a real machine drop.
 */
export function sessionReconnectingBanner(sessionLabel: string, machineLabel: string, fatal: boolean): string {
  return fatal
    ? `Lost the link to ${sessionLabel}. Not retrying. ${machineLabel} is still connected.`
    : `Reconnecting to ${sessionLabel}… ${machineLabel} is still connected.`;
}

/** What a session-only drop means for Interrupt and Raw output (cas-0546), without restating the drop. */
export const SESSION_OUTAGE_CONTROLS_RETURN = "Interrupt and raw output return when it's back.";

/** Why the conversation's actions wait while only its own link reconnects (cas-d15c). */
export function sessionOutageControlsReason(sessionLabel: string): string {
  return `Reconnecting to ${sessionLabel}. ${SESSION_OUTAGE_CONTROLS_RETURN}`;
}

/** A message the outage refused. The composer keeps the draft, so it says so. */
export function outageRefusal(machineLabel: string): string {
  return `Not sent: lost connection to ${machineLabel}. Your message is kept; send it again when it's back.`;
}

/**
 * cas-7b31 (journey F2): why the conversation's actions wait while the
 * pairing is refused. Nothing reconnects or comes back by itself; re-pairing
 * is the step.
 */
export const PAIRING_CONTROLS_RETURN = "Re-pair it to interrupt the supervisor or read its raw output.";

export function pairingControlsReason(machineLabel: string): string {
  return `${pairingLostBanner(machineLabel)} ${PAIRING_CONTROLS_RETURN}`;
}

/** What a machine outage means for the conversation's actions, without restating the outage. */
export const OUTAGE_CONTROLS_RETURN = "Interrupt and raw output return when it reconnects.";

/** Why Interrupt and Raw output are unavailable during an outage (cas-0546). */
export function outageControlsReason(machineLabel: string): string {
  return `Lost connection to ${machineLabel}. ${OUTAGE_CONTROLS_RETURN}`;
}
