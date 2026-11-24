/** Fixed semantic controls; the hub and daemon independently enforce this list. */
export type SupervisorCommand = "compact" | "clear" | "model" | "usage" | "exit" | "finish" | "standby" | "mcp";
export type CommandKey = "previous" | "next" | "select" | "cancel";
export interface CommandReceipt {
  client_ref: string;
  command: SupervisorCommand;
  phase: "queued" | "submitted" | "observed" | "refused" | "unconfirmed" | "cancelled";
  detail: string;
  output: string;
  interactive: boolean;
}
const commands: { id: SupervisorCommand; label: string; effect: string; confirm?: string }[] = [
  { id: "compact", label: "Compact", effect: "Summarize the supervisor's context" },
  { id: "usage", label: "Usage", effect: "Read usage from the supervisor's harness" },
  { id: "model", label: "Model", effect: "Open the harness's model chooser" },
  { id: "mcp", label: "Reload Cassy", effect: "Load the installed Cassy build and show the version it loaded" },
  { id: "clear", label: "Clear", effect: "Start a fresh conversation context", confirm: "Clear this supervisor's conversation context? Existing messages remain in Commander." },
  { id: "finish", label: "Finish", effect: "Run the supervisor's Finish skill", confirm: "Run Finish? The supervisor will wrap up its work and may end this session." },
  { id: "standby", label: "Standby", effect: "Finish, stop the workers and go quiet", confirm: "Go on standby? The supervisor will finish its wrap-up, stop its workers and go quiet. Resume brings them back." },
  { id: "exit", label: "Exit", effect: "End the supervisor's harness", confirm: "Exit this supervisor? Its harness will end. Check its handoff before leaving." },
];
/** Typed names that mean the same semantic command. */
const COMMAND_ALIASES: Readonly<Record<string, SupervisorCommand>> = { "reload-cassy": "mcp" };
function commandMatches(id: SupervisorCommand, query: string): boolean {
  return id.startsWith(query) || Object.entries(COMMAND_ALIASES).some(([alias, target]) => target === id && query.length > 0 && alias.startsWith(query));
}

/** Codex has no MCP reconnect, so its Reload Cassy restarts the supervisor (cas-8c4f). */
export const CODEX_RELOAD_CONFIRM = "Reload Cassy? Codex reloads by restarting its supervisor on the installed build, with a fresh context. Workers keep running.";

export function commandsForHarness(harness: string | undefined): typeof commands {
  return harness === "claude" ? commands
    : harness === "codex" ? commands.map(command => command.id === "mcp" ? { ...command, confirm: CODEX_RELOAD_CONFIRM } : command) : [];
}
interface Options {
  harness(): string | undefined;
  unavailable(): string | undefined;
  label(): string;
  session: SupervisorCommandSession;
}

type CommandOrigin = "composer" | "header";
interface CommandIntent {
  birth?: string;
  origin: CommandOrigin;
  command: SupervisorCommand;
  phase: "confirming" | "sending" | "failed";
  draft?: string;
  focus?: "cancel" | "confirm";
  message?: string;
}

/** Owns only the mounted composer's local menu, never session or lease state. */
export class SlashCommands {
  private readonly panel: HTMLElement;
  private open = false;
  private escaped = false;
  private anchor?: HTMLElement;
  private readonly events = new AbortController();
  constructor(private readonly input: HTMLTextAreaElement, private readonly options: Options) {
    this.panel = document.createElement("section");
    this.panel.className = "supervisor-command-palette";
    this.panel.id = "supervisor-command-palette";
    this.panel.setAttribute("aria-label", "Supervisor commands");
    this.panel.hidden = true;
    input.closest(".message")?.classList.add("has-supervisor-commands");
    input.parentElement!.insertBefore(this.panel, input);
    input.setAttribute("aria-controls", this.panel.id);
    input.addEventListener("input", () => {
      const intent = this.options.session.intent;
      if (intent && intent.phase !== "failed") return;
      this.options.session.cancel();
      this.mountAtComposer(); this.open = false; this.escaped = false; this.sync();
    }, { signal: this.events.signal });
    document.addEventListener("pointerdown", event => {
      const target = event.target as Node;
      if (this.anchor && !this.anchor.contains(target) && !this.visibleAnchor()?.contains(target) && !this.panel.contains(target)) this.close();
    }, { signal: this.events.signal });
    window.addEventListener("resize", () => this.positionDropdown(), { signal: this.events.signal });
    input.addEventListener("keydown", event => {
      if (event.isComposing) return;
      if (event.key === "Escape" && !this.panel.hidden) { event.preventDefault(); this.close(); }
      if ((event.key === "ArrowDown" || event.key === "ArrowUp") && !this.panel.hidden) {
        const listed = [...this.panel.querySelectorAll<HTMLButtonElement>("[data-supervisor-command]")];
        const target = event.key === "ArrowDown" ? listed[0] : listed.at(-1);
        if (target) { event.preventDefault(); target.focus(); }
      }
      if (event.key === "Enter" && !event.shiftKey && !event.altKey && !event.ctrlKey && !event.metaKey && this.submit()) {
        event.preventDefault();
      }
    }, { capture: true, signal: this.events.signal });
    this.panel.addEventListener("keydown", event => {
      if (event.key === "Escape") { event.preventDefault(); this.dismiss(); return; }
      // cas-c009: the confirmation's actions are one roving group; Tab and
      // every arrow cycle inside it while it is open.
      const actions = [...this.panel.querySelectorAll<HTMLButtonElement>('[role="alertdialog"] button:not(:disabled)')];
      const at = actions.indexOf(document.activeElement as HTMLButtonElement);
      if (at >= 0) {
        const step = event.key === "ArrowRight" || event.key === "ArrowDown" || (event.key === "Tab" && !event.shiftKey) ? 1
          : event.key === "ArrowLeft" || event.key === "ArrowUp" || (event.key === "Tab" && event.shiftKey) ? -1 : 0;
        if (step) { event.preventDefault(); actions[(at + step + actions.length) % actions.length]?.focus(); }
        return;
      }
      if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
      const buttons = [...this.panel.querySelectorAll<HTMLButtonElement>("[data-supervisor-command]")];
      const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
      if (index < 0) return;
      event.preventDefault(); buttons[(index + (event.key === "ArrowDown" ? 1 : buttons.length - 1)) % buttons.length]?.focus();
    }, { signal: this.events.signal });
    this.panel.addEventListener("focusin", event => {
      const intent = this.options.session.intent;
      if (intent?.phase === "confirming" && event.target instanceof HTMLButtonElement) {
        intent.focus = event.target.hasAttribute("data-command-confirm") ? "confirm" : "cancel";
      }
    }, { signal: this.events.signal });
    this.sync();
  }
  dispose(): void { this.anchor?.setAttribute("aria-expanded", "false"); this.events.abort(); this.panel.remove(); }
  show(anchor?: HTMLElement): void {
    if (anchor && this.anchor === anchor && !this.panel.hidden) { this.close(); return; }
    if (this.options.session.intent) return;
    if (anchor) this.mountAtAnchor(anchor); else this.mountAtComposer();
    this.open = true; this.escaped = false; this.sync(); this.positionDropdown();
    this.panel.querySelector<HTMLButtonElement>("button")?.focus();
  }
  close(): void { this.hide(); this.options.session.cancel(); }
  private hide(): void {
    this.open = false; this.escaped = true; this.panel.hidden = true; this.anchor?.setAttribute("aria-expanded", "false");
  }
  private dismiss(): void {
    const origin = this.anchor ? "header" : "composer";
    this.close(); this.returnFocus(origin);
  }
  returnFocus(origin: CommandOrigin): void {
    const target = origin === "header" ? this.visibleAnchor(document.getElementById("conversation-commands") ?? undefined) : undefined;
    (target?.getClientRects().length ? target : this.input).focus({ preventScroll: true });
  }
  private mountAtAnchor(anchor: HTMLElement): void {
    this.anchor = anchor;
    document.body.append(this.panel); this.panel.classList.add("supervisor-command-dropdown");
    anchor.setAttribute("aria-controls", this.panel.id); anchor.setAttribute("aria-expanded", "true");
  }
  private mountAtComposer(): void {
    this.anchor?.setAttribute("aria-expanded", "false"); this.anchor = undefined;
    this.panel.classList.remove("supervisor-command-dropdown"); this.panel.removeAttribute("style");
    this.input.parentElement!.insertBefore(this.panel, this.input);
  }
  /** An anchor inside a closed overflow menu stands in for its visible invoker (cas-e1db). */
  private visibleAnchor(anchor = this.anchor): HTMLElement | undefined {
    if (!anchor || anchor.getClientRects().length) return anchor;
    const menu = anchor.closest<HTMLElement>("[popover]");
    const invoker = menu?.id ? document.querySelector<HTMLElement>(`[popovertarget="${menu.id}"]`) : null;
    return invoker?.getClientRects().length ? invoker : anchor;
  }
  private positionDropdown(): void {
    const anchor = this.visibleAnchor();
    if (!anchor || this.panel.hidden) return;
    const edge = anchor.getBoundingClientRect();
    const gap = parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--space-2")) || 0;
    const width = this.panel.getBoundingClientRect().width;
    const top = edge.bottom + gap;
    this.panel.style.left = `${Math.max(gap, Math.min(edge.right - width, window.innerWidth - width - gap))}px`;
    this.panel.style.top = `${top}px`;
    this.panel.style.maxHeight = `${Math.max(0, window.innerHeight - top - gap)}px`;
  }
  reconcile(): void {
    const intent = this.options.session.intent;
    if (!intent) {
      if (this.panel.dataset.intentPhase) {
        delete this.panel.dataset.intentPhase;
        this.hide(); this.panel.replaceChildren(); this.mountAtComposer();
      }
      return;
    }
    const anchor = intent.origin === "header" ? document.getElementById("conversation-commands") : null;
    if (anchor && anchor !== this.anchor) this.mountAtAnchor(anchor);
    this.panel.hidden = false;
    if (this.panel.dataset.intentPhase !== intent.phase) {
      this.panel.dataset.intentPhase = intent.phase;
      this.panel.replaceChildren();
      const command = commandsForHarness(this.options.harness()).find(item => item.id === intent.command);
      if (intent.phase === "failed") this.notice(intent.message!);
      else if (command?.confirm) this.renderConfirmation(command, intent);
      else this.notice("Requesting command…");
    }
    this.positionDropdown();
  }
  sync(): void {
    this.reconcile();
    if (this.options.session.intent) return;
    const query = !this.anchor && this.input.value.startsWith("/") ? this.input.value.slice(1).toLowerCase() : "";
    const suggestion = this.input.value.startsWith("/") && commands.some(command => commandMatches(command.id, query));
    this.panel.hidden = this.escaped || (!this.open && !suggestion);
    if (this.panel.hidden) return;
    this.panel.replaceChildren();
    const heading = document.createElement("strong"); heading.textContent = `Commands · ${this.options.label()}`; this.panel.append(heading);
    const unavailable = this.options.unavailable();
    if (unavailable) { this.notice(unavailable); return; }
    const shown = commandsForHarness(this.options.harness()).filter(command => commandMatches(command.id, query));
    if (!shown.length) this.notice(COMMAND_ALIASES[query] === "mcp" || query === "mcp"
      ? `Reload Cassy is unavailable for ${this.options.harness() ?? "unknown"}. It requires Claude Code or Codex.`
      : "No supported command. Choose a listed command without arguments.");
    for (const command of shown) {
      const button = document.createElement("button"); button.type = "button"; button.dataset.supervisorCommand = command.id;
      const name = document.createElement("span"); name.textContent = `/${command.id} · ${command.label}`;
      const effect = document.createElement("small"); effect.textContent = command.effect;
      button.append(name, effect); button.onclick = () => this.choose(command); this.panel.append(button);
    }
  }
  /** Claim only exact supported command tokens; other text uses the normal send. */
  submit(): boolean {
    // The normal composer trims chat text; /mcp must stay semantic even
    // when pasted with surrounding whitespace, rather than open Claude's menu.
    const trimmed = this.input.value.trim();
    const text = trimmed === "/mcp" || trimmed === "/reload-cassy" ? trimmed : this.input.value;
    const token = text.split(/\s/, 1)[0];
    const command = commands.find(item => `/${item.id}` === token || (token.startsWith("/") && COMMAND_ALIASES[token.slice(1)] === item.id));
    if (!command) return false;
    const intent = this.options.session.intent;
    if (intent && intent.phase !== "failed") return true;
    if (intent) this.close();
    this.mountAtComposer(); this.escaped = false; this.open = true; this.sync();
    if (this.options.unavailable()) return true;
    const supported = commandsForHarness(this.options.harness()).find(item => item.id === command.id);
    if (text.trimEnd() === token && supported) this.choose(supported);
    return true;
  }
  private notice(text: string): void {
    const notice = document.createElement("p"); notice.setAttribute("role", "status"); notice.textContent = text; this.panel.append(notice);
  }
  private choose(command: typeof commands[number]): void {
    const unavailable = this.options.unavailable();
    if (unavailable) { this.panel.replaceChildren(); this.notice(unavailable); return; }
    const original = this.input.value.trim();
    const draft = !this.anchor && original.startsWith("/") && commandMatches(command.id, original.slice(1)) ? this.input.value : undefined;
    this.options.session.choose(command.id, !!command.confirm, this.anchor ? "header" : "composer", draft);
  }
  private renderConfirmation(command: typeof commands[number], intent: CommandIntent): void {
    const confirmation = document.createElement("div"); confirmation.setAttribute("role", "alertdialog");
    const subject = document.createElement("strong"); subject.textContent = this.options.label();
    subject.id = `${this.panel.id}-subject`; confirmation.setAttribute("aria-labelledby", subject.id);
    const consequence = document.createElement("p"); consequence.textContent = command.confirm ?? null;
    consequence.id = `${this.panel.id}-consequence`; confirmation.setAttribute("aria-describedby", consequence.id);
    // cas-c009: content-width actions, Cancel first and the command last.
    const actions = document.createElement("div"); actions.className = "supervisor-command-actions";
    const cancel = document.createElement("button"); cancel.type = "button"; cancel.className = "secondary"; cancel.textContent = "Cancel";
    cancel.onclick = () => this.dismiss();
    const confirm = document.createElement("button"); confirm.type = "button"; confirm.className = "primary"; confirm.dataset.commandConfirm = command.id;
    confirm.textContent = command.label; confirm.onclick = () => this.options.session.confirm();
    cancel.disabled = confirm.disabled = intent.phase === "sending";
    actions.append(cancel, confirm); confirmation.append(subject, consequence, actions); this.panel.append(confirmation);
    if (intent.phase === "confirming") (intent.focus === "confirm" ? confirm : cancel).focus({ preventScroll: true });
  }
}

/** A timed-out Usage may still cover the native prompt. The daemon rechecks
 * the view before sending Escape; recovery never resends the command. */
/** Reload Cassy's wait (cas-8c4f): three soft dots that breathe in turn; still under reduced motion. */
export function reloadWaitNode(doc: Document): HTMLElement {
  const wait = doc.createElement("span"); wait.className = "reload-wait"; wait.setAttribute("aria-hidden", "true");
  for (let dot = 0; dot < 3; dot += 1) wait.append(doc.createElement("i"));
  return wait;
}

function canCloseUnconfirmedUsage(receipt: CommandReceipt): boolean {
  return receipt.command === "usage" && receipt.phase === "unconfirmed";
}

export function commandReceiptNode(receipt: CommandReceipt, step?: (key: CommandKey) => void, dismiss?: () => void): HTMLElement {
  const row = document.createElement("article"); row.className = "supervisor-command-receipt"; row.dataset.phase = receipt.phase;
  if (!receipt.interactive && !receipt.output && !canCloseUnconfirmedUsage(receipt)) row.classList.add("command-status");
  const title = document.createElement("strong"); title.textContent = `/${receipt.command} · ${{ queued: "Queued", submitted: "Requested", observed: "Observed", refused: "Not sent", unconfirmed: "Unconfirmed", cancelled: "Cancelled" }[receipt.phase]}`;
  const detail = document.createElement("p"); detail.textContent = receipt.detail;
  row.append(title, detail);
  if (receipt.command === "mcp" && (receipt.phase === "queued" || receipt.phase === "submitted")) title.prepend(reloadWaitNode(row.ownerDocument));
  if (receipt.output) {
    const details = document.createElement("details"); const summary = document.createElement("summary"); summary.textContent = "Harness output";
    const output = document.createElement("pre"); output.textContent = receipt.output;
    details.append(summary, output); row.append(details);
  }
  if ((receipt.interactive || canCloseUnconfirmedUsage(receipt)) && step) {
    const actions = receipt.command === "model" ? ["previous", "next", "select", "cancel"] as const : ["cancel"] as const;
    for (const key of actions) {
      const button = document.createElement("button"); button.type = "button";
      button.textContent = ({ previous: "Previous", next: "Next", select: "Choose", cancel: "Close chooser" })[key];
      button.onclick = () => step(key); row.append(button);
    }
  }
  if (dismiss) {
    const button = document.createElement("button"); button.type = "button";
    button.textContent = "×"; button.setAttribute("aria-label", "Dismiss command status"); button.title = "Dismiss command status";
    button.onclick = dismiss; row.append(button);
  }
  return row;
}

/** Reconcile by command identity so a poll cannot erase disclosure or focus. */
export function updateCommandReceipts(parent: HTMLElement, receipts: readonly CommandReceipt[], step: (ref: string, key: CommandKey) => void, dismiss?: (ref: string) => void): void {
  const kept = new Set<HTMLElement>();
  for (const receipt of receipts) {
    const existing = [...parent.children].find(child => (child as HTMLElement).dataset.commandRef === receipt.client_ref) as HTMLElement | undefined;
    const signature = JSON.stringify(receipt);
    if (existing?.dataset.signature === signature) { kept.add(existing); continue; }
    const row = commandReceiptNode(receipt, key => step(receipt.client_ref, key), dismiss ? () => dismiss(receipt.client_ref) : undefined);
    row.dataset.commandRef = receipt.client_ref; row.dataset.signature = signature;
    const focused = existing?.contains(document.activeElement);
    const label = focused ? document.activeElement?.textContent : undefined;
    const open = existing?.querySelector("details")?.open;
    if (open && row.querySelector("details")) row.querySelector("details")!.open = true;
    if (existing) existing.replaceWith(row); else parent.append(row);
    if (focused) {
      const control = [...row.querySelectorAll<HTMLElement>("button, summary")].find(node => node.textContent === label);
      row.tabIndex = -1; (control ?? row).focus();
    }
    kept.add(row);
  }
  for (const child of [...parent.children]) if (!kept.has(child as HTMLElement)) child.remove();
}

interface SessionOptions {
  unavailable(): string | undefined;
  control(): Promise<void>;
  send(message: unknown): boolean;
  changed(): void;
  birth?(): string | undefined;
  clearDraft?(original: string): void;
  completed?(origin: CommandOrigin): void;
  /** A finished command's one-line outcome, shown once as a self-clearing toast. */
  settled?(receipt: CommandReceipt): void;
  /** The hub's retained receipt for a command whose daemon is gone (cas-b521);
   * "retry" while its daemon may still run or the store is busy. */
  readRetained?(ref: string): Promise<CommandReceipt | "retry" | undefined>;
}
/** Waits before re-reading a retained receipt the hub could not answer yet. */
const RETAINED_RETRY_MS = [1_000, 2_000, 4_000];
const COMMAND_OUTCOME_WINDOW_MS = 95_000;
// Finish can outlast the short confirmation window. Keep read-only recovery
// within the daemon's ten-minute receipt retention; never resubmit the command.
const FINISH_OUTCOME_WINDOW_MS = 10 * 60_000;
function awaitsCommandOutcome(receipt: CommandReceipt): boolean {
  return receipt.phase === "queued" || receipt.phase === "submitted"
    || receipt.command === "finish" && receipt.phase === "unconfirmed";
}
/** The daemon's prefix on an error it answers a supervisor command with (`CommanderControl::error_prefix`). */
export const COMMAND_ERROR_PREFIX = "supervisor command failed:";
/** A finished command needs no card: its one-line outcome clears itself (cas-95ea).
 * Usage stays, because its card carries the harness readout the operator asked for. */
function settlesToToast(receipt: CommandReceipt): boolean {
  return receipt.phase === "observed" && !receipt.interactive && receipt.command !== "usage";
}
/** One conversation owns its mutations and polls only for their outcomes. */
export class SupervisorCommandSession {
  private currentIntent?: CommandIntent;
  get intent(): CommandIntent | undefined {
    if (this.currentIntent && this.currentIntent.birth !== this.options.birth?.()) this.currentIntent = undefined;
    return this.currentIntent;
  }
  choose(command: SupervisorCommand, needsConfirmation: boolean, origin: CommandOrigin, draft?: string): void {
    if (this.intent) return;
    const intent: CommandIntent = { birth: this.options.birth?.(), origin, command, draft, phase: "confirming" };
    this.currentIntent = intent;
    if (needsConfirmation) this.options.changed();
    else void this.sendIntent(intent, false);
  }
  confirm(): void {
    const intent = this.intent;
    if (intent?.phase === "confirming") void this.sendIntent(intent, true);
  }
  cancel(): void {
    if (!this.currentIntent) return;
    this.currentIntent = undefined;
    this.options.changed();
  }
  private async sendIntent(intent: CommandIntent, confirmed: boolean): Promise<void> {
    intent.phase = "sending";
    this.options.changed();
    try {
      await this.execute(intent.command, confirmed, intent);
      if (this.intent !== intent) return;
      if (intent.draft !== undefined) this.options.clearDraft?.(intent.draft);
      this.cancel();
      this.options.completed?.(intent.origin);
    } catch (error) {
      if (this.intent !== intent) return;
      intent.phase = "failed";
      intent.message = error instanceof Error ? error.message : "The command could not be sent. Try again once connected.";
      this.options.changed();
    }
  }
  readonly receipts = new Map<string, CommandReceipt>();
  private readonly dismissed = new Set<string>();
  private readonly cancelling = new Set<string>();
  get visibleReceipts(): CommandReceipt[] { return [...this.receipts.values()].filter(receipt => !this.dismissed.has(receipt.client_ref)); }
  /** Release queued work or chooser tracking only after the daemon confirms. */
  async dismiss(ref: string): Promise<void> {
    const receipt = this.receipts.get(ref);
    if (!receipt || this.cancelling.has(ref)) return;
    if (receipt.phase === "queued" || receipt.interactive) {
      this.cancelling.add(ref);
      try {
        const reason = this.options.unavailable(); if (reason) throw new Error(reason);
        await this.options.control();
        const changed = this.options.unavailable(); if (changed) throw new Error(changed);
        if (!this.options.send({ SupervisorCommandCancel: { client_ref: ref } })) throw new Error("The cancellation was not sent. Check the connection.");
        this.poll(ref);
      } catch (error) { this.cancelling.delete(ref); throw error; }
      return;
    }
    this.dismissed.add(ref); this.options.changed();
  }
  supported = false;
  private readonly polls = new Map<string, ReturnType<typeof setTimeout>>();
  private readonly endedCommands = new Map<string, string>();
  /** A queued receipt can predate typing. Retain that uncertainty through
   * disconnect errors until a newer per-command submission/outcome arrives. */
  private readonly queuedCommands = new Set<string>();
  private readonly exitSentAt = new Map<string, number>();
  /** Commands the daemon has answered with a receipt, so a later error can't mean "never ran". */
  private readonly acknowledged = new Set<string>();
  /** How many refused attempts the one visible row for each command stands for. */
  private readonly refusals = new Map<SupervisorCommand, number>();
  private requesting = false;
  constructor(private readonly options: SessionOptions) {}
  run(command: SupervisorCommand, confirmed: boolean): Promise<void> {
    return this.execute(command, confirmed);
  }
  private async execute(command: SupervisorCommand, confirmed: boolean, intent?: CommandIntent): Promise<void> {
    const reason = this.options.unavailable(); if (reason) throw new Error(reason);
    const blocking = [...this.receipts.values()].find(receipt => receipt.phase === "queued" || receipt.phase === "submitted" || receipt.interactive);
    if (blocking) throw new Error(blocking.phase === "submitted"
      ? `/${blocking.command} has been submitted. Wait for its outcome; Dismiss cannot cancel a submitted command.`
      : `/${blocking.command} is pending. Use Dismiss command status on its card to cancel it, or Close chooser to close its native view.`);
    if (this.requesting) throw new Error("Wait for the current command request.");
    this.requesting = true;
    try { await this.request(command, confirmed, intent); } finally { this.requesting = false; }
  }
  private async request(command: SupervisorCommand, confirmed: boolean, intent?: CommandIntent): Promise<void> {
    await this.options.control();
    if (intent && this.intent !== intent) return;
    const changed = this.options.unavailable(); if (changed) throw new Error(changed);
    const ref = `cmd-${crypto.randomUUID()}`;
    this.receipts.set(ref, { client_ref: ref, command, phase: "submitted", detail: "Command requested; harness execution is not confirmed yet.", output: "", interactive: false });
    if (!this.options.send({ SupervisorCommand: { client_ref: ref, command, confirmed } })) {
      this.receipts.delete(ref); throw new Error("The command was not sent. Try again once connected.");
    }
    this.clearRefusals(command, false);
    if (command === "exit") this.exitSentAt.set(ref, Date.now());
    this.options.changed(); this.poll(ref);
  }
  receive(receipt: CommandReceipt): void {
    const current = this.receipts.get(receipt.client_ref);
    if (!current || current.command !== receipt.command) return;
    if (current.phase === "cancelled") return;
    if (current.phase === "refused" && receipt.phase !== "refused") return;
    if (receipt.phase === "queued") this.queuedCommands.add(receipt.client_ref);
    else if (receipt.phase === "submitted" || receipt.phase === "observed") this.queuedCommands.delete(receipt.client_ref);
    this.receipts.set(receipt.client_ref, receipt);
    this.acknowledged.add(receipt.client_ref);
    if (settlesToToast(receipt) && !this.dismissed.has(receipt.client_ref)) {
      this.dismissed.add(receipt.client_ref); this.options.settled?.(receipt);
    }
    if (this.cancelling.has(receipt.client_ref) && receipt.phase !== "queued" && receipt.phase !== "submitted" && !receipt.interactive) {
      this.cancelling.delete(receipt.client_ref); this.dismissed.add(receipt.client_ref);
    }
    if (receipt.phase !== "refused") this.clearRefusals(receipt.command);
    const ended = this.endedCommands.get(receipt.client_ref);
    if (ended !== undefined) this.settleEnded(receipt, ended);
    if (!awaitsCommandOutcome(receipt)) this.stopPoll(receipt.client_ref);
    this.options.changed();
  }
  /** Only the authenticated catalog's ended receipt may settle a vanished pane. */
  ended(reason: string): void {
    for (const receipt of this.receipts.values()) {
      if (!this.canSettleEnded(receipt)) continue;
      // A later, unrelated end is not evidence that an old Exit executed.
      if (receipt.command === "exit" && receipt.phase !== "queued") {
        const sentAt = this.exitSentAt.get(receipt.client_ref);
        if (sentAt === undefined || Date.now() - sentAt > COMMAND_OUTCOME_WINDOW_MS) continue;
      }
      this.endedCommands.set(receipt.client_ref, reason);
      this.settleEnded(receipt, reason);
      this.readRetained(receipt.client_ref);
    }
    this.options.changed();
  }
  private readonly retainedReads = new Set<string>();
  /** The daemon is gone; the receipt it left may still say how the command
   * ended. This only reads: nothing is ever resent. */
  private readRetained(ref: string, attempt = 0): void {
    const read = this.options.readRetained;
    if (!read || (attempt === 0 && this.retainedReads.has(ref))) return;
    this.retainedReads.add(ref);
    void read(ref).then(result => {
      if (result !== "retry") { if (result) this.receive(result); return; }
      const wait = RETAINED_RETRY_MS[attempt];
      if (wait !== undefined) setTimeout(() => this.readRetained(ref, attempt + 1), wait);
    }, () => undefined);
  }
  private canSettleEnded(receipt: CommandReceipt): boolean {
    return receipt.phase === "queued" || receipt.phase === "submitted" || receipt.phase === "unconfirmed" || receipt.interactive;
  }
  private settleEnded(receipt: CommandReceipt, reason: string): void {
    if (!this.canSettleEnded(receipt)) return;
    const queued = receipt.phase === "queued" || this.queuedCommands.has(receipt.client_ref);
    receipt.phase = !queued && receipt.command === "exit" ? "observed" : "unconfirmed";
    receipt.interactive = false;
    receipt.detail = queued ? `Session ended (${reason}); /${receipt.command} was last reported queued, so submission is unconfirmed.` : receipt.command === "exit"
      ? `Session ended after Exit (${reason}). See the session-ended receipt.`
      : `Session ended (${reason}); /${receipt.command} completion was not confirmed.`;
    this.stopPoll(receipt.client_ref);
  }
  reject(ref: string, detail: string, retryable = false): boolean {
    const receipt = this.receipts.get(ref); if (!receipt) return false;
    if (this.cancelling.delete(ref)) {
      // A failed Cancel does not settle the command. Keep reading its actual
      // outcome, including when it advanced before the cancellation arrived.
      receipt.detail = detail; this.poll(ref); this.options.changed(); return true;
    }
    if (receipt.phase === "observed" || receipt.phase === "refused" || receipt.phase === "cancelled") return true;
    // cas-6b21: a status read that met a reconnecting daemon link says nothing
    // about a command the daemon already holds (a Codex reload restarts the
    // supervisor meanwhile). Keep its own status and keep reading.
    if (retryable && this.acknowledged.has(ref) && (receipt.phase === "queued" || receipt.phase === "submitted")) return true;
    if (!this.acknowledged.has(ref) && detail.startsWith(COMMAND_ERROR_PREFIX)) {
      this.refuse(receipt, detail.slice(COMMAND_ERROR_PREFIX.length).trim());
      this.stopPoll(ref); this.options.changed(); return true;
    }
    if (receipt.phase !== "queued") receipt.phase = "unconfirmed";
    receipt.interactive = false; receipt.detail = detail;
    const ended = this.endedCommands.get(ref);
    if (ended !== undefined) this.settleEnded(receipt, ended);
    if (!awaitsCommandOutcome(receipt)) this.stopPoll(ref);
    this.options.changed(); return true;
  }
  /** The daemon wrote nothing: one row per command stands for every refused attempt. */
  private refuse(receipt: CommandReceipt, reason: string): void {
    const attempts = (this.refusals.get(receipt.command) ?? 0) + 1;
    this.clearRefusals(receipt.command);
    this.refusals.set(receipt.command, attempts);
    receipt.phase = "refused"; receipt.interactive = false;
    receipt.detail = attempts > 1 ? `${reason} Tried ${attempts} times.` : reason;
  }
  private clearRefusals(command: SupervisorCommand, resetAttempts = true): void {
    if (resetAttempts) this.refusals.delete(command);
    for (const [ref, receipt] of this.receipts) {
      if (receipt.command === command && receipt.phase === "refused") this.receipts.delete(ref);
    }
  }
  async step(ref: string, key: CommandKey): Promise<void> {
    const receipt = this.receipts.get(ref);
    if (!receipt || (!receipt.interactive && !(canCloseUnconfirmedUsage(receipt) && key === "cancel"))) return;
    const reason = this.options.unavailable(); if (reason) throw new Error(reason);
    await this.options.control();
    const changed = this.options.unavailable(); if (changed) throw new Error(changed);
    if (!this.options.send({ SupervisorCommandStep: { client_ref: ref, key } })) throw new Error("The selection was not sent. Check the connection.");
    receipt.phase = "submitted"; receipt.interactive = false; receipt.detail = "Selection requested; awaiting the harness's response.";
    this.options.changed(); this.poll(ref);
  }
  private stopPoll(ref: string): void { clearTimeout(this.polls.get(ref)); this.polls.delete(ref); }
  private poll(ref: string): void {
    this.stopPoll(ref);
    let started = Date.now();
    let queued = false;
    const read = () => {
      const receipt = this.receipts.get(ref);
      const cancelling = this.cancelling.has(ref);
      if (!receipt || (!cancelling && !awaitsCommandOutcome(receipt))) { this.stopPoll(ref); return; }
      if (receipt.phase === "queued" && !cancelling) queued = true;
      else if (queued) { started = Date.now(); queued = false; }
      const elapsed = Date.now() - started;
      if ((cancelling || receipt.phase !== "queued") && elapsed > COMMAND_OUTCOME_WINDOW_MS) {
        if (cancelling) { this.reject(ref, "Cancellation was not confirmed; reading the command outcome."); return; }
        if (receipt.phase !== "unconfirmed") this.reject(ref, "Harness execution was not observed. Check Raw output before running another command.");
        if (receipt.command !== "finish" || elapsed > FINISH_OUTCOME_WINDOW_MS) { this.stopPoll(ref); return; }
      }
      this.options.send({ SupervisorCommandStatus: { client_ref: ref } });
      this.polls.set(ref, setTimeout(read, receipt.phase === "unconfirmed" ? 5_000 : 1_000));
    };
    this.polls.set(ref, setTimeout(read, 1_000));
  }
}
