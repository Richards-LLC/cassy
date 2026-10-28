import { escapeHtml, projectTitle } from "./cloud-brand";
import { ConversationList, machineName, type ConversationRow } from "./conversation-list";

export interface NavigationMachine { id: string; label: string; online?: boolean }
type Filter = "all" | "needs-you" | `machine:${string}`;
type Open = (row: ConversationRow, event?: MouseEvent) => void;
type End = (row: ConversationRow) => Promise<void>;
interface Options { machines?: readonly NavigationMachine[]; searchActive?: boolean }
interface Card { node: HTMLElement; heading: HTMLElement; toggle: HTMLButtonElement; chips: HTMLElement; summary: HTMLElement; body: HTMLElement; rows: ConversationList }
const COLLAPSED_KEY = "commander.collapsed-projects";
const needsYou = (row: ConversationRow) => row.attention > 0 || row.needsYou === true;
const projectName = (row: ConversationRow) => row.project?.trim() || projectTitle(row.projectDir) || row.supervisor;
const projectKey = (row: ConversationRow) => row.project?.trim() || projectTitle(row.projectDir) || row.key;

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
    this.filters(container, machines, chosen, false);
  }

  private filters(container: HTMLElement, machines: readonly NavigationMachine[], chosen?: () => void, includeNeeds = true): void {
    container.setAttribute("role", "group");
    container.setAttribute("aria-label", "Filter conversations");
    const choices: [Filter, string][] = [["all", "All"], ...(includeNeeds ? [["needs-you", "Needs you"] as [Filter, string]] : []), ...machines.map((machine): [Filter, string] => [`machine:${machine.id}`, machineName(machine.label)])];
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

  render(container: HTMLElement, rows: readonly ConversationRow[], open: Open, end?: End, options: Options = {}): void {
    this.latest = { container, rows, open, end, options };
    const document = container.ownerDocument;
    const machines = options.machines ?? [...new Map(rows.map((row) => [row.machineId, { id: row.machineId, label: row.host, online: row.machine_online }])).values()];
    const filters = document.getElementById("conversation-filters");
    if (filters) this.filters(filters, machines);
    const matches = (row: ConversationRow) => this.filter === "all" || (this.filter === "needs-you" ? needsYou(row) : row.machineId === this.filter.slice(8));
    const groups = new Map<string, ConversationRow[]>();
    for (const row of rows) {
      const key = projectKey(row);
      groups.set(key, [...groups.get(key) ?? [], row]);
    }
    const ordered: HTMLElement[] = [];
    const decisions = rows.filter((row) => needsYou(row) && matches(row));
    if (decisions.length) {
      const section = this.pinned ??= document.createElement("section");
      section.className = "needs-you-section";
      section.setAttribute("aria-label", "Needs you");
      let heading = section.querySelector("h2");
      if (!heading) { heading = document.createElement("h2"); heading.textContent = "Needs you"; section.append(heading); }
      const current = new Set(decisions.map((row) => row.key));
      for (const [key, node] of this.shortcuts) if (!current.has(key)) { node.remove(); this.shortcuts.delete(key); }
      decisions.forEach((row, index) => {
        let button = this.shortcuts.get(row.key);
        if (!button) { button = document.createElement("button"); button.type = "button"; button.className = "needs-you-shortcut"; this.shortcuts.set(row.key, button); }
        const markup = `<strong>${escapeHtml(projectName(row))}</strong><span class="codename">${escapeHtml(row.supervisor)}</span><small>${escapeHtml(row.operatorReason || "Waiting for your answer")} · ${escapeHtml(machineName(row.host))}</small>`;
        if (button.innerHTML !== markup) button.innerHTML = markup;
        button.onclick = (event) => { this.collapsed.delete(projectKey(row)); this.save(); open(row, event); };
        if (section.children[index + 1] !== button) section.insertBefore(button, section.children[index + 1] ?? null);
      });
      ordered.push(section);
    } else this.pinned?.remove();

    for (const [key, members] of groups) {
      const visible = members.filter(matches);
      if (!visible.length) continue;
      let card = this.cards.get(key);
      if (!card) {
        const node = document.createElement("section"); node.className = "conversation-group project-card"; node.setAttribute("role", "group");
        // The spoken group name and visual header share the same facts. Keep
        // the complete machine names in the spoken heading when chips wrap.
        const heading = document.createElement("p"); heading.className = "conversation-group-head sr-only"; heading.setAttribute("aria-hidden", "true");
        const toggle = document.createElement("button"); toggle.type = "button"; toggle.className = "project-card-toggle";
        const chips = document.createElement("div"); chips.className = "project-machine-chips";
        const summary = document.createElement("p"); summary.className = "project-card-summary";
        const body = document.createElement("div"); body.className = "project-card-rows"; body.id = `project-rows-${encodeURIComponent(key)}`;
        toggle.setAttribute("aria-controls", body.id);
        node.append(heading, toggle, chips, summary, body);
        card = { node, heading, toggle, chips, summary, body, rows: new ConversationList() };
        this.cards.set(key, card);
      }
      const name = projectName(members[0]!);
      const hosts = [...new Map(members.map((row) => [row.machineId, row])).values()];
      const label = `${name} · ${visible.length} ${visible.length === 1 ? "conversation" : "conversations"} on ${hosts.map((row) => machineName(row.host)).join(", ")}`;
      card.node.setAttribute("aria-label", label);
      if (card.heading.textContent !== label) card.heading.textContent = label;
      const markup = `<strong class="project-card-name">${escapeHtml(name)}</strong><span class="project-card-count">${visible.length}<span class="sr-only"> ${visible.length === 1 ? "conversation" : "conversations"}</span></span><span class="project-card-chevron" aria-hidden="true">⌄</span>`;
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
      const chipMarkup = hosts.map((row) => `<button type="button" class="project-machine-chip" data-project-machine="${escapeHtml(row.machineId)}" aria-label="Filter conversations on ${escapeHtml(machineName(row.host))}${row.machine_online === undefined ? "" : row.machine_online ? ", online" : ", offline"}"><span class="machine-online-dot${row.machine_online === true ? " online" : row.machine_online === false ? " offline" : ""}" aria-hidden="true"></span>${escapeHtml(machineName(row.host))}<span class="sr-only">${row.machine_online === undefined ? "" : row.machine_online ? ", online" : ", offline"}</span></button>`).join("");
      if (card.chips.innerHTML !== chipMarkup) card.chips.innerHTML = chipMarkup;
      for (const chip of card.chips.querySelectorAll<HTMLButtonElement>("button")) chip.onclick = () => this.setFilter(`machine:${chip.dataset.projectMachine!}`);
      const live = members.filter((row) => row.machine_online === true && row.workerCount !== undefined);
      const workerCount = live.reduce((sum, row) => sum + row.workerCount!, 0);
      const rate = members.find((row) => row.mergesPerHour !== undefined)?.mergesPerHour;
      const summary = [live.length ? `${workerCount} live ${workerCount === 1 ? "worker" : "workers"}` : "", rate === undefined ? "" : `${rate} merges/h`].filter(Boolean).join(" · ");
      if (card.summary.textContent !== summary) card.summary.textContent = summary;
      card.summary.hidden = !summary;
      const anyActivity = visible.some((row) => row.activityAt !== undefined);
      const rank = (row: ConversationRow) => (anyActivity ? row.activityAt : row.startedAt) ?? -Infinity;
      const sorted = [...visible].sort((a, b) => rank(b) - rank(a));
      // The card supplies the heading; the row primitive supplies the row,
      // marks and controls. No second group wrapper is needed inside it.
      const nested = sorted.map((row, index) => ({ ...row, supervisorState: needsYou(row) ? "needs-you" as const : row.supervisorState ?? (row.machine_online === false ? "offline" as const : "idle" as const), group: { key, label, size: sorted.length, first: false, active: sorted.length > 1 && index === 0 && rank(row) !== -Infinity } }));
      card.rows.render(card.body, nested, open, end);
      ordered.push(card.node);
    }
    if (!ordered.length && rows.length) {
      const empty = this.empty ??= document.createElement("p"); empty.className = "project-nav-empty"; empty.textContent = "No conversations match this filter. Choose All to see your projects."; ordered.push(empty);
    }
    const kept = new Set(ordered);
    for (const child of [...container.children]) if (!kept.has(child as HTMLElement)) child.remove();
    ordered.forEach((node, index) => { if (container.children[index] !== node) container.insertBefore(node, container.children[index] ?? null); });
  }
}
