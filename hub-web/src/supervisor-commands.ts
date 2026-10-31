/** Fixed semantic controls; the hub and daemon independently enforce this list. */
export type SupervisorCommand = "compact" | "clear" | "model" | "usage" | "exit" | "finish";
export type CommandKey = "previous" | "next" | "select" | "cancel";
export interface CommandReceipt {
  client_ref: string;
  command: SupervisorCommand;
  phase: "submitted" | "observed" | "refused" | "unconfirmed";
  detail: string;
  output: string;
  interactive: boolean;
}
const commands: { id: SupervisorCommand; label: string; effect: string; confirm?: string }[] = [
  { id: "compact", label: "Compact", effect: "Summarize the supervisor's context" },
  { id: "usage", label: "Usage", effect: "Read usage from the supervisor's harness" },
  { id: "model", label: "Model", effect: "Open the harness's model chooser" },
  { id: "clear", label: "Clear", effect: "Start a fresh conversation context", confirm: "Clear this supervisor's conversation context? Existing messages remain in Commander." },
  { id: "finish", label: "Finish", effect: "Run the supervisor's Finish skill", confirm: "Run Finish? The supervisor will wrap up its work and may end this session." },
  { id: "exit", label: "Exit", effect: "End the supervisor's harness", confirm: "Exit this supervisor? Its harness will end. Check its handoff before leaving." },
];
export function commandsForHarness(harness: string | undefined): typeof commands {
  return harness === "claude" || harness === "codex" ? commands : [];
}
interface Options {
  harness(): string | undefined;
  unavailable(): string | undefined;
  label(): string;
  run(command: SupervisorCommand, confirmed: boolean): Promise<void>;
}

/** Owns only the mounted composer's local menu, never session or lease state. */
export class SlashCommands {
  private readonly panel: HTMLElement;
  private open = false;
  private escaped = false;
  private pending = false;
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
    input.addEventListener("input", () => { this.escaped = false; this.sync(); }, { signal: this.events.signal });
    input.addEventListener("keydown", event => {
      if (event.isComposing) return;
      if (event.key === "Escape" && !this.panel.hidden) { event.preventDefault(); this.close(); }
      if (event.key === "ArrowDown" && !this.panel.hidden) {
        event.preventDefault(); this.panel.querySelector<HTMLButtonElement>("[data-supervisor-command]")?.focus();
      }
      if (event.key === "Enter" && !event.shiftKey && !event.altKey && !event.ctrlKey && !event.metaKey && this.submit()) {
        event.preventDefault();
      }
    }, { capture: true, signal: this.events.signal });
    this.panel.addEventListener("keydown", event => {
      if (event.key === "Escape") { event.preventDefault(); this.close(); input.focus(); return; }
      if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
      const buttons = [...this.panel.querySelectorAll<HTMLButtonElement>("[data-supervisor-command]")];
      const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
      if (index < 0) return;
      event.preventDefault(); buttons[(index + (event.key === "ArrowDown" ? 1 : buttons.length - 1)) % buttons.length]?.focus();
    }, { signal: this.events.signal });
    this.sync();
  }
  dispose(): void { this.events.abort(); this.panel.remove(); }
  show(): void { this.open = true; this.escaped = false; this.sync(); this.panel.querySelector<HTMLButtonElement>("button")?.focus(); }
  close(): void { this.open = false; this.escaped = true; this.panel.hidden = true; }
  sync(): void {
    const query = this.input.value.startsWith("/") ? this.input.value.slice(1).toLowerCase() : "";
    const suggestion = this.input.value.startsWith("/") && commands.some(command => command.id.startsWith(query));
    this.panel.hidden = this.escaped || (!this.open && !suggestion);
    if (this.panel.hidden || this.pending) return;
    this.panel.replaceChildren();
    const heading = document.createElement("strong"); heading.textContent = `Commands · ${this.options.label()}`; this.panel.append(heading);
    const unavailable = this.options.unavailable();
    if (unavailable) { this.notice(unavailable); return; }
    const shown = commandsForHarness(this.options.harness()).filter(command => command.id.startsWith(query));
    if (!shown.length) this.notice("No supported command. Choose a listed command without arguments.");
    for (const command of shown) {
      const button = document.createElement("button"); button.type = "button"; button.dataset.supervisorCommand = command.id;
      const name = document.createElement("span"); name.textContent = `/${command.id} · ${command.label}`;
      const effect = document.createElement("small"); effect.textContent = command.effect;
      button.append(name, effect); button.onclick = () => this.choose(command); this.panel.append(button);
    }
  }
  /** Claim only exact supported command tokens; other text uses the normal send. */
  submit(): boolean {
    const token = this.input.value.split(/\s/, 1)[0];
    const command = commands.find(item => `/${item.id}` === token);
    if (!command) return false;
    this.escaped = false; this.open = true; this.sync();
    if (this.pending || this.options.unavailable()) return true;
    if (this.input.value.trimEnd() === token && commandsForHarness(this.options.harness()).includes(command)) this.choose(command);
    return true;
  }
  private notice(text: string): void {
    const notice = document.createElement("p"); notice.setAttribute("role", "status"); notice.textContent = text; this.panel.append(notice);
  }
  private choose(command: typeof commands[number]): void {
    if (this.pending) return;
    const unavailable = this.options.unavailable();
    if (unavailable) { this.panel.replaceChildren(); this.notice(unavailable); return; }
    if (!command.confirm) { void this.run(command.id, false); return; }
    this.panel.hidden = false; this.panel.replaceChildren();
    const confirmation = document.createElement("div"); confirmation.setAttribute("role", "alertdialog");
    confirmation.setAttribute("aria-label", `${command.label} · ${this.options.label()}`);
    const subject = document.createElement("strong"); subject.textContent = this.options.label();
    const consequence = document.createElement("p"); consequence.textContent = command.confirm;
    const cancel = document.createElement("button"); cancel.type = "button"; cancel.textContent = "Cancel";
    cancel.onclick = () => { this.close(); this.input.focus(); };
    const confirm = document.createElement("button"); confirm.type = "button"; confirm.dataset.commandConfirm = command.id;
    confirm.textContent = command.label; confirm.onclick = () => { void this.run(command.id, true); };
    confirmation.append(subject, consequence, cancel, confirm); this.panel.append(confirmation); cancel.focus();
  }
  private async run(command: SupervisorCommand, confirmed: boolean): Promise<void> {
    if (this.pending) return;
    this.pending = true;
    this.panel.querySelectorAll("button").forEach(button => { button.disabled = true; });
    const original = this.input.value;
    try {
      await this.options.run(command, confirmed);
      if (this.input.value === original && original.startsWith("/")) { this.input.value = ""; this.input.dispatchEvent(new Event("input", { bubbles: true })); }
      this.close(); this.input.focus();
    } catch (error) {
      this.panel.replaceChildren(); this.notice(error instanceof Error ? error.message : "The command could not be sent. Try again once connected.");
    } finally { this.pending = false; }
  }
}

export function commandReceiptNode(receipt: CommandReceipt, step?: (key: CommandKey) => void): HTMLElement {
  const row = document.createElement("article"); row.className = "supervisor-command-receipt"; row.dataset.phase = receipt.phase;
  const title = document.createElement("strong"); title.textContent = `/${receipt.command}`;
  const detail = document.createElement("p"); detail.textContent = receipt.detail;
  row.append(title, detail);
  if (receipt.output) {
    const details = document.createElement("details"); const summary = document.createElement("summary"); summary.textContent = "Harness output";
    const output = document.createElement("pre"); output.textContent = receipt.output;
    details.append(summary, output); row.append(details);
  }
  if (receipt.interactive && step) {
    const actions = receipt.command === "model" ? ["previous", "next", "select", "cancel"] as const : ["cancel"] as const;
    for (const key of actions) {
      const button = document.createElement("button"); button.type = "button";
      button.textContent = ({ previous: "Previous", next: "Next", select: "Choose", cancel: "Close chooser" })[key];
      button.onclick = () => step(key); row.append(button);
    }
  }
  return row;
}

/** Reconcile by command identity so a poll cannot erase disclosure or focus. */
export function updateCommandReceipts(parent: HTMLElement, receipts: readonly CommandReceipt[], step: (ref: string, key: CommandKey) => void): void {
  const kept = new Set<HTMLElement>();
  for (const receipt of receipts) {
    const existing = [...parent.children].find(child => (child as HTMLElement).dataset.commandRef === receipt.client_ref) as HTMLElement | undefined;
    const signature = JSON.stringify(receipt);
    if (existing?.dataset.signature === signature) { kept.add(existing); continue; }
    const row = commandReceiptNode(receipt, key => step(receipt.client_ref, key));
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
}
const COMMAND_OUTCOME_WINDOW_MS = 95_000;
/** The daemon's prefix on an error it answers a supervisor command with (`CommanderControl::error_prefix`). */
export const COMMAND_ERROR_PREFIX = "supervisor command failed:";
/** One conversation owns its mutations and polls only for their outcomes. */
export class SupervisorCommandSession {
  readonly receipts = new Map<string, CommandReceipt>();
  supported = false;
  private readonly polls = new Map<string, ReturnType<typeof setTimeout>>();
  private readonly endedCommands = new Map<string, string>();
  private readonly exitSentAt = new Map<string, number>();
  /** Commands the daemon has answered with a receipt, so a later error can't mean "never ran". */
  private readonly acknowledged = new Set<string>();
  /** How many refused attempts the one visible row for each command stands for. */
  private readonly refusals = new Map<SupervisorCommand, number>();
  constructor(private readonly options: SessionOptions) {}
  async run(command: SupervisorCommand, confirmed: boolean): Promise<void> {
    const reason = this.options.unavailable(); if (reason) throw new Error(reason);
    if ([...this.receipts.values()].some(receipt => receipt.phase === "submitted" || receipt.interactive)) throw new Error("Wait for the current command or close its chooser first.");
    await this.options.control();
    const changed = this.options.unavailable(); if (changed) throw new Error(changed);
    const ref = `cmd-${crypto.randomUUID()}`;
    this.receipts.set(ref, { client_ref: ref, command, phase: "submitted", detail: "Command requested; harness execution is not confirmed yet.", output: "", interactive: false });
    if (!this.options.send({ SupervisorCommand: { client_ref: ref, command, confirmed } })) {
      this.receipts.delete(ref); throw new Error("The command was not sent. Try again once connected.");
    }
    if (command === "exit") this.exitSentAt.set(ref, Date.now());
    this.options.changed(); this.poll(ref);
  }
  receive(receipt: CommandReceipt): void {
    const current = this.receipts.get(receipt.client_ref);
    if (!current || current.command !== receipt.command) return;
    if (current.phase === "refused" && receipt.phase !== "refused") return;
    this.receipts.set(receipt.client_ref, receipt);
    this.acknowledged.add(receipt.client_ref);
    if (receipt.phase !== "refused") this.clearRefusals(receipt.command);
    const ended = this.endedCommands.get(receipt.client_ref);
    if (ended !== undefined) this.settleEnded(receipt, ended);
    if (receipt.phase !== "submitted") this.stopPoll(receipt.client_ref);
    this.options.changed();
  }
  /** Only the authenticated catalog's ended receipt may settle a vanished pane. */
  ended(reason: string): void {
    for (const receipt of this.receipts.values()) {
      if (!this.canSettleEnded(receipt)) continue;
      // A later, unrelated end is not evidence that an old Exit executed.
      if (receipt.command === "exit") {
        const sentAt = this.exitSentAt.get(receipt.client_ref);
        if (sentAt === undefined || Date.now() - sentAt > COMMAND_OUTCOME_WINDOW_MS) continue;
      }
      this.endedCommands.set(receipt.client_ref, reason);
      this.settleEnded(receipt, reason);
    }
    this.options.changed();
  }
  private canSettleEnded(receipt: CommandReceipt): boolean {
    return receipt.phase === "submitted" || receipt.phase === "unconfirmed" || receipt.interactive;
  }
  private settleEnded(receipt: CommandReceipt, reason: string): void {
    if (!this.canSettleEnded(receipt)) return;
    receipt.phase = receipt.command === "exit" ? "observed" : "unconfirmed";
    receipt.interactive = false;
    receipt.detail = receipt.command === "exit"
      ? `Session ended after Exit (${reason}). See the session-ended receipt.`
      : `Session ended (${reason}); /${receipt.command} completion was not confirmed.`;
    this.stopPoll(receipt.client_ref);
  }
  reject(ref: string, detail: string): boolean {
    const receipt = this.receipts.get(ref); if (!receipt) return false;
    if (receipt.phase === "observed" || receipt.phase === "refused") return true;
    if (!this.acknowledged.has(ref) && detail.startsWith(COMMAND_ERROR_PREFIX)) {
      this.refuse(receipt, detail.slice(COMMAND_ERROR_PREFIX.length).trim());
      this.stopPoll(ref); this.options.changed(); return true;
    }
    receipt.phase = "unconfirmed"; receipt.interactive = false; receipt.detail = detail;
    const ended = this.endedCommands.get(ref);
    if (ended !== undefined) this.settleEnded(receipt, ended);
    this.stopPoll(ref); this.options.changed(); return true;
  }
  /** The daemon wrote nothing: one row per command stands for every refused attempt. */
  private refuse(receipt: CommandReceipt, reason: string): void {
    const attempts = (this.refusals.get(receipt.command) ?? 0) + 1;
    this.clearRefusals(receipt.command);
    this.refusals.set(receipt.command, attempts);
    receipt.phase = "refused"; receipt.interactive = false;
    receipt.detail = attempts > 1 ? `${reason} Tried ${attempts} times.` : reason;
  }
  private clearRefusals(command: SupervisorCommand): void {
    this.refusals.delete(command);
    for (const [ref, receipt] of this.receipts) {
      if (receipt.command === command && receipt.phase === "refused") this.receipts.delete(ref);
    }
  }
  async step(ref: string, key: CommandKey): Promise<void> {
    const receipt = this.receipts.get(ref); if (!receipt?.interactive) return;
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
    const started = Date.now();
    const read = () => {
      const receipt = this.receipts.get(ref);
      if (!receipt || receipt.phase !== "submitted") { this.stopPoll(ref); return; }
      if (Date.now() - started > COMMAND_OUTCOME_WINDOW_MS) {
        this.reject(ref, "Harness execution was not observed. Check Raw output before running another command."); return;
      }
      this.options.send({ SupervisorCommandStatus: { client_ref: ref } });
      this.polls.set(ref, setTimeout(read, 1_000));
    };
    this.polls.set(ref, setTimeout(read, 1_000));
  }
}
