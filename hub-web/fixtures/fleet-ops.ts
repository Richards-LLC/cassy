import { FleetOpsState, holdAction, stopAction, type FleetAgent, type FleetTask } from "../src/fleet-ops";
import { agentControls, headerControls, taskControls, undoBar, type FleetOpsViewContext } from "../src/fleet-ops-view";
import type { Scope } from "../src/types";
import { WriteGrantState } from "../src/write-grant";

/**
 * cas-a474: the rail's fleet controls, drawn by the real view in the
 * conversation's Tasks & progress column: an open ⋯ menu (fleet-ops-menu), the
 * inline Stop confirmation (fleet-ops-confirm), and the Undo offer after a
 * pause with the ask-to-merge preview open (fleet-ops-undo).
 */
const AGENTS: FleetAgent[] = [
  { name: "swift-lark-3", status: "active", current_task: "cas-1234", generation: 2 },
  { name: "quiet-owl-7", status: "idle", current_task: null, generation: 1 },
  { name: "brisk-wren-9", status: "active", current_task: "cas-1500", generation: 4 },
];
const TASKS: FleetTask[] = [
  { id: "cas-1234", title: "Docs lane", status: "in_progress", assignee: "swift-lark-3" },
  { id: "cas-2001", title: "Footer copy", status: "open", assignee: null, updated_at: "2026-10-03T11:20:00Z" },
  { id: "cas-1999", title: "Pairing wording", status: "awaiting_merge", tip: "9ffb3897", branch: "factory/wren-cas-1999" },
];
const SCOPES: Scope[] = ["machine-read", "session-read", "pane-read", "pane-input", "message-send", "pane-interrupt", "factory-operate"];

export function renderFleetOpsFixture(app: HTMLElement, name: string): void {
  const state = new FleetOpsState();
  if (name === "fleet-ops-menu") state.menuFor = "agent:swift-lark-3";
  if (name === "fleet-ops-confirm") state.confirm = { rowKey: "agent:brisk-wren-9", action: stopAction(AGENTS[2]!) };
  if (name === "fleet-ops-undo") {
    state.succeeded("agent:swift-lark-3", holdAction(AGENTS[0]!), Date.now());
    state.preview = { rowKey: "task:cas-1999", task: TASKS[2]! };
  }
  const noop = () => undefined;
  // cas-ab04: the Write access panel, open (fleet-ops-write-access) and at
  // its confirmation (fleet-ops-write-access-confirm).
  const grant = new WriteGrantState();
  grant.reset(TASKS);
  if (name === "fleet-ops-write-access-confirm") {
    grant.draft.path = "~/soundwave-config/docs/requests";
    grant.draft.reason = "INGEST request files";
    grant.review();
  }
  const writing = name.startsWith("fleet-ops-write-access");
  const context: FleetOpsViewContext = {
    state, scopes: name === "fleet-ops-confirm" || writing ? [...SCOPES, "factory-manage"] : SCOPES, origin: "https://commander.example", now: Date.now(),
    agents: AGENTS, tasks: TASKS, epics: ["cas-f29b", "cas-c4d3"], currentEpic: "cas-f29b", asked: new Map(), relative: () => "2m ago",
    on: { toggleMenu: noop, choose: noop, confirm: noop, cancelConfirm: noop, openPreview: noop, sendMerge: noop, closePanels: noop, toggleAssign: noop, toggleHeader: noop, undo: noop },
    writeAccess: { state: grant, tasks: TASKS, on: { changed: noop, review: noop, revoke: noop, confirm: noop, cancel: noop } },
  };
  const rail = document.createElement("aside");
  rail.className = "conversation-context";
  rail.style.cssText = "display:block;max-width:22rem;margin:0 auto;min-height:100dvh;box-sizing:border-box";
  const heading = document.createElement("h2"); heading.textContent = "Tasks & progress";
  const view = document.createElement("div"); view.id = "status-view";
  const undo = undoBar(document, context);
  if (undo) view.append(undo);
  view.append(headerControls(document, context, writing ? "grant" : undefined));
  const label = (text: string) => { const p = document.createElement("p"); p.className = "status-section-label"; p.textContent = text; return p; };
  const identifier = (text: string) => { const span = document.createElement("span"); span.className = "status-identifier"; span.textContent = text; return span; };
  const chip = (text: string) => { const span = document.createElement("span"); span.className = `status-chip status-chip--${text.replaceAll("_", "-")}`; span.textContent = text.replaceAll("_", " "); return span; };
  view.append(label(`Agents · ${AGENTS.length}`));
  for (const agent of AGENTS) {
    const row = document.createElement("article"); row.className = "status-row status-agent";
    const line = document.createElement("div"); line.className = "status-line";
    line.append(identifier(agent.name), chip(String(agent.status)));
    if (agent.current_task) line.append(identifier(agent.current_task));
    row.append(line, agentControls(document, context, agent));
    view.append(row);
  }
  view.append(label(`Tasks · ${TASKS.length}`));
  for (const task of TASKS) {
    const row = document.createElement("article"); row.className = "status-row status-task";
    const line = document.createElement("div"); line.className = "status-line";
    line.append(identifier(task.id), chip(String(task.status)));
    const title = document.createElement("p"); title.className = "status-task-title"; title.textContent = task.title ?? "";
    row.append(line, title);
    const controls = taskControls(document, context, task);
    if (controls) row.append(controls);
    view.append(row);
  }
  rail.append(heading, view);
  app.replaceChildren(rail);
}
