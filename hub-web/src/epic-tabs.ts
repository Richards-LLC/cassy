import { PARKED_EPIC_STATES, selectedEpicTab, type EpicTab } from "./progress-model";

const tabId = (key: string): string => `epic-tab-${encodeURIComponent(key)}`;
const plural = (count: number, one: string, many = `${one}s`): string => `${count} ${count === 1 ? one : many}`;

/**
 * Pure view of the pane project's epics in the context rail: a vertical tab
 * list, then the selected epic's work. Status reads, the project filter and
 * local selection belong to the conversation owner.
 */
export function renderEpicTabs(root: HTMLElement, tabs: readonly EpicTab[], options: {
  choice?: string; notice?: string; retry: () => void; select: (key: string) => void;
  /** Epics hidden by the pane-project scope, and whether they are shown anyway. */
  otherProjects: number; allProjects: boolean; toggleProjects: () => void;
  parkedOpen: boolean; toggleParked: () => void;
}): void {
  const document = root.ownerDocument;
  const selected = selectedEpicTab(tabs, options.choice);
  const signature = JSON.stringify([tabs, selected, options.notice, options.otherProjects, options.allProjects, options.parkedOpen]);
  if (root.dataset.signature === signature) return;
  root.dataset.signature = signature;
  const focused = root.contains(document.activeElement) ? (document.activeElement as HTMLElement)?.id : undefined;
  root.replaceChildren();
  root.hidden = !tabs.length && !options.otherProjects && !options.notice;
  const heading = document.createElement("h3"); heading.className = "epic-tabs-heading"; heading.textContent = "Epics"; root.append(heading);
  if (options.notice) {
    const notice = document.createElement("p"); notice.className = "epic-status-notice"; notice.setAttribute("role", "status"); notice.append(options.notice);
    const retry = document.createElement("button"); retry.type = "button"; retry.textContent = "Retry epic status"; retry.onclick = options.retry;
    notice.append(" ", retry); root.append(notice);
  }
  const nav = document.createElement("nav"); nav.className = "epic-tab-list";
  nav.setAttribute("role", "tablist"); nav.setAttribute("aria-orientation", "vertical"); nav.setAttribute("aria-label", "Live epics");
  if (tabs.length) root.append(nav);
  else {
    const empty = document.createElement("p"); empty.className = "epic-tabs-empty";
    empty.textContent = options.otherProjects ? "No live epics in this project." : "No live epics reported."; root.append(empty);
  }
  for (const [index, tab] of tabs.entries()) {
    const button = document.createElement("button"); button.type = "button"; button.setAttribute("role", "tab");
    button.id = tabId(tab.key); button.className = "epic-tab"; button.title = tab.epic.title || tab.title;
    button.setAttribute("aria-selected", String(tab.key === selected));
    button.setAttribute("aria-controls", "epic-tab-panel"); button.tabIndex = tab.key === selected ? 0 : -1;
    const name = document.createElement("span"); name.className = "epic-tab-name"; name.textContent = tab.title; button.append(name);
    const meta = document.createElement("span"); meta.className = "epic-tab-meta";
    // Unknown progress is left out, never drawn as a placeholder.
    const progress = tab.epic.progress;
    if (progress && Number.isSafeInteger(progress.merged) && Number.isSafeInteger(progress.total) && progress.merged >= 0 && progress.total >= progress.merged) {
      const count = document.createElement("span"); count.className = "epic-tab-count";
      count.textContent = `${progress.merged}/${progress.total} merged`; meta.append(count);
    }
    if (tab.workers.length) {
      const dots = document.createElement("span"); dots.className = "epic-worker-dots";
      dots.setAttribute("role", "img"); dots.setAttribute("aria-label", plural(tab.workers.length, "live worker"));
      for (const worker of tab.workers) {
        const dot = document.createElement("span"); dot.className = `epic-worker-dot${worker.state === "Waiting" ? " waiting" : ""}`;
        dot.title = `${worker.name} · ${worker.machine} · ${worker.state}`; dots.append(dot);
      }
      meta.append(dots);
    }
    const machine = document.createElement("small"); machine.className = "epic-tab-machine";
    machine.textContent = `${tab.machines.join(" · ")}${tab.stale ? " · Last known" : ""}`; meta.append(machine);
    button.append(meta);
    button.onclick = () => options.select(tab.key);
    button.onkeydown = event => {
      const offset = ["ArrowDown", "ArrowRight"].includes(event.key) ? 1 : ["ArrowUp", "ArrowLeft"].includes(event.key) ? -1 : 0;
      const next = event.key === "Home" ? 0 : event.key === "End" ? tabs.length - 1 : offset ? (index + offset + tabs.length) % tabs.length : undefined;
      if (next === undefined) return;
      event.preventDefault(); options.select(tabs[next].key);
      root.querySelector<HTMLElement>(`#${CSS.escape(tabId(tabs[next].key))}`)?.focus();
    };
    nav.append(button);
  }
  const tab = tabs.find(tab => tab.key === selected);
  if (tab) {
    const panel = document.createElement("section"); panel.id = "epic-tab-panel"; panel.className = "epic-tab-panel";
    panel.setAttribute("role", "tabpanel"); panel.setAttribute("aria-labelledby", tabId(tab.key)); panel.tabIndex = 0;
    const summary = document.createElement("p"); summary.className = "epic-panel-summary";
    summary.textContent = `${tab.inFlight} in flight · ${plural(tab.workers.length, "live worker")}${tab.stale ? " · Last known status" : ""}`; panel.append(summary);
    const parked = tab.rows.filter(row => PARKED_EPIC_STATES.includes(row.state));
    const shown = tab.rows.filter(row => !PARKED_EPIC_STATES.includes(row.state));
    if (!tab.rows.length) {
      const empty = document.createElement("p"); empty.className = "epic-tabs-empty"; empty.textContent = "No in-flight or ready tasks reported."; panel.append(empty);
    }
    const list = (rows: typeof tab.rows, id?: string): HTMLUListElement => {
      const ul = document.createElement("ul"); ul.className = "epic-task-list"; if (id) ul.id = id;
      for (const row of rows) {
        const li = document.createElement("li"); li.className = "epic-task";
        const title = document.createElement("span"); title.className = "epic-task-title"; title.textContent = row.task.title || row.task.id;
        const who = document.createElement("small"); who.className = "epic-task-who";
        const worker = document.createElement("span"); worker.className = "epic-task-worker"; worker.textContent = row.worker;
        const state = document.createElement("b"); state.className = `epic-task-state${row.state === "Blocked" ? " blocked" : ""}`; state.textContent = row.state;
        who.append(worker, ` · ${row.machine} · `, state);
        li.append(title, who); ul.append(li);
      }
      return ul;
    };
    if (shown.length) panel.append(list(shown));
    if (parked.length) {
      const toggle = document.createElement("button"); toggle.type = "button"; toggle.id = "epic-parked-toggle"; toggle.className = "epic-parked-toggle";
      toggle.setAttribute("aria-expanded", String(options.parkedOpen)); toggle.setAttribute("aria-controls", "epic-parked-list");
      const done = parked.some(row => row.state === "Merged") ? (parked.every(row => row.state === "Merged") ? "merged" : "parked or merged") : "parked";
      toggle.textContent = `${options.parkedOpen ? "Hide" : "Show"} ${parked.length} ${done}`;
      toggle.onclick = options.toggleParked;
      panel.append(toggle);
      if (options.parkedOpen) panel.append(list(parked, "epic-parked-list"));
    }
    root.append(panel);
  }
  if (options.otherProjects) {
    const other = document.createElement("button"); other.type = "button"; other.id = "epic-other-projects"; other.className = "epic-other-projects";
    other.textContent = options.allProjects ? "Show this project only" : `Show ${plural(options.otherProjects, "epic")} from other projects`;
    other.onclick = options.toggleProjects;
    root.append(other);
  }
  if (focused) root.querySelector<HTMLElement>(`#${CSS.escape(focused)}`)?.focus({ preventScroll: true });
}
