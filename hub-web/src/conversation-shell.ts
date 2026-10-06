import { cloudBrand, escapeHtml, projectTitle } from "./cloud-brand";
import { machineAccentClass, machineMonogram } from "./machine-accent";

export interface ConversationShellModel {
  supervisor?: string;
  projectDir?: string;
  host?: string;
  /** Selected machine; its accent class goes on the shell root so the thread inherits it. */
  machineId?: string;
  selected: boolean;
  loaded: boolean;
  paired: boolean;
  /** The list search's text, kept across shell rebuilds. */
  searchQuery?: string;
  /** Offer the Ctrl K hint in the search placeholder (not on a phone). */
  keyboardHint?: boolean;
  /**
   * New session (cas-0f51): "ready" when a paired machine lets this browser
   * start sessions, "grant" when none does yet (the button becomes the way to
   * allow it); absent with no machine paired.
   */
  launch?: "ready" | "grant";
}

/**
 * The list header's New session control (cas-865c). It is named by its goal
 * in both states: where no paired machine lets this browser start sessions
 * yet, the same control opens the sheet's grant view, which explains the
 * permission, instead of reading like a toggle ("Allow new sessions").
 */
export function newSessionButtonMarkup(launch: ConversationShellModel["launch"]): string {
  if (!launch) return "";
  return `<button id="new-session-toggle" class="new-session-toggle" type="button" aria-haspopup="dialog"${launch === "grant" ? ' data-launch-grant="true"' : ""}><span aria-hidden="true">+</span> New session</button>`;
}

export const CONVERSATION_SEARCH_LABEL = "Search conversations";
export const CONVERSATION_SEARCH_PLACEHOLDER = "Search conversations (Ctrl K)";
/** Without a keyboard shortcut to offer (a touch device, or narrower than the
 * palette's breakpoint), the placeholder names the search alone (journey F10). */
export const CONVERSATION_SEARCH_PLACEHOLDER_TOUCH = "Search conversations";
/** Where the Ctrl K hint is worth printing: a fine pointer (so, in practice, a
 * keyboard) and wide enough that the palette it leads to is on screen. */
export const KEYBOARD_HINT_MEDIA_QUERY = "(any-pointer: fine) and (min-width: 500px)";

type PlatformNavigator = { readonly platform?: string; readonly userAgentData?: { readonly platform?: string } };

/** Apple keyboards print ⌘ where every other keyboard prints Ctrl. */
export function applePlatform(nav: PlatformNavigator | undefined = typeof navigator === "undefined" ? undefined : navigator as PlatformNavigator): boolean {
  return /mac|iphone|ipad|ipod/i.test(nav?.userAgentData?.platform || nav?.platform || "");
}

/** The palette shortcut as this keyboard prints it: every surface that names
 * it (the list search, the Appearance tooltip) reads the same one, so a Linux
 * browser never shows ⌘K beside Ctrl K (journey F16). Both chords work
 * everywhere. */
export function paletteShortcutLabel(apple = applePlatform()): string {
  return apple ? "⌘K" : "Ctrl K";
}

export function conversationSearchPlaceholder(keyboardHint: boolean, shortcut = paletteShortcutLabel()): string {
  if (!keyboardHint) return CONVERSATION_SEARCH_PLACEHOLDER_TOUCH;
  return shortcut === "Ctrl K" ? CONVERSATION_SEARCH_PLACEHOLDER : `${CONVERSATION_SEARCH_LABEL} (${shortcut})`;
}

/** The list's visible name search (journey F8): filters rows by project,
 * machine or supervisor. Ctrl/Cmd+K focuses it; pressed again from the field,
 * it opens the command palette. */
export function conversationSearchMarkup(query = "", keyboardHint = true): string {
  return `<div class="conversation-search" role="search"><svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true" focusable="false"><circle cx="8.5" cy="8.5" r="5.5"/><path d="M13 13l4 4"/></svg><input id="conversation-search" type="search" aria-label="${CONVERSATION_SEARCH_LABEL}" aria-controls="conversation-list" aria-keyshortcuts="Control+K Meta+K" placeholder="${conversationSearchPlaceholder(keyboardHint)}" autocomplete="off" spellcheck="false" enterkeyhint="go" value="${escapeHtml(query)}"></div>`;
}

/** The list's empty line while a search hides every row. */
export function conversationNoMatchText(query: string): string {
  return `No conversation matches “${query.trim()}”. Search looks at project, machine and supervisor names.`;
}

/** Compose is a YOU action: the button takes the operator's constant colour
 * (--you-bg at :root), never a machine accent, so it stays visible whatever
 * machine scope the shell root carries. On a phone it is a labelled bar in the
 * list's own flow, above the status footer, not a floating icon (journey F15). */
export const composeFabMarkup = '<button id="compose-fab" class="compose-fab" type="button" aria-label="Write to a supervisor"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true" focusable="false"><path d="M12 5v14M5 12h14"/></svg><span class="compose-fab-label">Write to a supervisor</span></button>';

/**
 * The one header above the thread (cas-5b2d): the Pebble thead — machine
 * monogram in the accent, the project bold as the title (once: journey F7),
 * machine · supervisor codename · connection in mono beneath (same order as a
 * list row) — with the cas-11b01 back link and the conversation's actions
 * (Raw output, Interrupt: cas-0546) on the row above it. Elevated with
 * --lift-head; the thread view itself renders no header inside this shell.
 *
 * On phone the two rows fold into one (cas-1776): the back link shows only its
 * "‹" glyph and Raw output only its icon; each button's aria-label keeps its
 * accessible name ("‹ Conversations", "Raw output") the same at every width,
 * and Interrupt keeps its word. The project ellipsises (its title carries it
 * whole) and the host line ellipsises its machine · codename part while the
 * connection state after it stays visible. The machine name has priority over
 * the generated codename (cas-766c): its OS word goes first, then the codename
 * ellipsises and, with no room left, steps aside (fitMachineLine).
 */
/** A machine label's trailing operating-system word, as in "Studio Mac · macOS". */
const HOST_OS = /^(.*\S)(\s·\s(?:macOS|Linux|Windows|FreeBSD|OpenBSD|NetBSD|ChromeOS|iPadOS|iOS|Android|Ubuntu|Debian|Fedora|WSL))$/i;

/**
 * The machine label with its OS word in its own span (journey F14): on a
 * phone the header drops " · macOS" before it truncates the codename.
 */
export function hostMarkup(host: string): string {
  const match = HOST_OS.exec(host);
  return match ? `${escapeHtml(match[1])}<span class="host-os">${escapeHtml(match[2])}</span>` : escapeHtml(host);
}

/**
 * The phone's way to this session's Attention (cas-5c22). The rail that lists
 * attention beside the thread on a desktop is not shown on a phone, so a live
 * delivery problem had no sign there. Hidden until the session has an open
 * item; main.ts fills the count and opens the rail as a sheet.
 */
const CONVERSATION_ATTENTION_BUTTON = `<button id="conversation-attention" class="conversation-attention" type="button" aria-haspopup="dialog" hidden><svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"><path d="M10 3.2 18 17H2z"/><path d="M10 8.5v3.6"/><path d="M10 14.6v.1"/></svg><span class="conversation-attention-count"></span></button>`;

/** Badge text and accessible name for the session's open attention items. */
export function conversationAttentionBadge(count: number): { hidden: boolean; text: string; label: string } {
  return { hidden: count < 1, text: String(count), label: `Attention: ${count} item${count === 1 ? "" : "s"} for this session` };
}

const RAW_OUTPUT_ICON = '<svg class="action-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"><rect x="3" y="4" width="18" height="16" rx="2"/><path d="M7 9l3 3-3 3M12 15h5"/></svg>';
const INTERRUPT_ICON = '<svg class="action-icon" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true" focusable="false"><rect x="6" y="6" width="12" height="12" rx="2"/></svg>';

/** "Interrupt the cas-src supervisor": the project's supervisor, never the codename (journey F20). */
export function interruptLabel(project: string | undefined): string {
  return project ? `Interrupt the ${project} supervisor` : "Interrupt the supervisor";
}

/**
 * The conversation's own actions (cas-0546): Raw output opens a read-only
 * drawer over the supervisor's terminal text, Interrupt stops what the
 * supervisor is doing. Each names its reason when it is unavailable, in a
 * description beside it (applyActionAvailability), instead of disappearing.
 */
export function conversationActionsMarkup(project: string | undefined): string {
  return `<span class="conversation-header-actions"><button id="conversation-raw-output" class="conversation-action" type="button" aria-haspopup="dialog" aria-expanded="false" aria-label="Raw output">${RAW_OUTPUT_ICON}<span class="action-label">Raw output</span></button><button id="conversation-interrupt" class="conversation-action conversation-interrupt" type="button" aria-label="${escapeHtml(interruptLabel(project))}">${INTERRUPT_ICON}<span class="action-label">Interrupt</span></button><span id="conversation-raw-output-reason" hidden></span><span id="conversation-interrupt-reason" hidden></span></span>`;
}

export function conversationHeaderMarkup(model: ConversationShellModel): string {
  const host = model.host || "";
  // No project named: the codename is the title and the host line names only the machine (cas-1ca1 F03).
  const project = projectTitle(model.projectDir);
  const supervisor = model.supervisor || "Supervisor unavailable";
  const title = project ?? supervisor;
  return `<header class="conversation-heading thead"><div class="conversation-topline"><button id="conversation-back" type="button" aria-label="‹ Conversations"><span class="back-glyph">‹</span><span class="back-label"> Conversations</span></button>${cloudBrand()}${CONVERSATION_ATTENTION_BUTTON}<button id="conversation-fleet" class="conversation-fleet" type="button" aria-haspopup="dialog" aria-expanded="false" aria-label="Tasks &amp; progress">⋯</button>${conversationActionsMarkup(project)}</div><div class="conversation-identity"><span class="conversation-avatar" aria-hidden="true">${escapeHtml(machineMonogram(host || model.supervisor || "?"))}</span><div class="id"><h1><b title="${escapeHtml(title)}"${project ? "" : ' class="codename"'}>${escapeHtml(title)}</b></h1><span class="conversation-host"><span class="host-where" title="${escapeHtml(project ? [host, supervisor].filter(Boolean).join(" · ") : host)}">${project ? `${host ? `<span class="host-machine">${hostMarkup(host)}</span><span class="host-sep"> · </span>` : ""}<span class="codename">${escapeHtml(supervisor)}</span>` : `<span class="host-machine">${hostMarkup(host)}</span>`}</span><span id="conversation-connection" role="status"></span></span></div></div></header>`;
}

/** Attaching files from this browser has no transport yet; the clip stays out of the composer until it does. */
export const ATTACH_SUPPORTED = false;
/** The clip's reason, for when it comes back disabled-with-a-reason or enabled. */
export const ATTACH_DISABLED_REASON = "Attaching files from this device is not supported yet. Supervisors send reports to you; sending files to a supervisor is coming.";
export const composerClipMarkup = `<button type="button" class="composer-clip" disabled aria-disabled="true" title="${ATTACH_DISABLED_REASON}" aria-label="Attach a file (not yet supported)"><svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"><path d="M14.8 9.1l-5 5a3.2 3.2 0 01-4.6-4.6l6-6a2.1 2.1 0 013 3l-6 6a1 1 0 01-1.4-1.4l5.3-5.3"/></svg></button>`;
const SEND_GLYPH = '<svg class="send-glyph" viewBox="0 0 20 20" fill="currentColor" aria-hidden="true" focusable="false"><path d="M2.4 9.1l14.3-6.4c.7-.3 1.4.4 1.1 1.1l-6.4 14.3c-.3.7-1.4.6-1.5-.2l-.8-5.3-5.3-.8c-.8-.1-.9-1.2-.2-1.5z"/></svg>';

/**
 * Pebble composer (cas-3800): the field is a pill on --panel with --lift, the
 * send button takes the machine accent from the shell root, and the attach
 * clip sits beside the field, present but disabled. Dresses the existing
 * `.message` region in place so the ids the live-region updater and the send
 * handler bind to are untouched.
 *
 * The field and the button lead with the project, never the generated
 * codename (journey F13): "Message the cas-src supervisor" and "Send". The
 * codename is an identifier, not a name to read in prose; the header shows it.
 * The button's accessible name names the project's supervisor too, "Send to
 * the cas-src supervisor" (cas-71f4, journey F20).
 */
export function dressComposer(composer: HTMLElement, supervisor?: string, project?: string): void {
  composer.classList.add("conversation-composer");
  const label = composer.querySelector("label"); if (label) label.textContent = "Your message";
  composer.querySelector("h2")?.classList.add("sr-only");
  const input = composer.querySelector("textarea");
  if (input) {
    // The longest wording that fits the field is chosen when the field is
    // laid out and again whenever it resizes, so a phone never shows a cut
    // "Message the gabber-studio supervi…" (cas-1e0f QA F01). The stylesheet
    // still ellipsises on one line as the last guard (journey F9).
    input.dataset.placeholders = JSON.stringify(composerPlaceholders(project));
    fitComposerPlaceholder(input);
    // dressComposer runs before a rebuild re-attaches the composer: fit again
    // once it is back in the document.
    queueMicrotask(() => fitComposerPlaceholder(input));
    watchComposerPlaceholder(input);
    input.rows = 1;
    // Hidden until attaching works (cas-17e3): a control that can never be
    // used is noise for every reader, and a dead stop for keyboard users.
    if (ATTACH_SUPPORTED && !composer.querySelector(".composer-clip")) input.insertAdjacentHTML("beforebegin", composerClipMarkup);
    if (!ATTACH_SUPPORTED) composer.querySelector(".composer-clip")?.remove();
  }
  const button = composer.querySelector<HTMLElement>("#message-send");
  if (button) {
    button.classList.add("send");
    button.setAttribute("aria-label", project ? `Send to the ${project} supervisor` : "Send to the supervisor");
    button.replaceChildren();
    button.insertAdjacentHTML("afterbegin", SEND_GLYPH);
    const text = composer.ownerDocument.createElement("span"); text.className = "send-label"; text.textContent = "Send";
    button.append(text);
  }
}

/**
 * The composer's placeholder wordings, longest first: the project's
 * supervisor, then the role. Never the codename, and
 * never a name cut inside the text (cas-1e0f QA F02).
 */
export function composerPlaceholders(project?: string): string[] {
  const name = project?.trim();
  // cas-71f4 (journey F20): no "Message <project>" step, which read as
  // messaging the project itself; the role is the short wording.
  return name ? [`Message the ${name} supervisor`, COMPOSER_ROLE_PLACEHOLDER] : [COMPOSER_ROLE_PLACEHOLDER];
}

export const COMPOSER_ROLE_PLACEHOLDER = "Message the supervisor";

/** The composer's resting placeholder before it is measured: the fullest wording. */
export function composerPlaceholder(project?: string): string {
  return composerPlaceholders(project)[0];
}

/** The first wording whose width fits `available` px; the last (shortest) when none does. */
export function fittingPlaceholder(candidates: readonly string[], available: number, measure: (text: string) => number): string {
  return candidates.find((text) => measure(text) <= available) ?? candidates[candidates.length - 1];
}

/**
 * Measure the field's placeholder wordings in its own font and keep the
 * longest that fits its text box. Not laid out yet (no width): keep the
 * fullest; the resize watch fits it once there is a box. While dictation
 * holds the placeholder, the fitted wording becomes the one it restores.
 */
export function fitComposerPlaceholder(field: HTMLTextAreaElement): void {
  let candidates: string[];
  try { candidates = JSON.parse(field.dataset.placeholders ?? "[]") as string[]; } catch { return; }
  if (!candidates.length) return;
  const document = field.ownerDocument;
  const view = document.defaultView;
  const current = field.dataset.restingPlaceholder ?? field.placeholder;
  // Unmeasurable (detached mid-rebuild, not laid out): keep a wording already
  // fitted for these candidates; the microtask and resize watch refit it.
  let chosen = candidates.includes(current) ? current : candidates[0];
  if (view && field.isConnected && field.clientWidth > 0) {
    const style = view.getComputedStyle(field);
    const available = field.clientWidth - parseFloat(style.paddingLeft || "0") - parseFloat(style.paddingRight || "0");
    const probe = document.createElement("span");
    probe.setAttribute("aria-hidden", "true");
    probe.style.cssText = "position:absolute;left:-10000px;top:0;visibility:hidden;white-space:pre;";
    probe.style.font = style.font;
    probe.style.letterSpacing = style.letterSpacing;
    document.body.append(probe);
    chosen = fittingPlaceholder(candidates, available, (text) => { probe.textContent = text; return probe.getBoundingClientRect().width; });
    probe.remove();
  }
  if (field.dataset.restingPlaceholder !== undefined) field.dataset.restingPlaceholder = chosen;
  else if (field.placeholder !== chosen) field.placeholder = chosen;
}

const watchedComposerFields = new WeakSet<HTMLTextAreaElement>();

/** Refit on every size change of the field (a rotation, a breakpoint, the rail opening) and once web fonts land. */
function watchComposerPlaceholder(field: HTMLTextAreaElement): void {
  if (watchedComposerFields.has(field)) return;
  watchedComposerFields.add(field);
  const view = field.ownerDocument.defaultView as (Window & typeof globalThis) | null;
  if (view && typeof view.ResizeObserver === "function") new view.ResizeObserver(() => fitComposerPlaceholder(field)).observe(field);
  void field.ownerDocument.fonts?.ready.then(() => fitComposerPlaceholder(field));
}

/** The list's empty line: loading, unpaired, or paired with nothing live. */
export function conversationEmptyText(catalogLoaded: boolean, machineCount: number): string {
  return !catalogLoaded ? "Loading paired machines…" : machineCount === 0 ? "Pair a machine to start your first conversation." : "No live supervisors listed. Use Appearance & commands to show dormant sessions for recovery.";
}

/** What the list knows about one paired machine's session catalog. */
export interface MachineCatalogProgress {
  /** The hub has answered with a session catalog at least once in this visit. */
  catalogReceived: boolean;
  phase: string | undefined;
  /** cas-97d58 F16: the browser itself blocks the connection (Local network access denied). */
  browserBlocked?: boolean;
}

export type ConversationListState = { readonly kind: "loading" } | { readonly kind: "text"; readonly text: string };

/**
 * What an empty conversation list shows (journey F14). Until browser storage
 * and the machines' first catalog responses arrive it is a loading skeleton,
 * never "Not paired" or "No live supervisors". The empty copy needs an actual
 * empty catalog; a machine that could not be reached on its first attempt is
 * named as unreachable instead of being read as "nothing live".
 */
export function conversationListState(storageLoaded: boolean, machines: readonly MachineCatalogProgress[]): ConversationListState {
  if (!storageLoaded) return { kind: "loading" };
  if (machines.length === 0) return { kind: "text", text: conversationEmptyText(true, 0) };
  const unanswered = machines.filter((machine) => !machine.catalogReceived);
  // Still on a first attempt: no answer yet, and no failure either.
  if (unanswered.some((machine) => machine.phase !== "failed" && machine.phase !== "backoff")) return { kind: "loading" };
  // cas-97d58 F16: when this browser blocks every machine, the cause is the
  // browser, not "check that the machines are awake". The permission notice
  // beside it carries the one remedy (cas-7c37f).
  if (unanswered.length === machines.length && unanswered.every((machine) => machine.browserBlocked)) return { kind: "text", text: "This browser is blocking its connection to your paired machines." };
  if (unanswered.length === machines.length) return { kind: "text", text: "Can't reach your paired machines yet. Cassy keeps retrying; check that the machines are awake and on your network." };
  return { kind: "text", text: conversationEmptyText(true, machines.length) };
}

/** Three placeholder rows in the list's own geometry, announced once as loading. */
export function conversationSkeletonMarkup(): string {
  const row = '<span class="conversation-skeleton-row" aria-hidden="true"><span class="conversation-skeleton-avatar"></span><span class="conversation-skeleton-lines"><span></span><span></span></span></span>';
  return `<div class="conversation-skeleton" role="status"><span class="sr-only">Loading your conversations…</span>${row}${row}${row}</div>`;
}

/**
 * The desktop context rail (P10). It holds only what the thread header does
 * not: open asks and blockers, task progress, the thread's attachments and
 * its attention events — each section starts hidden and syncContextRail
 * (context-rail.ts) reveals the ones with data. With none, the rail folds to
 * the 48px context track. No lockup, badge, supervisor or host: those live in
 * the header beside it.
 */
function contextRailMarkup(selected: boolean): string {
  const sections = selected
    ? '<section class="context-section" data-section="waiting" aria-labelledby="context-waiting-heading" hidden><h2 id="context-waiting-heading">Waiting on you</h2><ul class="context-list context-waiting"></ul></section>'
      + '<section class="context-section" data-section="progress" aria-labelledby="context-progress-heading" hidden><h2 id="context-progress-heading">Tasks &amp; progress</h2><p class="status-stale" role="status" hidden></p><div id="conversation-status-slot"></div></section>'
      + '<section class="context-section" data-section="attachments" aria-labelledby="context-attachments-heading" hidden><h2 id="context-attachments-heading">Attachments</h2><ul class="context-list context-attachments"></ul></section>'
      + '<section class="context-section" data-section="attention" hidden><div id="conversation-attention-slot"></div></section>'
    : "";
  const close = selected ? '<button class="context-sheet-close" type="button" aria-label="Close attention">×</button>' : "";
  return `<aside class="conversation-context" aria-label="Conversation context" data-open="false" aria-hidden="true">${close}${sections}</aside>`;
}

/** Appearance & commands as a header icon button (P13): out of the phone thumb zone, named for assistive tech. */
export const appearanceButtonMarkup = (shortcut = paletteShortcutLabel()): string => `<button id="command-palette-toggle" class="icon-button" type="button" aria-label="Appearance &amp; commands" title="Appearance &amp; commands (${shortcut} twice)"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" aria-hidden="true" focusable="false"><path d="M4 7h9M17 7h3M4 17h3M11 17h9"/><circle cx="15" cy="7" r="2"/><circle cx="9" cy="17" r="2"/></svg></button>`;

export function conversationShellMarkup(model: ConversationShellModel): string {
  const networkHelp = '<p id="network-access-help" class="compatibility-warning" role="status" hidden></p>';
  // First run: the welcome carries the one primary "Pair a machine". The list
  // header's chip is hidden beside it on wide screens (styles.css
  // .welcome-pairs) and, on a phone where the welcome is not shown, it is the
  // primary action itself.
  const welcomePairs = !model.selected && model.loaded && !model.paired;
  return `<div class="conversation-shell${model.selected ? " thread-open" : ""}${welcomePairs ? " welcome-pairs" : ""}${model.machineId ? ` ${machineAccentClass(model.machineId)}` : ""}">
    <aside class="conversation-sidebar" aria-label="Supervisor conversations">
      <header class="conversation-list-heading"><div class="conversation-list-top">${cloudBrand()}<span class="conversation-list-tools"><button id="pair-toggle"${welcomePairs ? ' class="primary"' : ""} type="button">Pair a machine</button>${appearanceButtonMarkup()}</span></div><div class="conversation-list-title"><h1>Conversations</h1>${model.launch ? `<span class="conversation-list-actions">${newSessionButtonMarkup(model.launch)}</span>` : ""}</div><p>Your projects. Your supervisors. <button id="inbox-toggle" class="link-button" type="button" aria-haspopup="dialog">Operator inbox</button></p>${model.paired ? conversationSearchMarkup(model.searchQuery, model.keyboardHint ?? true) : ""}</header>
      <nav id="conversation-list" aria-label="Choose a supervisor"></nav>
      <div id="conversation-empty" class="conversation-empty" hidden></div>
      ${!model.selected ? networkHelp : ""}
      ${model.paired ? composeFabMarkup : ""}
      <footer><div id="hub-footer-badges" class="hub-footer-badges" aria-label="Hub status"></div></footer>
    </aside>
    <main class="conversation-main">
      ${model.selected ? `${conversationHeaderMarkup(model)}${networkHelp}<section id="conversation-pane-slot" class="conversation-pane-slot"></section><div id="conversation-composer-slot"></div>` : `<div class="conversation-welcome"><span class="conversation-eyebrow">SUPERVISOR CONVERSATIONS</span><h2>Stay close to the work.</h2><p>${!model.loaded ? "Loading your paired machines…" : !model.paired ? "Pair a machine to read your supervisors’ words and talk to them here." : "Choose a project to read its supervisor’s words and send an instruction."}</p>${!model.paired && model.loaded ? '<button id="empty-pair" class="primary" type="button">Pair a machine</button><p class="conversation-welcome-inbox">Machine off? <button id="empty-inbox" class="link-button" type="button" aria-haspopup="dialog">Read your inbox</button> without pairing.</p>' : ""}</div>`}
    </main>
    ${contextRailMarkup(model.selected)}
  </div>`;
}

/** The room a machine name keeps on a "machine · codename" line, in ch (cas-766c). */
export const MACHINE_KEEP_CH = 16;
/** The least a codename shows before it steps aside for the machine, in ch (cas-766c). */
export const CODENAME_KEEP_CH = 8;

/**
 * Fit a "machine · codename" line to `available` px (cas-766c). The machine
 * name is the one thing that says where a conversation runs, so it holds its
 * place; the generated codename yields:
 * 1. everything whole, if it fits;
 * 2. else the machine's OS word goes ("Studio Mac · macOS" → "Studio Mac"),
 *    so it is never cut mid-word;
 * 3. the codename ellipsises (CSS), down to CODENAME_KEEP_CH, while the
 *    machine keeps up to MACHINE_KEEP_CH;
 * 4. with no room even for that, the codename and its separator step aside
 *    and the machine ellipsises alone.
 * The line's title always carries the whole "machine · codename". Decided
 * from widths that don't depend on the current state, so it never flips.
 */
export function fitMachineLine(line: HTMLElement | null | undefined, available: number): void {
  if (!line) return;
  line.classList.remove("os-dropped", "codename-squeezed", "machine-long", "machine-squeezed");
  const codename = line.querySelector<HTMLElement>(":scope > .codename");
  const machine = line.querySelector<HTMLElement>(":scope > .host-machine, :scope > .proj2-machine");
  // A dropped OS word is an accessibility helper: hidden from sight, still heard (cas-8526, cas-0546).
  const os = machine?.querySelector<HTMLElement>(".host-os");
  os?.classList.remove("sr-only");
  if (!codename || !machine || !(available > 0)) return;
  const ch = parseFloat(getComputedStyle(codename).fontSize) * 0.6;
  const separator = line.querySelector<HTMLElement>(":scope > .host-sep, :scope > .proj2-sep");
  const gap = separator?.getBoundingClientRect().width || 3 * ch;
  if (machine.scrollWidth + gap + codename.scrollWidth <= available + 1) return;
  os?.classList.add("sr-only");
  if (machine.scrollWidth > MACHINE_KEEP_CH * ch) line.classList.add("machine-long");
  const kept = Math.min(machine.scrollWidth, MACHINE_KEEP_CH * ch);
  if (kept + gap + Math.min(codename.scrollWidth, CODENAME_KEEP_CH * ch) > available + 1) line.classList.add("codename-squeezed");
}

/** The conversation header's host line, fitted to the room beside its connection state. */
export function fitConversationHost(root: ParentNode): void {
  const identity = root.querySelector<HTMLElement>(".conversation-identity");
  const host = identity?.querySelector<HTMLElement>(".conversation-host");
  if (!identity || !host) return;
  // The room is the identity row less the avatar, not the host line's own
  // width: that line is sized to its content, so it shrinks once the machine
  // steps aside and would keep it aside for good.
  const avatar = identity.querySelector<HTMLElement>(":scope > .conversation-avatar");
  const style = getComputedStyle(identity);
  const room = identity.clientWidth - parseFloat(style.paddingLeft || "0") - parseFloat(style.paddingRight || "0")
    - (avatar ? avatar.getBoundingClientRect().width + (parseFloat(style.columnGap) || 0) : 0);
  const connection = host.querySelector<HTMLElement>("#conversation-connection");
  fitMachineLine(host.querySelector<HTMLElement>(":scope > .host-where"), room - (connection?.getBoundingClientRect().width ?? 0));
}

/**
 * cas-0546: a conversation action that can't run says why instead of
 * vanishing. It stays focusable and in the tab order, is marked
 * aria-disabled, and is described by its reason, which a click or Enter also
 * shows; with no reason it is an ordinary button again.
 */
export function applyActionAvailability(button: HTMLButtonElement | null, note: HTMLElement | null, reason: string | undefined): void {
  if (!button) return;
  if (reason === undefined) {
    if (button.dataset.disabledReason === undefined) return;
    delete button.dataset.disabledReason;
    button.removeAttribute("aria-disabled");
    button.removeAttribute("aria-describedby");
    button.removeAttribute("title");
    if (note) note.textContent = "";
    return;
  }
  if (button.dataset.disabledReason === reason && note?.textContent === reason) return;
  button.dataset.disabledReason = reason;
  button.setAttribute("aria-disabled", "true");
  if (note?.id) button.setAttribute("aria-describedby", note.id);
  button.title = reason;
  if (note && note.textContent !== reason) note.textContent = reason;
}

/**
 * The host a session's pane surface lives in (cas-0546). The surface is
 * internal plumbing — it keeps the attach, the keyframes and the raw text the
 * Raw output drawer reads — so its host renders nothing: hidden (display
 * none, also pinned in styles.css), inert, and out of the accessibility tree.
 */
export function createPaneHost(document: Document): HTMLElement {
  const host = document.createElement("div");
  host.className = "pane-host";
  host.hidden = true;
  host.setAttribute("inert", "");
  host.setAttribute("aria-hidden", "true");
  return host;
}

/**
 * The open thread's stage inside the grid: a visible slot the conversation
 * mounts in, and beside it — never around it — the hidden pane host. Existing
 * nodes are kept, so a heartbeat never remounts the thread.
 */
export function ensureConversationStage(grid: HTMLElement): { slot: HTMLElement; host: HTMLElement } {
  let slot = grid.querySelector<HTMLElement>(":scope > .conversation-thread-slot");
  let host = grid.querySelector<HTMLElement>(":scope > .pane-host");
  if (!slot || !host) {
    slot = grid.ownerDocument.createElement("div");
    slot.className = "conversation-mount conversation-thread-slot";
    host = createPaneHost(grid.ownerDocument);
    grid.replaceChildren(slot, host);
  }
  return { slot, host };
}

/** The Raw output drawer's contents; main.ts mounts the transcript in its body. */
export function rawOutputDrawerMarkup(): string {
  return '<header class="raw-output-header"><div class="raw-output-heading"><h2 id="raw-output-title">Raw output</h2><p id="raw-output-subject" class="raw-output-subject"></p></div><button type="button" class="raw-output-close" aria-label="Close raw output">×</button></header><div class="raw-output-body"><p class="raw-output-empty" role="status" hidden>The supervisor\'s output appears here once the conversation is connected.</p></div>';
}

/** The regions an open conversation's shell holds; each keeps the id its updater reads. */
export interface ConversationRegions {
  readonly grid?: HTMLElement;
  readonly composer?: HTMLElement;
  readonly status?: HTMLElement;
  readonly attention?: HTMLElement;
}

/**
 * The conversation shell, with an open thread's regions in their slots: the
 * grid (thread, connection card and hidden pane host), the composer, the
 * session's status and its attention.
 */
export function arrangeConversationShell(document: Document, model: ConversationShellModel, regions: ConversationRegions = {}): HTMLElement {
  const template = document.createElement("template"); template.innerHTML = conversationShellMarkup(model);
  const shell = template.content.firstElementChild as HTMLElement;
  if (model.selected) {
    if (regions.grid) shell.querySelector("#conversation-pane-slot")!.append(regions.grid);
    if (regions.composer) {
      dressComposer(regions.composer, model.supervisor, projectTitle(model.projectDir));
      shell.querySelector("#conversation-composer-slot")!.append(regions.composer);
    }
    if (regions.status) shell.querySelector("#conversation-status-slot")!.append(regions.status);
    if (regions.attention) { regions.attention.hidden = false; shell.querySelector("#conversation-attention-slot")!.append(regions.attention); }
  }
  return shell;
}

/* ---- Phone keyboard (cas-edc9) --------------------------------------------
 * The viewport meta asks the browser to resize the layout viewport when the
 * keyboard opens (interactive-widget=resizes-content), so 100dvh shrinks and
 * the shell — header, thread, composer — fits above the keys. A browser that
 * ignores the meta (iOS Safari, older Chrome) shrinks only the visual viewport
 * and then scrolls the page to reveal the focused field, pushing the header
 * and the thread tail above the top edge. The fallback below measures the
 * visual viewport, publishes its height as --keyboard-viewport-height (the
 * shell's height takes it over 100dvh), and scrolls the page back to the top
 * so the header stays where it is. */
export const KEYBOARD_VIEWPORT_PROPERTY = "--keyboard-viewport-height";

export interface VisualViewportLike {
  readonly height: number;
  readonly offsetTop: number;
  addEventListener(type: "resize" | "scroll", listener: () => void): void;
  removeEventListener(type: "resize" | "scroll", listener: () => void): void;
}

export interface KeyboardViewportWindow {
  readonly innerHeight: number;
  readonly visualViewport?: VisualViewportLike | null;
  readonly document: Document;
  scrollTo(x: number, y: number): void;
}

/**
 * The height the shell should take, or undefined when the layout viewport
 * already matches the visual one (no keyboard, or the meta was honoured and
 * 100dvh is right). A sub-pixel difference is not a keyboard.
 */
export function keyboardViewportHeight(innerHeight: number, visual: { height: number } | null | undefined): number | undefined {
  if (!visual || !(visual.height > 0)) return undefined;
  const height = Math.round(visual.height);
  return height < Math.round(innerHeight) - 1 ? height : undefined;
}

/** Publish (or clear) the shell height on the document root. */
export function applyKeyboardViewport(document: Document, height: number | undefined): void {
  const root = document.documentElement;
  if (height === undefined) root.style.removeProperty(KEYBOARD_VIEWPORT_PROPERTY);
  else root.style.setProperty(KEYBOARD_VIEWPORT_PROPERTY, `${height}px`);
}

/**
 * Keep the shell inside the visual viewport while the keyboard is up. Returns
 * a disposer; a window without visualViewport is left alone (the meta is the
 * only mechanism there).
 */
export function bindKeyboardViewport(window: KeyboardViewportWindow): () => void {
  const visual = window.visualViewport;
  if (!visual) return () => {};
  const sync = () => {
    applyKeyboardViewport(window.document, keyboardViewportHeight(window.innerHeight, visual));
    // The browser scrolled the page to reveal the field; the shell now fits, so put the header back.
    if (visual.offsetTop > 0) window.scrollTo(0, 0);
  };
  visual.addEventListener("resize", sync);
  visual.addEventListener("scroll", sync);
  sync();
  return () => { visual.removeEventListener("resize", sync); visual.removeEventListener("scroll", sync); applyKeyboardViewport(window.document, undefined); };
}
