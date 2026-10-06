// The Operator inbox dialog (cas-9b7d S3): read retained supervisor messages
// from every enrolled machine with no hub reachable, and queue a reply for a
// machine that is off.
//
// It lives outside #app, so the shell's rebuilds never replace it, and it is
// built with DOM nodes and textContent only; supervisor text goes through the
// safe markdown renderer. Copy follows docs/design: plain words first, the
// consequence before the mechanism.

import { renderMarkdown } from "../markdown-renderer";
import { relativeTimestamp } from "../time";
import { commandStatusLabel } from "./commands";
import type { CommandState } from "./store";
import type { InboxSnapshot, InboxState, OperatorInboxController } from "./controller";
import { inboxThreads, type InboxTurn } from "./projection";

export interface InboxViewDeps {
  defaultLabel: string;
  /** Called when the operator opens a thread of a paired machine. */
  onOpenPaired?: (hubId: string, session: string) => void;
}

interface Thread {
  hubId: string;
  machineLabel: string;
  session: string;
  projectId: string;
  sessionId: string;
  turns: InboxTurn[];
  lastAt: string;
}

function el<K extends keyof HTMLElementTagNameMap>(tag: K, attributes: Record<string, string> = {}, text?: string): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  for (const [name, value] of Object.entries(attributes)) node.setAttribute(name, value);
  if (text !== undefined) node.textContent = text;
  return node;
}

/** QA F03: envelopes refused on open or verification are kept for replay
 * coverage and never shown; the operator still learns that they exist. */
export function unverifiedCount(snapshot: InboxSnapshot): number {
  return snapshot.events.filter((event) => event.verification !== "verified").length;
}

export function withheldCopy(count: number): string {
  return count === 1
    ? "1 message couldn’t be verified, so it isn’t shown."
    : `${count} messages couldn’t be verified, so they aren’t shown.`;
}

/** QA F01: the reply's trust state is a state line, not metadata. */
function commandStateMarkup(state: CommandState, machine?: string): HTMLElement {
  return el("span", { class: "operator-inbox-command-state", "data-state": state }, commandStatusLabel(state, machine));
}

/**
 * cas-97d58 F21: the name a person would give this browser ("Chrome on
 * Linux", "Safari on iPhone"), not "Commander on Linux x86_64".
 */
export function browserLabel(userAgent: string): string {
  const browser = /Edg\//.test(userAgent) ? "Edge"
    : /Firefox\//.test(userAgent) ? "Firefox"
      : /Chrome\/|CriOS\//.test(userAgent) ? "Chrome"
        : /Safari\//.test(userAgent) ? "Safari"
          : "Browser";
  const device = /iPhone/.test(userAgent) ? "iPhone"
    : /iPad/.test(userAgent) ? "iPad"
      : /Android/.test(userAgent) ? "Android"
        : /Mac OS X|Macintosh/.test(userAgent) ? "Mac"
          : /Windows/.test(userAgent) ? "Windows"
            : /CrOS/.test(userAgent) ? "ChromeOS"
              : /Linux/.test(userAgent) ? "Linux"
                : "this device";
  return `${browser} on ${device}`;
}

function codename(session: string): string {
  return /([a-z]+-[a-z]+-\d+)$/i.exec(session)?.[1] ?? session;
}

export function inboxThreadList(snapshot: InboxSnapshot): Thread[] {
  const hubs = new Set(snapshot.events.map((event) => event.hubId));
  const labels = new Map(snapshot.machines.map((machine) => [machine.hubId, machine.label ?? "Machine"]));
  const threads: Thread[] = [];
  for (const hubId of hubs) {
    for (const [session, turns] of inboxThreads(snapshot.events, hubId)) {
      const last = turns.at(-1)!;
      threads.push({
        hubId,
        machineLabel: labels.get(hubId) ?? "Machine",
        session,
        projectId: last.projectId,
        sessionId: last.sessionId,
        turns,
        lastAt: last.at,
      });
    }
  }
  return threads.sort((a, b) => (a.lastAt < b.lastAt ? 1 : -1));
}

export class InboxView {
  readonly dialog: HTMLDialogElement;
  private snapshot: InboxSnapshot | null = null;
  private selected: { hubId: string; session: string } | null = null;
  private polling = false;
  private busy = false;
  private status = "";

  constructor(private readonly controller: OperatorInboxController, private readonly deps: InboxViewDeps) {
    this.dialog = el("dialog", { id: "operator-inbox-dialog", "aria-labelledby": "operator-inbox-title", class: "operator-inbox" });
    document.body.append(this.dialog);
    controller.subscribe((snapshot) => {
      this.snapshot = snapshot;
      if (this.dialog.open) this.render();
    });
    this.dialog.addEventListener("close", () => {
      this.selected = null;
    });
  }

  async open(): Promise<void> {
    if (!this.dialog.open) this.dialog.showModal();
    this.snapshot = await this.controller.snapshot();
    this.render();
    if (this.controller.current().kind === "ready") void this.controller.machines().then(async () => {
      this.snapshot = await this.controller.snapshot();
      this.render();
    }).catch(() => undefined);
    this.pollIfWaiting();
  }

  private pollIfWaiting(): void {
    if (this.polling || this.controller.current().kind !== "awaiting_approval") return;
    this.polling = true;
    const step = async () => {
      let delay: number | null = 5000;
      try {
        delay = await this.controller.pollSignIn();
      } catch {
        this.status = "Can't reach Cassy Cloud. Still waiting…";
        this.render();
      }
      if (delay === null) {
        this.polling = false;
        this.status = "";
        this.snapshot = await this.controller.snapshot();
        this.render();
        return;
      }
      window.setTimeout(() => void step(), delay);
    };
    void step();
  }

  private async act(work: () => Promise<unknown>, failure: string): Promise<void> {
    if (this.busy) return;
    this.busy = true;
    this.status = "";
    this.render();
    try {
      await work();
    } catch (error) {
      this.status = `${failure} ${error instanceof Error && "reason" in error ? `(${String((error as { reason: unknown }).reason)})` : ""}`.trim();
    } finally {
      this.busy = false;
      this.snapshot = await this.controller.snapshot();
      this.render();
      this.pollIfWaiting();
    }
  }

  render(): void {
    const state = this.controller.current();
    const body = el("section", { class: "operator-inbox-body" });
    const header = el("header", { class: "operator-inbox-heading" });
    header.append(el("h2", { id: "operator-inbox-title" }, "Operator inbox"));
    const close = el("button", { type: "button", id: "operator-inbox-close", "aria-label": "Close the operator inbox" }, "×");
    close.onclick = () => this.dialog.close();
    header.append(close);
    body.append(header);
    if (this.snapshot?.generationWarning) {
      const warning = el("p", { class: "operator-inbox-warning", role: "status" }, this.snapshot.generationWarning);
      const dismiss = el("button", { type: "button" }, "OK");
      dismiss.onclick = () => this.controller.dismissGenerationWarning();
      warning.append(" ", dismiss);
      body.append(warning);
    }
    body.append(this.stateMarkup(state));
    if (this.status) body.append(el("p", { class: "operator-inbox-status", role: "alert" }, this.status));
    this.dialog.replaceChildren(body);
  }

  private stateMarkup(state: InboxState): HTMLElement {
    switch (state.kind) {
      case "awaiting_approval":
        return this.approvalMarkup(state);
      case "ready":
        return this.readyMarkup(state);
      default:
        return this.signInMarkup(state);
    }
  }

  private signInMarkup(state: InboxState): HTMLElement {
    const section = el("form", { class: "operator-inbox-signin" });
    if (state.kind === "revoked" || state.kind === "unavailable") section.append(el("p", { class: "operator-inbox-warning" }, state.reason));
    section.append(
      el("p", { class: "operator-inbox-lead" }, "Read your supervisors’ messages here even when their machines are off, and leave a reply for when they’re back."),
      // cas-97d58 F21: one brand, and plainly who can read the messages.
      el("p", {}, "Your Cassy Cloud account approves this browser. Cassy Cloud keeps messages for 90 days, encrypted when stored and sent. It holds the keys, so Cassy Cloud can read them as well as you: this isn’t end-to-end encryption."),
    );
    const label = el("label", {}, "Name this browser");
    const input = el("input", { id: "operator-inbox-label", type: "text", maxlength: "80", value: this.deps.defaultLabel });
    label.append(input);
    section.append(label);
    // QA F02: Enter in "Name this browser" signs in, like the button.
    const signIn = el("button", { id: "operator-inbox-signin", type: "submit", class: "primary" }, this.busy ? "Signing in…" : "Sign in");
    section.onsubmit = (event) => {
      event.preventDefault();
      void this.act(() => this.controller.beginSignIn(input.value.trim() || this.deps.defaultLabel), "Sign-in could not start.");
    };
    const actions = el("div", { class: "dialog-actions" });
    actions.append(signIn);
    section.append(actions);
    return section;
  }

  private approvalMarkup(state: Extract<InboxState, { kind: "awaiting_approval" }>): HTMLElement {
    const section = el("div", { class: "operator-inbox-approval" });
    section.append(el("p", { class: "operator-inbox-lead" }, "Approve this browser from your Petra Stella account. Check that the code matches."));
    section.append(el("div", { class: "pair-code operator-inbox-code", "aria-label": "Sign-in code" }, state.userCode));
    const link = el("a", { href: state.approvalUrl, target: "_blank", rel: "noopener noreferrer", class: "button primary", id: "operator-inbox-approve-link" }, "Approve on Cassy Cloud");
    section.append(link);
    // cas-97d58 F21: the command-line route is for engineers; it stays one tap away.
    const other = el("details", { class: "operator-inbox-cli" });
    other.append(el("summary", {}, "Approve from an enrolled machine instead"));
    const command = el("p", {}, "On a machine already enrolled, run ");
    command.append(el("code", {}, `cas hub operator approve ${state.userCode}`));
    other.append(command);
    section.append(other);
    const minutes = Math.max(0, Math.round((Date.parse(state.expiresAt) - Date.now()) / 60_000));
    section.append(el("p", { role: "status", class: "operator-inbox-waiting" }, `Waiting for approval · the code expires in ${minutes < 1 ? "under a minute" : `${minutes} min`}`));
    const cancel = el("button", { type: "button", id: "operator-inbox-cancel" }, "Cancel");
    cancel.onclick = () => void this.act(() => this.controller.cancelSignIn(), "Could not cancel.");
    section.append(cancel);
    return section;
  }

  private readyMarkup(state: Extract<InboxState, { kind: "ready" }>): HTMLElement {
    const section = el("div", { class: "operator-inbox-ready" });
    const snapshot = this.snapshot;
    const threads = snapshot ? inboxThreadList(snapshot) : [];
    const selected = this.selected ? threads.find((thread) => thread.hubId === this.selected!.hubId && thread.session === this.selected!.session) : undefined;
    const withheld = snapshot ? unverifiedCount(snapshot) : 0;
    if (withheld > 0) section.append(el("p", { class: "operator-inbox-withheld", role: "status" }, withheldCopy(withheld)));
    if (selected) {
      section.append(this.threadMarkup(selected));
    } else if (threads.length === 0) {
      section.append(el("p", { class: "operator-inbox-empty" }, "No messages yet. Supervisor messages from enrolled machines appear here, including ones sent while this browser was away."));
    } else {
      const list = el("ul", { class: "operator-inbox-threads", "aria-label": "Conversations in your inbox" });
      for (const thread of threads) {
        const item = el("li");
        const open = el("button", { type: "button", class: "operator-inbox-thread", "data-hub": thread.hubId, "data-session": thread.session });
        const last = thread.turns.at(-1)!;
        open.append(
          el("strong", {}, `${thread.machineLabel} · ${codename(thread.session)}`),
          el("span", { class: "operator-inbox-preview" }, last.kind === "reply" ? last.summary || last.message.slice(0, 120) : `You: ${last.text.slice(0, 120)}`),
          el("time", { datetime: thread.lastAt }, relativeTimestamp(thread.lastAt)),
        );
        open.onclick = () => {
          this.selected = { hubId: thread.hubId, session: thread.session };
          this.render();
        };
        item.append(open);
        list.append(item);
      }
      section.append(list);
    }
    if (snapshot?.expiredThrough) section.append(el("p", { class: "operator-inbox-expired" }, "Older history has expired. Messages are kept for 90 days."));
    const footer = el("footer", { class: "operator-inbox-footer" });
    footer.append(el("span", {}, `Signed in as ${state.label}${state.accountHint ? ` (${state.accountHint})` : ""}`));
    const signOut = el("button", { type: "button", id: "operator-inbox-signout" }, "Sign out of the inbox");
    signOut.onclick = () => void this.act(() => this.controller.signOut(), "Could not sign out.");
    footer.append(signOut);
    section.append(footer);
    return section;
  }

  private threadMarkup(thread: Thread): HTMLElement {
    const section = el("div", { class: "operator-inbox-thread-view" });
    const back = el("button", { type: "button", id: "operator-inbox-back", class: "operator-inbox-back" }, "← All conversations");
    back.onclick = () => {
      this.selected = null;
      this.render();
    };
    section.append(back, el("h3", {}, `${thread.machineLabel} · ${codename(thread.session)}`));
    const log = el("ol", { class: "operator-inbox-turns", "aria-label": `Messages with ${codename(thread.session)}` });
    const commands = new Map((this.snapshot?.commands ?? []).map((command) => [command.commandId, command]));
    // Like the Conversations thread, a run of turns by one author is named once.
    let previousAuthor: string | null = null;
    const author = (name: string): HTMLElement[] => {
      const first = name !== previousAuthor;
      previousAuthor = name;
      return first ? [el("span", { class: "operator-inbox-author" }, name)] : [];
    };
    for (const turn of thread.turns) {
      const item = el("li", { class: `operator-inbox-turn operator-inbox-${turn.kind}` });
      if (turn.kind === "reply") {
        const bubble = el("div", { class: "operator-inbox-bubble" });
        bubble.append(...renderMarkdown(document, turn.message));
        item.append(...author("Supervisor"), bubble);
      } else {
        item.append(...author("You"), el("p", { class: "operator-inbox-bubble" }, turn.text));
        if (turn.kind === "command") {
          const command = commands.get(turn.commandId);
          item.append(commandStateMarkup(command?.state ?? "pending_machine", thread.machineLabel));
        }
      }
      item.append(el("time", { datetime: turn.at }, relativeTimestamp(turn.at)));
      log.append(item);
    }
    // This device's own queued messages that have not replayed back yet.
    for (const command of this.snapshot?.commands ?? []) {
      if (command.hubId !== thread.hubId || command.sessionName !== thread.session) continue;
      if (thread.turns.some((turn) => turn.kind === "command" && turn.commandId === command.commandId)) continue;
      const item = el("li", { class: "operator-inbox-turn operator-inbox-command" });
      item.append(...author("You"), el("p", { class: "operator-inbox-bubble" }, command.body), commandStateMarkup(command.state, thread.machineLabel));
      log.append(item);
    }
    section.append(log);
    section.append(this.composerMarkup(thread));
    return section;
  }

  private composerMarkup(thread: Thread): HTMLElement {
    const form = el("form", { class: "operator-inbox-composer" });
    const machine = this.snapshot?.machines.find((entry) => entry.hubId === thread.hubId && entry.status === "active");
    const granted = this.controller.commandScopes().some(
      (scope) =>
        scope.hub_id === thread.hubId &&
        scope.project_id === thread.projectId &&
        (scope.session_id === null || scope.session_id === thread.sessionId) &&
        scope.operations.includes("operator_message"),
    );
    if (!machine || !granted) {
      form.append(el("p", { class: "operator-inbox-hint" }, !machine
        ? "This machine is not enrolled for offline replies."
        : "This browser can read this conversation but not reply. To reply from it, sign out of the inbox, sign in again and allow replies when you approve it."));
      return form;
    }
    const label = el("label", { for: "operator-inbox-reply" }, `Reply — ${machine.label ?? "the machine"} gets it when it’s back`);
    const text = el("textarea", { id: "operator-inbox-reply", rows: "3", maxlength: "8000" });
    const send = el("button", { type: "submit", class: "primary", id: "operator-inbox-send" }, this.busy ? "Queuing…" : "Queue reply");
    form.append(label, text, send);
    form.onsubmit = (event) => {
      event.preventDefault();
      const body = text.value.trim();
      if (!body) return;
      void this.act(async () => {
        await this.controller.queueMessage(
          { machineId: machine.machineId, hubId: thread.hubId, projectId: thread.projectId, sessionId: thread.sessionId, sessionName: thread.session },
          body,
        );
        text.value = "";
      }, "The reply was not queued.");
    };
    return form;
  }
}
