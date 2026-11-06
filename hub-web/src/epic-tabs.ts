import { selectedEpicTab, type EpicTab } from "./progress-model";

const tabId = (key: string): string => `epic-tab-${encodeURIComponent(key)}`;

/** Pure view: status reads and local selection belong to the conversation owner. */
export function renderEpicTabs(root: HTMLElement, tabs: readonly EpicTab[], options: {
  choice?: string; phone: boolean; sheetOpen: boolean;
  select: (key: string) => void; close: () => void; notice?: string; retry: () => void;
}): void {
  const document = root.ownerDocument;
  const selected = selectedEpicTab(tabs, options.choice);
  const signature = JSON.stringify([tabs, selected, options.phone, options.sheetOpen, options.notice]);
  if (root.dataset.signature === signature) return;
  root.dataset.signature = signature;
  const focused = root.contains(document.activeElement) ? (document.activeElement as HTMLElement)?.id : undefined;
  // Close before replacement so native dialog focus restoration never targets a detached node.
  root.querySelector<HTMLDialogElement>("dialog[open]")?.close();
  root.replaceChildren();
  const nav = document.createElement("nav"); nav.className = "epic-tab-strip";
  nav.setAttribute("role", "tablist"); nav.setAttribute("aria-label", "Live epics across machines");
  root.append(nav);
  if (options.notice) {
    const notice = document.createElement("p"); notice.className = "epic-status-notice"; notice.setAttribute("role", "status"); notice.append(options.notice);
    const retry = document.createElement("button"); retry.type = "button"; retry.textContent = "Retry epic status"; retry.onclick = options.retry;
    notice.append(" ", retry); root.append(notice);
  }
  if (!tabs.length) {
    const empty = document.createElement("p"); empty.className = "epic-tabs-empty";
    empty.textContent = "No live epics reported."; root.append(empty); return;
  }
  for (const [index, tab] of tabs.entries()) {
    const button = document.createElement("button"); button.type = "button"; button.setAttribute("role", "tab");
    button.id = tabId(tab.key); button.className = "epic-tab";
    button.setAttribute("aria-selected", String(tab.key === selected));
    button.setAttribute("aria-controls", "epic-tab-panel"); button.tabIndex = tab.key === selected ? 0 : -1;
    const name = document.createElement("span"); name.className = "epic-tab-name";
    name.textContent = (tab.epic.title || tab.epic.id).split(":")[0]; button.title = tab.epic.title || tab.epic.id; button.append(name);
    const progress = tab.epic.progress;
    const valid = progress && Number.isSafeInteger(progress.merged) && Number.isSafeInteger(progress.total) && progress.merged >= 0 && progress.total >= progress.merged;
    const count = document.createElement("span"); count.className = "epic-tab-count";
    count.textContent = valid ? `${progress.merged}/${progress.total}` : "—/—";
    count.setAttribute("aria-label", valid ? `${progress.merged} of ${progress.total} merged` : "Merge progress not reported"); button.append(count);
    const dots = document.createElement("span"); dots.className = "epic-worker-dots";
    dots.setAttribute("role", "img"); dots.setAttribute("aria-label", `${tab.workers.length} live ${tab.workers.length === 1 ? "worker" : "workers"}`);
    for (const worker of tab.workers) {
      const dot = document.createElement("span"); dot.className = `epic-worker-dot${worker.state === "Waiting" ? " waiting" : ""}`;
      dot.title = `${worker.name} · ${worker.machine} · ${worker.state}`; dots.append(dot);
    }
    button.append(dots);
    const machine = document.createElement("small"); machine.className = "epic-tab-machine";
    machine.textContent = `${tab.machines.join(" · ")}${tab.stale ? " · Last known" : ""}`; button.append(machine);
    button.onclick = () => options.select(tab.key);
    button.onkeydown = event => {
      const offset = event.key === "ArrowRight" ? 1 : event.key === "ArrowLeft" ? -1 : 0;
      const next = event.key === "Home" ? 0 : event.key === "End" ? tabs.length - 1 : offset ? (index + offset + tabs.length) % tabs.length : undefined;
      if (next === undefined) return;
      event.preventDefault(); options.select(tabs[next].key);
      root.querySelector<HTMLElement>(`#${CSS.escape(tabId(tabs[next].key))}`)?.focus();
    };
    nav.append(button);
  }
  const tab = tabs.find(tab => tab.key === selected)!;
  const panel = document.createElement("section"); panel.id = "epic-tab-panel"; panel.className = "epic-tab-panel";
  panel.setAttribute("role", "tabpanel"); panel.setAttribute("aria-labelledby", tabId(tab.key)); panel.tabIndex = 0;
  const heading = document.createElement("h2"); heading.textContent = tab.epic.title || tab.epic.id; panel.append(heading);
  const summary = document.createElement("p"); summary.className = "epic-panel-summary";
  summary.textContent = `${tab.inFlight} in flight · ${tab.workers.length} live workers${tab.stale ? " · Last known status" : ""}`; panel.append(summary);
  if (!tab.rows.length) {
    const empty = document.createElement("p"); empty.textContent = "No in-flight or ready tasks reported."; panel.append(empty);
  } else {
    const table = document.createElement("table"); table.className = "epic-task-table";
    const caption = table.createCaption(); caption.textContent = "Workers, tasks and delivery state"; caption.className = "sr-only";
    const header = table.createTHead().insertRow();
    for (const label of ["Worker", "Task", "State"]) { const cell = document.createElement("th"); cell.scope = "col"; cell.textContent = label; header.append(cell); }
    const body = table.createTBody();
    for (const row of tab.rows) {
      const tr = body.insertRow();
      const worker = tr.insertCell(); worker.className = "epic-task-worker"; worker.append(row.worker);
      const machine = document.createElement("small"); machine.textContent = row.machine; worker.append(machine);
      tr.insertCell().textContent = row.task.title || row.task.id;
      const state = tr.insertCell(); state.className = "epic-task-state"; state.textContent = row.state;
    }
    panel.append(table);
  }
  if (options.phone) {
    const sheet = document.createElement("dialog"); sheet.className = "epic-sheet";
    sheet.setAttribute("aria-label", `${tab.epic.title || tab.epic.id} tasks`);
    const close = document.createElement("button"); close.type = "button"; close.className = "epic-sheet-close";
    close.textContent = "Close epic tasks"; close.onclick = options.close;
    sheet.addEventListener("cancel", event => { event.preventDefault(); options.close(); });
    sheet.append(close, panel); root.append(sheet);
    if (options.sheetOpen) sheet.showModal();
  } else root.append(panel);
  if (focused) root.querySelector<HTMLElement>(`#${CSS.escape(focused)}`)?.focus({ preventScroll: true });
}
