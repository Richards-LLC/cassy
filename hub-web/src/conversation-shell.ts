import { cloudBrand, escapeHtml, projectBadge } from "./cloud-brand";

export interface ConversationShellModel {
  supervisor?: string;
  projectDir?: string;
  host?: string;
  selected: boolean;
  loaded: boolean;
  paired: boolean;
}

export function conversationShellMarkup(model: ConversationShellModel): string {
  return `<div class="conversation-shell${model.selected ? " thread-open" : ""}">
    <aside class="conversation-sidebar" aria-label="Supervisor conversations">
      <header class="conversation-list-heading">${cloudBrand()}<div class="conversation-list-title"><h1>Conversations</h1><button id="pair-toggle" type="button">Pair a machine</button></div><p>Your projects. Your supervisors.</p></header>
      <nav id="conversation-list" aria-label="Choose a supervisor"></nav>
      <div id="conversation-empty" class="conversation-empty" hidden></div>
      <footer><button id="command-palette-toggle" type="button">Appearance &amp; commands</button></footer>
    </aside>
    <main class="conversation-main">
      ${model.selected ? `<header class="conversation-heading"><div class="conversation-topline"><button id="conversation-back" type="button">‹ Conversations</button>${cloudBrand()}<button id="conversation-terminal" type="button">Terminal view</button></div><div class="conversation-identity">${projectBadge(model.projectDir)}<h1>${escapeHtml(model.supervisor || "Supervisor unavailable")}</h1></div><p class="conversation-host">${escapeHtml(model.host || "")}<span id="conversation-connection" role="status"></span></p></header><section id="conversation-pane-slot" class="conversation-pane-slot"></section><div id="conversation-composer-slot"></div>` : `<div class="conversation-welcome"><span class="conversation-eyebrow">SUPERVISOR CONVERSATIONS</span><h2>Stay close to the work.</h2><p>${!model.loaded ? "Loading your paired machines…" : !model.paired ? "Pair a machine to read your supervisors’ words and talk to them here." : "Choose a project to read its supervisor’s words and send an instruction."}</p>${!model.paired && model.loaded ? '<button id="empty-pair" class="primary" type="button">Pair a machine</button>' : ""}</div>`}
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
      composer.classList.add("conversation-composer");
      composer.querySelector(".operator-thread")?.remove();
      const label = composer.querySelector("label"); if (label) label.textContent = "Your message";
      const input = composer.querySelector("textarea"); if (input) input.placeholder = `Write to ${model.supervisor || "the supervisor"}…`;
      const button = composer.querySelector("#message-send"); if (button) button.textContent = `Send to ${model.supervisor || "supervisor"}`;
      shell.querySelector("#conversation-composer-slot")!.append(composer);
    }
    if (status) shell.querySelector("#conversation-status-slot")!.append(status);
    if (attention) { attention.hidden = false; shell.querySelector("#conversation-attention-slot")!.append(attention); }
  }
  old.replaceWith(shell);
}
