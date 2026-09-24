import { cloudBrand, escapeHtml, projectName } from "./cloud-brand";
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
}

export const CONVERSATION_SEARCH_LABEL = "Search conversations";
export const CONVERSATION_SEARCH_PLACEHOLDER = "Search conversations (Ctrl K)";

/** The list's visible name search (journey F8): filters rows by project,
 * machine or supervisor. Ctrl/Cmd+K focuses it; pressed again from the field,
 * it opens the command palette. */
export function conversationSearchMarkup(query = ""): string {
  return `<div class="conversation-search" role="search"><svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true" focusable="false"><circle cx="8.5" cy="8.5" r="5.5"/><path d="M13 13l4 4"/></svg><input id="conversation-search" type="search" aria-label="${CONVERSATION_SEARCH_LABEL}" aria-controls="conversation-list" aria-keyshortcuts="Control+K Meta+K" placeholder="${CONVERSATION_SEARCH_PLACEHOLDER}" autocomplete="off" spellcheck="false" enterkeyhint="go" value="${escapeHtml(query)}"></div>`;
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
 * list row) — with the cas-11b01 back link and
 * Terminal view control on the row above it. Elevated with --lift-head; the
 * thread view itself renders no header inside this shell.
 *
 * On phone the two rows fold into one (cas-1776): the back link shows only its
 * "‹" glyph and the Terminal control only "Terminal"; each button's aria-label
 * keeps its accessible name ("‹ Conversations", "Terminal view") the same at
 * every width. The project ellipsises (its title carries it whole) and the
 * host line ellipsises its machine · codename part while the connection state
 * after it stays visible.
 */
export function conversationHeaderMarkup(model: ConversationShellModel): string {
  const host = model.host || "";
  const project = projectName(model.projectDir);
  return `<header class="conversation-heading thead"><div class="conversation-topline"><button id="conversation-back" type="button" aria-label="‹ Conversations"><span class="back-glyph">‹</span><span class="back-label"> Conversations</span></button>${cloudBrand()}<button id="conversation-terminal" type="button" aria-label="Terminal view">Terminal<span class="terminal-suffix"> view</span></button></div><div class="conversation-identity"><span class="conversation-avatar" aria-hidden="true">${escapeHtml(machineMonogram(host || model.supervisor || "?"))}</span><div class="id"><h1><b title="${escapeHtml(project)}">${escapeHtml(project)}</b></h1><span class="conversation-host"><span class="host-where">${host ? `${escapeHtml(host)} · ` : ""}<span class="codename">${escapeHtml(model.supervisor || "Supervisor unavailable")}</span></span><span id="conversation-connection" role="status"></span></span></div></div></header>`;
}

/** Attach is a real affordance beside the field but has no transport yet, so it is disabled and says so. */
export const ATTACH_DISABLED_REASON = "Attaching files from this device is not supported yet. Supervisors send reports to you; sending files to a supervisor is coming.";
export const composerClipMarkup = `<button type="button" class="composer-clip" disabled aria-disabled="true" title="${ATTACH_DISABLED_REASON}" aria-label="Attach a file (not yet supported)"><svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"><path d="M14.8 9.1l-5 5a3.2 3.2 0 01-4.6-4.6l6-6a2.1 2.1 0 013 3l-6 6a1 1 0 01-1.4-1.4l5.3-5.3"/></svg></button>`;
const SEND_GLYPH = '<svg class="send-glyph" viewBox="0 0 20 20" fill="currentColor" aria-hidden="true" focusable="false"><path d="M2.4 9.1l14.3-6.4c.7-.3 1.4.4 1.1 1.1l-6.4 14.3c-.3.7-1.4.6-1.5-.2l-.8-5.3-5.3-.8c-.8-.1-.9-1.2-.2-1.5z"/></svg>';

/**
 * Pebble composer (cas-3800): the field is a pill on --panel with --lift, the
 * send button takes the machine accent from the shell root and names the
 * supervisor, and the attach clip sits beside the field, present but
 * disabled. Dresses the existing `.message` region in place so the ids the
 * live-region updater and the send handler bind to are untouched.
 */
export function dressComposer(composer: HTMLElement, supervisor?: string): void {
  const name = supervisor || "the supervisor";
  composer.classList.add("conversation-composer");
  composer.querySelector(".operator-thread")?.remove();
  const label = composer.querySelector("label"); if (label) label.textContent = "Your message";
  composer.querySelector("h2")?.classList.add("sr-only");
  const input = composer.querySelector("textarea");
  if (input) {
    input.placeholder = `Message ${name}`;
    input.rows = 1;
    if (!composer.querySelector(".composer-clip")) input.insertAdjacentHTML("beforebegin", composerClipMarkup);
  }
  const button = composer.querySelector<HTMLElement>("#message-send");
  if (button) {
    button.classList.add("send");
    button.setAttribute("aria-label", `Send to ${supervisor || "supervisor"}`);
    button.replaceChildren();
    button.insertAdjacentHTML("afterbegin", SEND_GLYPH);
    const text = composer.ownerDocument.createElement("span"); text.className = "send-label"; text.textContent = `Send to ${supervisor || "supervisor"}`;
    button.append(text);
  }
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
  return `<aside class="conversation-context" aria-label="Conversation context" data-open="false" aria-hidden="true">${sections}</aside>`;
}

/** Appearance & commands as a header icon button (P13): out of the phone thumb zone, named for assistive tech. */
export const appearanceButtonMarkup = '<button id="command-palette-toggle" class="icon-button" type="button" aria-label="Appearance &amp; commands" title="Appearance &amp; commands (Ctrl K twice)"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" aria-hidden="true" focusable="false"><path d="M4 7h9M17 7h3M4 17h3M11 17h9"/><circle cx="15" cy="7" r="2"/><circle cx="9" cy="17" r="2"/></svg></button>';

export function conversationShellMarkup(model: ConversationShellModel): string {
  // First run: the welcome carries the one primary "Pair a machine". The list
  // header's chip is hidden beside it on wide screens (styles.css
  // .welcome-pairs) and, on a phone where the welcome is not shown, it is the
  // primary action itself.
  const welcomePairs = !model.selected && model.loaded && !model.paired;
  return `<div class="conversation-shell${model.selected ? " thread-open" : ""}${welcomePairs ? " welcome-pairs" : ""}${model.machineId ? ` ${machineAccentClass(model.machineId)}` : ""}">
    <aside class="conversation-sidebar" aria-label="Supervisor conversations">
      <header class="conversation-list-heading"><div class="conversation-list-top">${cloudBrand()}${appearanceButtonMarkup}</div><div class="conversation-list-title"><h1>Conversations</h1><button id="pair-toggle"${welcomePairs ? ' class="primary"' : ""} type="button">Pair a machine</button></div><p>Your projects. Your supervisors.</p>${model.paired ? conversationSearchMarkup(model.searchQuery) : ""}</header>
      <nav id="conversation-list" aria-label="Choose a supervisor"></nav>
      <div id="conversation-empty" class="conversation-empty" hidden></div>
      ${model.paired ? composeFabMarkup : ""}
      <footer><div id="hub-footer-badges" class="hub-footer-badges" aria-label="Hub status"></div></footer>
    </aside>
    <main class="conversation-main">
      ${model.selected ? `${conversationHeaderMarkup(model)}<section id="conversation-pane-slot" class="conversation-pane-slot"></section><div id="conversation-composer-slot"></div>` : `<div class="conversation-welcome"><span class="conversation-eyebrow">SUPERVISOR CONVERSATIONS</span><h2>Stay close to the work.</h2><p>${!model.loaded ? "Loading your paired machines…" : !model.paired ? "Pair a machine to read your supervisors’ words and talk to them here." : "Choose a project to read its supervisor’s words and send an instruction."}</p>${!model.paired && model.loaded ? '<button id="empty-pair" class="primary" type="button">Pair a machine</button>' : ""}</div>`}
    </main>
    ${contextRailMarkup(model.selected)}
  </div>`;
}

/** Rehouse existing owned regions; retain terminal surfaces and composer APIs. */
export function arrangeConversationShell(app: HTMLElement, model: ConversationShellModel): void {
  const old = app.querySelector<HTMLElement>(".shell");
  if (!old) return;
  const grid = old.querySelector<HTMLElement>("#pane-grid");
  const composer = old.querySelector<HTMLElement>(".message");
  const status = old.querySelector<HTMLElement>("#status-view");
  const attention = old.querySelector<HTMLElement>("#attention-panel");
  const template = app.ownerDocument.createElement("template"); template.innerHTML = conversationShellMarkup(model);
  const shell = template.content.firstElementChild!;
  if (model.selected) {
    if (grid) shell.querySelector("#conversation-pane-slot")!.append(grid);
    if (composer) {
      dressComposer(composer, model.supervisor);
      shell.querySelector("#conversation-composer-slot")!.append(composer);
    }
    if (status) shell.querySelector("#conversation-status-slot")!.append(status);
    if (attention) { attention.hidden = false; shell.querySelector("#conversation-attention-slot")!.append(attention); }
  }
  old.replaceWith(shell);
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
