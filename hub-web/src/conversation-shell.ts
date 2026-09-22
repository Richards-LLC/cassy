import { cloudBrand, escapeHtml, projectBadge, projectName } from "./cloud-brand";
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
}

/** Compose is a YOU action: the FAB takes the operator's constant colour
 * (--you-bg at :root), never a machine accent, so it stays visible whatever
 * machine scope the shell root carries. */
export const composeFabMarkup = '<button id="compose-fab" class="compose-fab" type="button" aria-label="Write to a supervisor"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true" focusable="false"><path d="M12 5v14M5 12h14"/></svg></button>';

/**
 * The one header above the thread (cas-5b2d): the Pebble thead — machine
 * monogram in the accent, supervisor bold with the project badge beside it,
 * project · machine · connection in mono beneath (same order as a list row) — with the cas-11b01 back link and
 * Terminal view control on the row above it. Elevated with --lift-head; the
 * thread view itself renders no header inside this shell.
 *
 * On phone the two rows fold into one (cas-1776): the back link shows only its
 * "‹" glyph and the Terminal control only "Terminal"; each button's aria-label
 * keeps its accessible name ("‹ Conversations", "Terminal view") the same at
 * every width. The codename ellipsises (its title carries it whole) and the
 * host line ellipsises its project · machine part while the connection state
 * after it stays visible.
 */
export function conversationHeaderMarkup(model: ConversationShellModel): string {
  const host = model.host || "";
  return `<header class="conversation-heading thead"><div class="conversation-topline"><button id="conversation-back" type="button" aria-label="‹ Conversations"><span class="back-glyph">‹</span><span class="back-label"> Conversations</span></button>${cloudBrand()}<button id="conversation-terminal" type="button" aria-label="Terminal view">Terminal<span class="terminal-suffix"> view</span></button></div><div class="conversation-identity"><span class="conversation-avatar" aria-hidden="true">${escapeHtml(machineMonogram(host || model.supervisor || "?"))}</span><div class="id"><h1><b title="${escapeHtml(model.supervisor || "Supervisor unavailable")}">${escapeHtml(model.supervisor || "Supervisor unavailable")}</b>${projectBadge(model.projectDir)}</h1><span class="conversation-host"><span class="host-where">${escapeHtml([projectName(model.projectDir), host].filter(Boolean).join(" · "))}</span><span id="conversation-connection" role="status"></span></span></div></div></header>`;
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

export function conversationShellMarkup(model: ConversationShellModel): string {
  // First run: the welcome carries the one primary "Pair a machine". The list
  // header's chip is hidden beside it on wide screens (styles.css
  // .welcome-pairs) and, on a phone where the welcome is not shown, it is the
  // primary action itself.
  const welcomePairs = !model.selected && model.loaded && !model.paired;
  return `<div class="conversation-shell${model.selected ? " thread-open" : ""}${welcomePairs ? " welcome-pairs" : ""}${model.machineId ? ` ${machineAccentClass(model.machineId)}` : ""}">
    <aside class="conversation-sidebar" aria-label="Supervisor conversations">
      <header class="conversation-list-heading">${cloudBrand()}<div class="conversation-list-title"><h1>Conversations</h1><button id="pair-toggle"${welcomePairs ? ' class="primary"' : ""} type="button">Pair a machine</button></div><p>Your projects. Your supervisors.</p></header>
      <nav id="conversation-list" aria-label="Choose a supervisor"></nav>
      <div id="conversation-empty" class="conversation-empty" hidden></div>
      <footer><div id="hub-footer-badges" class="hub-footer-badges" aria-label="Hub status"></div><button id="command-palette-toggle" type="button">Appearance &amp; commands</button></footer>
      ${composeFabMarkup}
    </aside>
    <main class="conversation-main">
      ${model.selected ? `${conversationHeaderMarkup(model)}<section id="conversation-pane-slot" class="conversation-pane-slot"></section><div id="conversation-composer-slot"></div>` : `<div class="conversation-welcome"><span class="conversation-eyebrow">SUPERVISOR CONVERSATIONS</span><h2>Stay close to the work.</h2><p>${!model.loaded ? "Loading your paired machines…" : !model.paired ? "Pair a machine to read your supervisors’ words and talk to them here." : "Choose a project to read its supervisor’s words and send an instruction."}</p>${!model.paired && model.loaded ? '<button id="empty-pair" class="primary" type="button">Pair a machine</button>' : ""}</div>`}
    </main>
    <aside class="conversation-context" aria-label="Conversation context">${cloudBrand()}<h2>In this conversation</h2>${model.selected ? `${projectBadge(model.projectDir)}<p class="conversation-context-name">${escapeHtml(model.supervisor || "Supervisor unavailable")}</p><p class="conversation-host">${escapeHtml(model.host || "")}</p><h2>Tasks &amp; progress</h2><p class="status-stale" role="status" hidden></p><div id="conversation-status-slot"></div><div id="conversation-attention-slot"></div>` : '<p class="conversation-host">Project context appears here when you open a conversation.</p>'}</aside>
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
