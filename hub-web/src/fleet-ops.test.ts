// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import {
  FleetOpsState,
  UNDO_WINDOW_MS,
  agentMenu,
  assignAction,
  focusEpicAction,
  holdAction,
  idleWorkers,
  mergeRequestMessage,
  requestMergeAction,
  spawnAction,
  staleMessage,
  stopAction,
  type FleetAgent,
  type FleetTask,
} from "./fleet-ops";
import { phoneFleetNotice, agentControls, headerControls, taskControls, undoBar, type FleetOpsHandlers, type FleetOpsViewContext } from "./fleet-ops-view";
import type { Scope } from "./types";

const ORIGIN = "https://commander.example";
const CONTROL: Scope[] = ["machine-read", "session-read", "pane-read", "pane-input", "message-send", "pane-interrupt"];
const lark: FleetAgent = { name: "swift-lark-3", status: "active", current_task: "cas-1234", generation: 2 };
const owl: FleetAgent = { name: "quiet-owl-7", status: "idle", current_task: null, generation: 1 };
const ready: FleetTask = { id: "cas-2001", title: "Docs lane", status: "open", assignee: null, updated_at: "2026-10-03T12:00:00Z" };
const parked: FleetTask = { id: "cas-1999", title: "Footer copy", status: "awaiting_merge", tip: "9ffb3897", branch: "factory/owl-cas-1999" };

describe("fleet actions send the brief's operations with their preconditions (cas-a474)", () => {
  it("pauses and resumes a worker, each the other's Undo, guarded by its generation", () => {
    const pause = holdAction(lark);
    expect(pause.request).toEqual({ op: { kind: "set_worker_hold", worker: "swift-lark-3", hold: true }, expected: { worker: "swift-lark-3", generation: 2 } });
    expect(pause).toMatchObject({ label: "Pause", destructive: false, done: "swift-lark-3 paused." });
    const resume = pause.inverse!();
    expect(resume.request.op).toEqual({ kind: "set_worker_hold", worker: "swift-lark-3", hold: false });
    expect(resume.label).toBe("Resume");
    expect(holdAction({ ...lark, status: "held" }).label).toBe("Resume");
  });

  it("lists Pause, Restart…, Stop… with the destructive ones confirmed and never undone", () => {
    const [pause, restart, stop] = agentMenu(lark);
    expect([pause!.label, restart!.label, stop!.label]).toEqual(["Pause", "Restart…", "Stop…"]);
    expect(restart).toMatchObject({ destructive: true, operation: "restart-worker" });
    expect(restart!.inverse).toBeUndefined();
    expect(stop!.question).toBe("Stop swift-lark-3? Its task cas-1234 goes back to Open, for any worker to pick up.");
    expect(stop!.request).toEqual({ op: { kind: "shutdown_workers", workers: ["swift-lark-3"] }, expected: { worker: "swift-lark-3", generation: 2 } });
    expect(stopAction(lark, true).request.op).toMatchObject({ force: true });
  });

  it("assigns a ready task to an idle worker, guarded by its updated_at and assignee, undone by unassigning", () => {
    expect(idleWorkers([lark, owl, { name: "sup", role: "supervisor" }]).map((agent) => agent.name)).toEqual(["quiet-owl-7"]);
    const assign = assignAction(ready, "quiet-owl-7");
    expect(assign.request).toEqual({ op: { kind: "assign_task", task_id: "cas-2001", assignee: "quiet-owl-7" }, expected: { updated_at: "2026-10-03T12:00:00Z", assignee: null } });
    expect(assign.inverse!().request).toEqual({ op: { kind: "assign_task", task_id: "cas-2001", assignee: null }, expected: { updated_at: null, assignee: "quiet-owl-7" } });
  });

  it("focuses an epic with Undo back to the previous one, and adds 1-4 workers optionally on a ready task", () => {
    expect(focusEpicAction("cas-f29b", "cas-c4d3").request).toEqual({ op: { kind: "focus_epic", epic_id: "cas-f29b" }, expected: { epic_id: "cas-c4d3" } });
    expect(focusEpicAction("cas-f29b", "cas-c4d3").inverse!().request.op).toEqual({ kind: "focus_epic", epic_id: "cas-c4d3" });
    // With no focus before, Undo clears it (focus_epic clear=true, cas-566b).
    expect(focusEpicAction("cas-f29b", null).inverse!().request).toEqual({ op: { kind: "focus_epic", clear: true }, expected: { epic_id: "cas-f29b" } });
    expect(spawnAction(9, "cas-2001").request.op).toEqual({ kind: "spawn_workers", count: 4, task_id: "cas-2001" });
    expect(spawnAction(1).inverse).toBeUndefined();
  });

  it("shows the exact merge message before it is sent, generated from the task", () => {
    // The hub's own template (ops::fleet::request_merge_text), so the preview is what is sent.
    expect(mergeRequestMessage(parked)).toBe("Operator request from Commander: please merge cas-1999 (Footer copy).\nBranch: factory/owl-cas-1999\nTip: 9ffb3897\nIt is awaiting merge. Merge it into its epic, or reply with what blocks it.");
    expect(mergeRequestMessage({ id: "cas-1", title: "T" })).toContain("Branch: <not recorded>\nTip: <not recorded>");
    expect(requestMergeAction(parked).request).toEqual({ op: { kind: "request_merge", task_id: "cas-1999" }, expected: { status: "awaiting_merge", tip: "9ffb3897" } });
  });

  it("says what changed when the hub answers stale", () => {
    expect(staleMessage(stopAction(lark), { generation: 3 })).toBe("swift-lark-3 already restarted.");
    expect(staleMessage(stopAction(lark), { exists: false })).toBe("swift-lark-3 is already gone.");
    // The hub's own shape (ops::fleet::check_worker_generation): a null generation is a worker gone.
    expect(staleMessage(stopAction(lark), { worker: "swift-lark-3", generation: null })).toBe("swift-lark-3 is already gone.");
    expect(staleMessage(stopAction(lark), { worker: "swift-lark-3", generation: "agent-77" })).toBe("swift-lark-3 already restarted.");
    expect(staleMessage(holdAction(lark), { held: true })).toBe("swift-lark-3 is already paused.");
    expect(staleMessage(assignAction(ready, "quiet-owl-7"), { assignee: "brisk-wren-9" })).toBe("cas-2001 is already assigned to brisk-wren-9.");
    expect(staleMessage(requestMergeAction(parked), { status: "closed" })).toBe("cas-1999 is closed.");
  });
});

describe("menu, confirm and undo state (cas-a474)", () => {
  it("opens one menu at a time; a destructive choice confirms first, the confirm runs once, Cancel runs nothing", () => {
    const state = new FleetOpsState();
    state.toggleMenu("agent:swift-lark-3");
    expect(state.menuFor).toBe("agent:swift-lark-3");
    state.toggleMenu("agent:quiet-owl-7");
    expect(state.menuFor).toBe("agent:quiet-owl-7");
    expect(state.choose("agent:swift-lark-3", stopAction(lark))).toBeUndefined();
    expect(state.menuFor).toBeUndefined();
    expect(state.confirm?.action.id).toBe("stop:swift-lark-3");
    state.cancelConfirm();
    expect(state.confirmed()).toBeUndefined();
    state.choose("agent:swift-lark-3", stopAction(lark));
    expect(state.confirmed()?.id).toBe("stop:swift-lark-3");
    expect(state.confirmed()).toBeUndefined();
    // A reversible choice runs at once.
    expect(state.choose("agent:swift-lark-3", holdAction(lark))?.id).toBe("pause:swift-lark-3");
  });

  it("uses the hub's outcome.inverse for Undo when it sends one (cas-31f0)", () => {
    const state = new FleetOpsState();
    const assign = assignAction(ready, "quiet-owl-7");
    state.succeeded("task:cas-2001", assign, 0, { kind: "assign_task", inverse: { op: { kind: "assign_task", task_id: "cas-2001", assignee: null }, expected: { assignee: "quiet-owl-7", updated_at: "2026-10-03T12:05:00Z" } } });
    expect(state.currentUndo(1)?.action.request).toEqual({ op: { kind: "assign_task", task_id: "cas-2001", assignee: null }, expected: { assignee: "quiet-owl-7", updated_at: "2026-10-03T12:05:00Z" } });
    expect(state.currentUndo(1)?.action.label).toBe("Unassign");
  });

  it("offers Undo for 8 s after a reversible action, none after a destructive or additive one, and announces each step", () => {
    const state = new FleetOpsState();
    const pause = holdAction(lark);
    state.started("agent:swift-lark-3", pause);
    expect(state.announcement).toBe("Pausing swift-lark-3…");
    expect(state.pending.has("agent:swift-lark-3")).toBe(true);
    state.succeeded("agent:swift-lark-3", pause, 1_000);
    expect(state.announcement).toBe("swift-lark-3 paused.");
    expect(state.currentUndo(1_000 + UNDO_WINDOW_MS - 1)?.action.label).toBe("Resume");
    expect(state.currentUndo(1_000 + UNDO_WINDOW_MS)).toBeUndefined();
    state.succeeded("agent:swift-lark-3", pause, 2_000);
    const undone = state.takeUndo(2_500)!;
    expect(undone.request.op).toMatchObject({ hold: false });
    expect(state.takeUndo(2_600)).toBeUndefined();
    // cas-97d58 F12: the Undo itself succeeds with no Undo of its own.
    state.succeeded("agent:swift-lark-3", undone, 2_700);
    expect(state.announcement).toBe("swift-lark-3 resumed.");
    expect(state.currentUndo(2_700)).toBeUndefined();
    state.succeeded("agent:swift-lark-3", stopAction(lark), 3_000);
    expect(state.currentUndo(3_000)).toBeUndefined();
    // cas-97d58 F12: a destructive result stays visible for the Undo window instead.
    expect(state.currentResult(3_000)?.label).toBe("swift-lark-3 stopped.");
    expect(state.currentResult(3_000 + UNDO_WINDOW_MS)).toBeUndefined();
    state.succeeded("header", spawnAction(2), 3_000);
    expect(state.currentUndo(3_000)).toBeUndefined();
  });

  it("keeps a failure on its row, in words: stale says what changed, an error says what the hub said", () => {
    const state = new FleetOpsState();
    state.failed("agent:swift-lark-3", stopAction(lark), { stale: true, current: { generation: 3 } });
    expect(state.notes.get("agent:swift-lark-3")).toEqual({ text: "swift-lark-3 already restarted.", tone: "stale" });
    expect(state.announcement).toBe("swift-lark-3 already restarted.");
    state.failed("agent:quiet-owl-7", holdAction(owl), { detail: "audit log unavailable" });
    expect(state.notes.get("agent:quiet-owl-7")?.text).toBe("Could not pause: audit log unavailable.");
  });

  it.each([
    ["Audit log unavailable. No worker was paused.", "Audit log unavailable. No worker was paused."],
    ["Audit log unavailable", "Audit log unavailable."],
    ["Try again!", "Try again!"],
    ["Is the machine available?", "Is the machine available?"],
    ["Machine unavailable…", "Machine unavailable…"],
    ["No worker was paused. \n", "No worker was paused. \n"],
  ])("preserves failure detail punctuation (cas-5203): %s", (detail, expected) => {
    const state = new FleetOpsState();
    const row = "agent:swift-lark-3", pause = holdAction(lark);
    state.started(row, pause);
    state.failed(row, pause, { detail });
    expect(state.notes.get(row)).toEqual({ text: `Could not pause: ${expected}`, tone: "error" });
    expect(state.announcement).toBe(`Could not pause: ${expected}`);
    expect(state.pending.has(row)).toBe(false);
    expect(state.currentUndo(0)).toBeUndefined();
    expect(lark.status).toBe("active");
  });
});

function context(scopes: Scope[], state = new FleetOpsState(), on: Partial<FleetOpsHandlers> = {}): FleetOpsViewContext {
  const handlers: FleetOpsHandlers = {
    toggleMenu: vi.fn(), choose: vi.fn(), confirm: vi.fn(), cancelConfirm: vi.fn(), openPreview: vi.fn(), sendMerge: vi.fn(),
    closePanels: vi.fn(), toggleAssign: vi.fn(), toggleHeader: vi.fn(), undo: vi.fn(), ...on,
  };
  return { state, scopes, origin: ORIGIN, now: 0, agents: [lark, owl], tasks: [ready, parked], epics: ["cas-f29b", "cas-c4d3"], currentEpic: "cas-c4d3", asked: new Map(), relative: () => "2m ago", on: handlers };
}

describe("the rail's controls draw that state (cas-a474)", () => {
  it("gates each menu item: a control pairing can pause but sees Stop disabled with the command that adds factory:manage", () => {
    const state = new FleetOpsState();
    state.menuFor = "agent:swift-lark-3";
    const view = agentControls(document, context([...CONTROL, "factory-operate"], state), lark);
    const trigger = view.querySelector<HTMLButtonElement>(".fleet-ops-trigger")!;
    expect(trigger.getAttribute("aria-haspopup")).toBe("menu");
    expect(trigger.getAttribute("aria-expanded")).toBe("true");
    expect(trigger.getAttribute("aria-label")).toBe("Actions for swift-lark-3");
    const items = [...view.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')];
    expect(items.map((item) => item.textContent)).toEqual(["Pause", "Restart…", "Stop…"]);
    expect(items[0]!.hasAttribute("aria-disabled")).toBe(false);
    const stop = items[2]!;
    expect(stop.getAttribute("aria-disabled")).toBe("true");
    expect(view.querySelector(`#${stop.getAttribute("aria-describedby")!}`)?.textContent).toBe("Restart and Stop: Not allowed on this pairing. Needs the Stop and restart workers and sessions permission. Add it in Paired machines.");
    // A separator sets the destructive items apart.
    expect(view.querySelector('[role="separator"]')).not.toBeNull();
  });

  it("draws the inline confirmation with Cancel first and ignores a double-click's second press", () => {
    const state = new FleetOpsState();
    state.confirm = { rowKey: "agent:swift-lark-3", action: stopAction(lark) };
    const confirm = vi.fn();
    const view = agentControls(document, context([...CONTROL, "factory-manage"], state, { confirm }), lark);
    const buttons = [...view.querySelectorAll<HTMLButtonElement>(".fleet-ops-confirm button")];
    expect(buttons.map((button) => button.textContent)).toEqual(["Cancel", "Stop"]);
    expect(buttons[1]!.classList.contains("danger")).toBe(true);
    expect(view.querySelector(".fleet-ops-question")?.textContent).toBe("Stop swift-lark-3? Its task cas-1234 goes back to Open, for any worker to pick up.");
    buttons[1]!.dispatchEvent(new MouseEvent("click", { detail: 2 }));
    expect(confirm).not.toHaveBeenCalled();
    buttons[1]!.dispatchEvent(new MouseEvent("click", { detail: 1 }));
    expect(confirm).toHaveBeenCalledOnce();
  });

  it("shows the exact message before Ask supervisor to merge sends it, and 'Asked 2m ago' after", () => {
    const state = new FleetOpsState();
    state.preview = { rowKey: "task:cas-1999", task: parked };
    const view = taskControls(document, context(CONTROL, state), parked)!;
    expect(view.querySelector(".fleet-ops-preview-text")?.textContent).toBe(mergeRequestMessage(parked));
    expect([...view.querySelectorAll(".fleet-ops-preview button")].map((button) => button.textContent)).toEqual(["Cancel", "Send"]);
    const asked = taskControls(document, { ...context(CONTROL), asked: new Map([["cas-1999", 1]]) }, parked)!;
    expect(asked.querySelector(".fleet-ops-asked")?.textContent).toBe("Asked 2m ago");
    expect(asked.querySelector(".fleet-ops-ask")?.textContent).toBe("Ask again");
  });

  it("offers Assign… on a ready task with the idle workers, and the header's Add worker… and Focus epic…", () => {
    const state = new FleetOpsState();
    state.assignFor = "task:cas-2001";
    const view = taskControls(document, context([...CONTROL, "factory-operate"], state), ready)!;
    expect([...view.querySelectorAll('[role="menuitem"]')].map((item) => item.textContent)).toEqual(["Assign to quiet-owl-7"]);
    const header = headerControls(document, context([...CONTROL, "factory-operate"]), "focus");
    expect([...header.querySelectorAll('[role="menuitem"]')].map((item) => item.textContent)).toEqual(["Focus cas-f29b"]);
    const readOnly = headerControls(document, context(["machine-read", "session-read", "pane-read"]), undefined);
    expect(readOnly.querySelector(".fleet-ops-add")?.getAttribute("aria-disabled")).toBe("true");
    // cas-97d58 F06: by what it allows, never the scope id.
    expect(readOnly.querySelector(".fleet-ops-reason")?.textContent).toContain("Needs the Manage workers and tasks permission.");
    expect(readOnly.querySelector(".fleet-ops-reason")?.textContent).not.toMatch(/factory:/);
  });

  it("draws the Undo offer while it lasts", () => {
    const state = new FleetOpsState();
    state.succeeded("agent:swift-lark-3", holdAction(lark), 0);
    const undo = vi.fn();
    const bar = undoBar(document, context(CONTROL, state, { undo }))!;
    expect(bar.textContent).toBe("swift-lark-3 paused.Undo");
    bar.querySelector<HTMLButtonElement>("button")!.click();
    expect(undo).toHaveBeenCalledOnce();
    expect(undoBar(document, { ...context(CONTROL, state), now: UNDO_WINDOW_MS + 1 })).toBeUndefined();
  });
});


describe("S6 phone controls", () => {
  it("has one task trigger, gates its action, and opens Assign rather than sending anything", () => {
    const state = new FleetOpsState(); state.menuFor = "task:cas-2001";
    const toggleAssign = vi.fn(), choose = vi.fn();
    const allowed = taskControls(document, { ...context([...CONTROL, "factory-operate"], state, { toggleAssign, choose }), phone: true }, ready)!;
    expect(allowed.querySelectorAll(":scope > button")).toHaveLength(1);
    expect(allowed.querySelector("button")?.getAttribute("aria-label")).toBe("Actions for cas-2001");
    allowed.querySelector<HTMLButtonElement>('[role="menuitem"]')!.click();
    expect(toggleAssign).toHaveBeenCalledWith("task:cas-2001"); expect(choose).not.toHaveBeenCalled();
    const denied = taskControls(document, { ...context(CONTROL, state, { toggleAssign }), phone: true }, ready)!;
    denied.querySelector<HTMLButtonElement>('[role="menuitem"]')!.click();
    expect(toggleAssign).toHaveBeenCalledTimes(1);
    expect(denied.querySelector('[role="menuitem"]')?.getAttribute("aria-disabled")).toBe("true");
  });

  it("searches idle workers locally, recovers from no match and keeps the query across redraw", () => {
    const state = new FleetOpsState(); state.assignFor = "task:cas-2001";
    const ctx = { ...context([...CONTROL, "factory-operate"], state), phone: true };
    const view = taskControls(document, ctx, ready)!;
    const search = view.querySelector<HTMLInputElement>('input[type="search"]')!;
    search.value = "absent"; search.dispatchEvent(new Event("input"));
    expect(view.querySelector<HTMLElement>('[role="menuitem"]')?.hidden).toBe(true);
    expect(view.querySelector<HTMLElement>(".fleet-ops-no-match")?.hidden).toBe(false);
    expect(taskControls(document, ctx, ready)!.querySelector<HTMLInputElement>("input")?.value).toBe("absent");
    search.value = "quiet"; search.dispatchEvent(new Event("input"));
    expect(view.querySelector<HTMLElement>('[role="menuitem"]')?.hidden).toBe(false);
    expect(ctx.on.choose).not.toHaveBeenCalled();
    state.closeMenus(); expect(state.pickerQuery).toBe("");
  });

  it("uses a searchable epic picker and keeps the merge preview behind one task trigger", () => {
    const ctx = { ...context([...CONTROL, "factory-operate"]), phone: true };
    expect(headerControls(document, ctx, "focus").querySelector("input")?.getAttribute("aria-label")).toBe("Search focus epic");
    ctx.state.preview = { rowKey: "task:cas-1999", task: parked };
    const preview = taskControls(document, ctx, parked)!;
    expect(preview.querySelectorAll(":scope > button")).toHaveLength(1);
    expect(preview.querySelector(".fleet-ops-preview-text")?.textContent).toBe(mergeRequestMessage(parked));
  });
});


it("keeps an Undo refusal readable after the phone progress sheet closes", () => {
  const state = new FleetOpsState(); const action = holdAction(lark, false);
  state.started("agent:swift-lark-3", action);
  const pending = phoneFleetNotice(document, context(CONTROL, state))!;
  expect(pending.querySelector("span")?.textContent).toBe(action.progress);
  expect(pending.dataset.fleetFocus).toBe("agent:swift-lark-3:progress");
  state.failed("agent:swift-lark-3", action, { stale: true, current: { worker: lark.name, generation: 3 } });
  const refused = phoneFleetNotice(document, context(CONTROL, state))!;
  expect(refused.textContent).toContain("already restarted");
  expect(refused.dataset.fleetFocus).toBe("agent:swift-lark-3:note");
  expect(undoBar(document, context(CONTROL, state))).toBeUndefined();
});

describe("phone feedback dismissal (cas-c2e7)", () => {
  it("dismisses the same failure across redraws, retains its row and allows a later failure", () => {
    const state = new FleetOpsState(); const action = stopAction(lark);
    state.failed("agent:swift-lark-3", action, { stale: true, current: { generation: 3 } });
    const note = state.notes.get("agent:swift-lark-3"); const announcement = state.announcement;
    const dismissNotice = vi.fn();
    const ctx = { ...context(CONTROL, state, { dismissNotice }), phone: true };
    const bar = phoneFleetNotice(document, ctx)!;
    const dismiss = bar.querySelector<HTMLButtonElement>('[aria-label="Dismiss fleet notice"]')!;
    expect(dismiss.tabIndex).toBe(0); dismiss.click();
    expect(dismissNotice).toHaveBeenCalledOnce();
    expect(phoneFleetNotice(document, ctx)).toBeUndefined();
    expect(state.notes.get("agent:swift-lark-3")).toBe(note);
    expect(state.announcement).toBe(announcement);
    state.failed("agent:swift-lark-3", action, { stale: true, current: { generation: 3 } });
    expect(phoneFleetNotice(document, ctx)).toBeDefined();
  });
  it("dismisses pending feedback without losing its owner, then shows completion and keeps desktop Undo", () => {
    const state = new FleetOpsState(); state.select("atlas/session"); const action = holdAction(lark);
    const owner = state.started("agent:swift-lark-3", action);
    const ctx = { ...context(CONTROL, state), phone: true };
    phoneFleetNotice(document, ctx)!.querySelector<HTMLButtonElement>("button")!.click();
    expect(state.owns(owner)).toBe(true);
    expect(state.pending.get("agent:swift-lark-3")).toBe(action);
    expect(phoneFleetNotice(document, ctx)).toBeUndefined();
    state.succeeded("agent:swift-lark-3", action, 0);
    const result = undoBar(document, ctx)!;
    result.querySelector<HTMLButtonElement>('[aria-label="Dismiss; Undo stays in Tasks & progress"]')!.click();
    expect(undoBar(document, ctx)).toBeUndefined();
    expect(state.currentUndo(0)).toBeDefined();
    expect(undoBar(document, { ...ctx, phone: false })!.querySelector("button")!.textContent).toBe("Undo");
    expect(state.takeUndo(0)?.request.op).toMatchObject({ hold: false });
  });
});
