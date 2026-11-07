import { escapeHtml, projectTitle } from "./cloud-brand";
import { machineVersionMarkup, machineVersionSkew } from "./machine-version";
import { splitBar } from "./progress-view";
import { plainTextMarkdown } from "./markdown-renderer";
import { ConversationList, machineName, mostRecentConversation, rowProjectName, type ConversationRow } from "./conversation-list";

export interface NavigationMachine { id: string; label: string; online?: boolean; runtime?: string; installed?: string; latestRelease?: string }
type Filter = "all" | "needs-you" | `machine:${string}`;
type Open = (row: ConversationRow, event?: MouseEvent) => void;
type End = (row: ConversationRow) => Promise<void>;
interface Options { machines?: readonly NavigationMachine[]; searchActive?: boolean; reload?: (row: ConversationRow) => void }
interface Card { node: HTMLElement; heading: HTMLElement; toggle: HTMLButtonElement; chips: HTMLElement; chipMarkup?: string; summary: HTMLElement; summaryKey?: string; body: HTMLElement; rows: ConversationList }
const COLLAPSED_KEY = "commander.collapsed-projects";
const needsYou = (row: ConversationRow) => row.attention > 0 || row.needsYou === true;
const projectName = (row: ConversationRow) => rowProjectName(row) || row.supervisor;
/** The project a conversation belongs to, as the register groups it. Live
 * epics use the same key, so they never show another project's work. */
export const sessionProjectKey = (row: Pick<ConversationRow, "project" | "projectDir" | "key">): string => row.project?.trim() || projectTitle(row.projectDir) || row.key;
const projectKey = sessionProjectKey;

/**
 * cas-ade2: the register's order is alphabetical and never follows activity,
 * so a live update cannot move a card or a row. Cards sort by project name
 * (case-insensitive, then canonical id); rows by supervisor name, then
 * machine, then session.
 */
const collator = new Intl.Collator("en", { sensitivity: "base", numeric: true });
const byText = (a: string, b: string) => collator.compare(a, b) || (a < b ? -1 : a > b ? 1 : 0);
export function compareProjectCards(a: readonly ConversationRow[], b: readonly ConversationRow[]): number {
  return byText(projectName(a[0]!), projectName(b[0]!)) || byText(projectKey(a[0]!), projectKey(b[0]!));
}
export function compareSupervisorRows(a: ConversationRow, b: ConversationRow): number {
  return byText(a.supervisor, b.supervisor) || byText(machineName(a.host), machineName(b.host))
    || byText(a.machineId, b.machineId) || byText(a.session, b.session);
}

/** The project register composes the existing keyed row renderer, so pointer
 * intent, confirmation and focus behavior have one implementation. */
export class ProjectNavigation {
  // Row renderers own page-lifetime gesture listeners. Reuse each project's
  // renderer when a filter detaches and later restores its card.
  private cards = new Map<string, Card>();
  private shortcuts = new Map<string, HTMLButtonElement>();
  private pinned?: HTMLElement;
  private empty?: HTMLElement;
  private filter: Filter = "all";
  private collapsed = new Set<string>();
  private latest?: { container: HTMLElement; rows: readonly ConversationRow[]; open: Open; end?: End; options: Options };

  constructor() {
    try {
      const values: unknown = JSON.parse(localStorage.getItem(COLLAPSED_KEY) ?? "[]");
      if (Array.isArray(values)) this.collapsed = new Set(values.filter((value): value is string => typeof value === "string"));
    } catch { /* Storage may be unavailable; this device still works in memory. */ }

  }

  private save(): void {
    try { localStorage.setItem(COLLAPSED_KEY, JSON.stringify([...this.collapsed])); } catch { /* No storage requirement for navigation. */ }
  }

  setFilter(filter: Filter): void {
    this.filter = filter;
    const latest = this.latest;
    if (latest) this.render(latest.container, latest.rows, latest.open, latest.end, latest.options);
  }

  /** The footer's existing machine chooser uses the same filter state. */
  renderMachineFilters(container: HTMLElement, machines: readonly NavigationMachine[], chosen: () => void): void {
    container.hidden = machines.length === 0;
    this.filters(container, machines, chosen);
  }

  private filters(container: HTMLElement, machines: readonly NavigationMachine[], chosen?: () => void): void {
    container.setAttribute("role", "group");
    container.setAttribute("aria-label", "Filter conversations");
    const choices: [Filter, string][] = [["all", "All"], ...machines.map((machine): [Filter, string] => [`machine:${machine.id}`, machineName(machine.label)])];
    const kept = new Set<HTMLElement>();
    choices.forEach(([value, label], index) => {
      let button = [...container.querySelectorAll<HTMLButtonElement>("button")].find((node) => node.dataset.navFilter === value);
      if (!button) { button = container.ownerDocument.createElement("button"); button.type = "button"; button.dataset.navFilter = value; }
      button.className = "conversation-filter";
      if (button.textContent !== label) button.textContent = label;
      button.setAttribute("aria-pressed", String(this.filter === value));
      button.onclick = () => { this.setFilter(value); chosen?.(); };
      kept.add(button);
      if (container.children[index] !== button) container.insertBefore(button, container.children[index] ?? null);
    });
    for (const child of [...container.children]) if (!kept.has(child as HTMLElement)) child.remove();
  }

  /**
   * cas-da546: beside search, one Needs you toggle with its count, and the
   * machine a card's dot chose with a way back to every machine. The footer's
   * machine chooser keeps the full list.
   */
  private quickFilters(container: HTMLElement, machines: readonly NavigationMachine[], waiting: number): void {
    container.setAttribute("role", "group");
    container.setAttribute("aria-label", "Filter conversations");
    const machine = this.filter.startsWith("machine:") ? machines.find(item => `machine:${item.id}` === this.filter) : undefined;
    const choices: { value: Filter; label: string; markup: string; pressed: boolean; className: string }[] = [];
    if (waiting > 0 || this.filter === "needs-you") {
      const on = this.filter === "needs-you";
      choices.push({ value: on ? "all" : "needs-you", label: "Needs you", pressed: on, className: "conversation-filter needs-you-filter",
        markup: `Needs you${waiting > 0 ? ` <span class="needs-you-count">${waiting}</span>` : ""}` });
    }
    if (machine) choices.push({ value: "all", label: `Show every machine`, pressed: true, className: "conversation-filter machine-filter",
      markup: `${escapeHtml(machineName(machine.label))} <span aria-hidden="true">×</span>` });
    const kept = new Set<HTMLElement>();
    choices.forEach((choice, index) => {
      const key = choice.className.split(" ")[1]!;
      let button = [...container.querySelectorAll<HTMLButtonElement>("button")].find((node) => node.dataset.quickFilter === key);
      if (!button) { button = container.ownerDocument.createElement("button"); button.type = "button"; button.dataset.quickFilter = key; }
      button.className = choice.className;
      button.dataset.navFilter = choice.value;
      if (button.innerHTML !== choice.markup) button.innerHTML = choice.markup;
      if (key === "machine-filter") button.setAttribute("aria-label", `Showing ${machineName(machine!.label)}. ${choice.label}`); else button.removeAttribute("aria-label");
      button.setAttribute("aria-pressed", String(choice.pressed));
      button.onclick = () => this.setFilter(choice.value);
      kept.add(button);
      if (container.children[index] !== button) container.insertBefore(button, container.children[index] ?? null);
    });
    for (const child of [...container.children]) if (!kept.has(child as HTMLElement)) child.remove();
    container.hidden = choices.length === 0;
  }

  render(container: HTMLElement, rows: readonly ConversationRow[], open: Open, end?: End, options: Options = {}): void {
    this.latest = { container, rows, open, end, options };
    rows = rows.filter(row => !row.ended);
    const document = container.ownerDocument;
    const machines = options.machines ?? [...new Map(rows.map((row) => [row.machineId, { id: row.machineId, label: row.host, online: row.machine_online }])).values()];
    const filters = document.getElementById("conversation-filters");
    if (filters) this.quickFilters(filters, machines, rows.filter(needsYou).length);
    const matches = (row: ConversationRow) => this.filter === "all" || (this.filter === "needs-you" ? needsYou(row) : row.machineId === this.filter.slice(8));
    const groups = new Map<string, ConversationRow[]>();
    for (const row of rows) {
      const key = projectKey(row);
      groups.set(key, [...groups.get(key) ?? [], row]);
    }
    const ordered: HTMLElement[] = [];
    const decisions = rows.filter((row) => needsYou(row) && matches(row))
      .sort((a, b) => byText(projectName(a), projectName(b)) || compareSupervisorRows(a, b));
    if (decisions.length) {
      const section = this.pinned ??= document.createElement("section");
      section.className = "needs-you-section";
      section.setAttribute("aria-label", "Needs you");
      let heading = section.querySelector("h2");
      if (!heading) { heading = document.createElement("h2"); heading.className = "project-nav-eyebrow"; heading.textContent = "Needs you"; section.append(heading); }
      const current = new Set(decisions.map((row) => row.key));
      for (const [key, node] of this.shortcuts) if (!current.has(key)) { node.remove(); this.shortcuts.delete(key); }
      decisions.forEach((row, index) => {
        let button = this.shortcuts.get(row.key);
        if (!button) { button = document.createElement("button"); button.type = "button"; button.className = "needs-you-shortcut"; this.shortcuts.set(row.key, button); }
        // cas-da546: who is asking, where, how long ago, then the question itself.
        const count = row.attention > 0 ? `<span class="needs-you-badge" aria-label="${row.attention} waiting">${row.attention}</span>` : "";
        const when = row.when ? `<span class="needs-you-when">${escapeHtml(row.when)}</span>` : "";
        const question = row.operatorQuestion ? `<q class="needs-you-question">${escapeHtml(plainTextMarkdown(row.operatorQuestion))}</q>` : `<small>${escapeHtml(row.operatorReason || "Waiting for your answer")}</small>`;
        const markup = `<span class="needs-you-line"><strong>${escapeHtml(projectName(row))}</strong><span class="needs-you-who"><span class="codename">${escapeHtml(row.supervisor)}</span> on ${escapeHtml(machineName(row.host))}</span>${when}${count}</span>${question}`;
        if (button.innerHTML !== markup) button.innerHTML = markup;
        button.onclick = (event) => { this.collapsed.delete(projectKey(row)); this.save(); open(row, event); };
        if (section.children[index + 1] !== button) section.insertBefore(button, section.children[index + 1] ?? null);
      });
      ordered.push(section);
    } else this.pinned?.remove();

    for (const [key, members] of [...groups].sort(([, a], [, b]) => compareProjectCards(a, b))) {
      const visible = members.filter((row) => matches(row));
      if (!visible.length) continue;
      let card = this.cards.get(key);
      if (!card) {
        const node = document.createElement("section"); node.className = "conversation-group project-card"; node.setAttribute("role", "group");
        // The spoken group name and visual header share the same facts. Keep
        // the complete machine names in the spoken heading when chips wrap.
        const heading = document.createElement("p"); heading.className = "conversation-group-head sr-only"; heading.setAttribute("aria-hidden", "true");
        const toggle = document.createElement("button"); toggle.type = "button"; toggle.className = "project-card-toggle";
        const header = document.createElement("div"); header.className = "project-card-head";
        const chips = document.createElement("div"); chips.className = "project-machine-chips";
        header.append(toggle, chips);
        const summary = document.createElement("p"); summary.className = "project-card-summary";
        const body = document.createElement("div"); body.className = "project-card-rows"; body.id = `project-rows-${encodeURIComponent(key)}`;
        toggle.setAttribute("aria-controls", body.id);
        node.append(heading, header, summary, body);
        card = { node, heading, toggle, chips, summary, body, rows: new ConversationList() };
        this.cards.set(key, card);
      }
      const name = projectName(members[0]!);
      const hosts = [...new Map(members.map((row) => [row.machineId, row])).values()];
      const label = `${name} · ${visible.length} ${visible.length === 1 ? "conversation" : "conversations"} on ${hosts.map((row) => machineName(row.host)).join(", ")}`;
      card.node.setAttribute("aria-label", label);
      if (card.heading.textContent !== label) card.heading.textContent = label;
      const identity = members[0]!.project?.trim();
      const markup = `<strong class="project-card-name"${identity && identity !== name ? ` title="${escapeHtml(identity)}"` : ""}>${escapeHtml(name)}</strong><span class="sr-only"> ${visible.length} ${visible.length === 1 ? "conversation" : "conversations"}</span><span class="project-card-chevron" aria-hidden="true">⌄</span>`;
      if (card.toggle.innerHTML !== markup) card.toggle.innerHTML = markup;
      card.body.hidden = this.collapsed.has(key) && !options.searchActive;
      card.toggle.setAttribute("aria-expanded", String(!card.body.hidden));
      const current = card;
      card.toggle.onclick = () => {
        current.body.hidden = !current.body.hidden;
        if (current.body.hidden) this.collapsed.add(key); else this.collapsed.delete(key);
        current.toggle.setAttribute("aria-expanded", String(!current.body.hidden));
        this.save();
      };
      const chipMarkup = hosts.map((row) => {
        const machine = options.machines?.find(machine => machine.id === row.machineId);
        const online = machine?.online ?? row.machine_online;
        return `<button type="button" class="project-machine-chip" data-project-machine="${escapeHtml(row.machineId)}" aria-label="Filter conversations on ${escapeHtml(machineName(row.host))}${online === undefined ? "" : online ? ", online" : ", offline"}"><span class="machine-online-dot${online === true ? " online" : online === false ? " offline" : ""}" aria-hidden="true"></span>${escapeHtml(machineName(row.host))}<span class="sr-only">${online === undefined ? "" : online ? ", online" : ", offline"}</span>${machineVersionSkew(machine?.runtime, machine?.latestRelease, machine?.installed) ? machineVersionMarkup(machine?.runtime, machine?.latestRelease, undefined, machine?.installed) : ""}</button>`;
      }).join("");
      // The live DOM also contains aria-description added below. Compare the
      // generated input so an unchanged catalog tick keeps the focused chip.
      if (card.chipMarkup !== chipMarkup) {
        card.chips.innerHTML = chipMarkup;
        card.chipMarkup = chipMarkup;
      }
      for (const chip of card.chips.querySelectorAll<HTMLButtonElement>("button")) {
        const version = chip.querySelector(".machine-version")?.textContent;
        if (version) chip.setAttribute("aria-description", version); else chip.removeAttribute("aria-description");
        chip.onclick = () => this.setFilter(`machine:${chip.dataset.projectMachine!}`);
      }
      // cas-da546: "train 1/5 · 14 workers · 0 blocked". Each fact shows only
      // when a status or the catalog reported it; a quiet project says idle.
      const live = members.filter((row) => !row.ended && row.machine_online === true && row.workerCount !== undefined);
      const workerCount = live.reduce((sum, row) => sum + row.workerCount!, 0);
      const lines = [...members].sort((a, b) => Number(b.selected) - Number(a.selected) || compareSupervisorRows(a, b)).flatMap(row => row.statusLine ? [row.statusLine] : []);
      const train = lines.find(line => line.train)?.train;
      const blocked = lines.length ? Math.max(...lines.map(line => line.blocked)) : undefined;
      const quiet = !members.some(row => needsYou(row) || row.supervisorState === "working") && !train?.in_progress;
      const parts = [train ? `train ${train.merged}/${train.total}` : quiet ? "idle" : "",
        live.length ? workerCount ? `${workerCount} ${workerCount === 1 ? "worker" : "workers"}` : "no workers" : ""].filter(Boolean);
      const summaryKey = JSON.stringify([train, parts, blocked]);
      if (card.summaryKey !== summaryKey) {
        card.summaryKey = summaryKey;
        const text = document.createElement("span"); text.className = "project-card-status";
        text.textContent = parts.join(" · ");
        if (blocked !== undefined) {
          const count = document.createElement("span"); count.className = blocked ? "project-card-blocked" : "";
          count.textContent = `${blocked} blocked`;
          text.append(parts.length ? " · " : "", count);
        }
        card.summary.replaceChildren(...(train ? [splitBar(document, train, `${name} train`)] : []), text);
      }
      card.summary.hidden = !card.summary.textContent && !train;
      card.node.classList.toggle("is-quiet", quiet);
      const sorted = [...visible].sort(compareSupervisorRows);
      // The most recent session is marked where it stands; it never moves.
      const newest = mostRecentConversation(sorted);
      // The card supplies the heading; the row primitive supplies the row,
      // marks and controls. No second group wrapper is needed inside it.
      const shared = new Set(sorted.filter((row, index) => sorted.findIndex(other => other.machineId === row.machineId) !== index).map(row => row.machineId));
      const nested = sorted.map((row) => ({ ...row, showCodename: shared.has(row.machineId), supervisorState: needsYou(row) ? "needs-you" as const : row.supervisorState ?? (row.machine_online === false ? "offline" as const : "idle" as const), group: { key, label, size: sorted.length, first: false, active: sorted.length > 1 && row === newest } }));
      card.rows.render(card.body, nested, open, end, options.reload);
      ordered.push(card.node);
    }
    if (!ordered.length && rows.length) {
      const empty = this.empty ??= document.createElement("p"); empty.className = "project-nav-empty";
      empty.textContent = "No conversations match this filter. Choose All to see your projects.";
      ordered.push(empty);
    }
    const kept = new Set(ordered);
    for (const child of [...container.children]) if (!kept.has(child as HTMLElement)) child.remove();
    ordered.forEach((node, index) => { if (container.children[index] !== node) container.insertBefore(node, container.children[index] ?? null); });
  }
}
