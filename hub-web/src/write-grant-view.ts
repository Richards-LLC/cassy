/**
 * The Write access panel (cas-ab04): task, folder, modes and reason, then an
 * inline confirmation, then the receipt. The hub records the grant from the
 * paired device and posts a receipt into the conversation as an operator turn.
 */

import type { FleetTask } from "./fleet-ops";
import { WRITE_MODES, type WriteGrantState } from "./write-grant";

export interface WriteAccessHandlers {
  changed(): void;
  review(): void;
  revoke(): void;
  confirm(): void;
  cancel(): void;
}

export interface WriteAccessContext {
  readonly state: WriteGrantState;
  readonly tasks: readonly FleetTask[];
  readonly on: WriteAccessHandlers;
  /** On a phone, the conversation and machine the sheet acts on ("cas-src on Atlas"). */
  readonly where?: string;
}

export const WRITE_ACCESS_TITLE = "Write access outside the worktree";

function labelled(document: Document, text: string, control: HTMLElement): HTMLLabelElement {
  const label = document.createElement("label");
  label.className = "write-grant-field";
  const caption = document.createElement("span");
  caption.textContent = text;
  label.append(caption, control);
  return label;
}

function button(document: Document, text: string, className: string, focusKey: string, onclick: () => void): HTMLButtonElement {
  const node = document.createElement("button");
  node.type = "button";
  node.className = className;
  node.textContent = text;
  node.dataset.fleetFocus = focusKey;
  node.onclick = onclick;
  return node;
}

export function writeAccessPanel(document: Document, context: WriteAccessContext): HTMLElement {
  const { state, on } = context;
  const panel = document.createElement("div");
  panel.className = "fleet-ops-panel write-grant";
  panel.setAttribute("role", "group");
  // cas-4cf2: the visible title is the accessible name; a phone sheet names
  // its close button for the panel.
  panel.setAttribute("aria-label", WRITE_ACCESS_TITLE);
  panel.dataset.closeLabel = "Close write access";
  const title = document.createElement("h4");
  title.className = "write-grant-title";
  title.textContent = WRITE_ACCESS_TITLE;
  const lead = document.createElement("p");
  lead.className = "write-grant-lead";
  lead.textContent = "Lets the agents on one task write to a folder outside their worktree until the task closes.";
  panel.append(title);
  if (context.where) {
    const where = document.createElement("p");
    where.className = "write-grant-where";
    where.textContent = context.where;
    panel.append(where);
  }
  panel.append(lead);

  if (state.stage === "confirm-grant" || state.stage === "confirm-revoke" || state.stage === "sending") {
    const confirm = document.createElement("div");
    confirm.className = "fleet-ops-confirm write-grant-confirm";
    confirm.setAttribute("role", "alertdialog");
    confirm.setAttribute("aria-label", state.stage === "confirm-revoke" ? "Confirm revoke" : "Confirm write access");
    const question = document.createElement("p");
    question.textContent = state.question();
    const cancel = button(document, "Cancel", "write-grant-cancel", "header:grant-cancel", () => on.cancel());
    const go = button(document, state.stage === "confirm-revoke" ? "Revoke" : "Grant", "write-grant-go primary danger", "header:grant-go", () => on.confirm());
    if (state.stage === "sending") {
      go.disabled = true;
      cancel.disabled = true;
    }
    confirm.append(question, cancel, go);
    panel.append(confirm);
  } else {
    const task = document.createElement("select");
    task.className = "write-grant-task";
    for (const item of context.tasks) task.append(new Option(`${item.id}${item.title ? ` · ${item.title}` : ""}`, item.id));
    task.value = state.draft.task;
    task.dataset.fleetFocus = "header:grant-task";
    task.onchange = () => { state.draft.task = task.value; on.changed(); };

    const path = document.createElement("input");
    path.className = "write-grant-path";
    path.type = "text";
    path.placeholder = "~/folder/outside/the/worktree";
    path.autocapitalize = "off";
    path.spellcheck = false;
    path.value = state.draft.path;
    path.dataset.fleetFocus = "header:grant-path";
    path.oninput = () => { state.draft.path = path.value; on.changed(); };

    const modes = document.createElement("fieldset");
    modes.className = "write-grant-modes";
    const legend = document.createElement("legend");
    legend.textContent = "Allow";
    modes.append(legend);
    for (const mode of WRITE_MODES) {
      const box = document.createElement("input");
      box.type = "checkbox";
      box.className = "write-grant-mode";
      box.value = mode;
      box.checked = state.draft.modes.has(mode);
      box.onchange = () => {
        if (box.checked) state.draft.modes.add(mode);
        else state.draft.modes.delete(mode);
        on.changed();
      };
      const label = document.createElement("label");
      label.append(box, ` ${mode}`);
      modes.append(label);
    }

    const reason = document.createElement("input");
    reason.className = "write-grant-reason";
    reason.type = "text";
    reason.placeholder = "Why: recorded on the task";
    reason.value = state.draft.reason;
    reason.dataset.fleetFocus = "header:grant-reason";
    reason.oninput = () => { state.draft.reason = reason.value; on.changed(); };

    const actions = document.createElement("div");
    actions.className = "write-grant-actions";
    actions.append(
      button(document, "Review grant", "write-grant-review primary", "header:grant-review", () => on.review()),
      button(document, "Revoke…", "write-grant-revoke", "header:grant-revoke", () => on.revoke()),
    );
    panel.append(labelled(document, "Task", task), labelled(document, "Folder", path), modes, labelled(document, "Reason", reason), actions);
  }

  if (state.result) {
    const line = document.createElement("p");
    line.className = `write-grant-result fleet-ops-note fleet-ops-note--${state.result.tone === "ok" ? "ok" : "error"}`;
    line.setAttribute("role", "status");
    line.tabIndex = -1;
    line.dataset.fleetFocus = "header:grant-result";
    // cas-06e8: the task id never breaks at its hyphen.
    const at = state.result.task ? state.result.text.indexOf(state.result.task) : -1;
    if (at < 0) line.textContent = state.result.text;
    else {
      const id = document.createElement("span");
      id.className = "write-grant-id";
      id.textContent = state.result.task!;
      line.append(state.result.text.slice(0, at), id, state.result.text.slice(at + state.result.task!.length));
    }
    panel.append(line);
  }
  return panel;
}
