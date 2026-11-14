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
  stopAction,
  type FleetAgent,
  type FleetTask,
} from "./fleet-ops";
import { phoneFleetNotice, agentControls, headerControls, taskControls, trainControls, undoBar, type FleetOpsHandlers, type FleetOpsViewContext } from "./fleet-ops-view";
import { releaseHeader } from "./progress-view";
import type { Scope } from "./types";
import type { WriteAccessContext } from "./write-grant-view";
import { WriteGrantState } from "./write-grant";

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
    expect(pause).toMatchObject({ destructive: false, operation: "pause-worker" });
    const resume = pause.inverse!();
    expect(resume.request.op).toEqual({ kind: "set_worker_hold", worker: "swift-lark-3", hold: false });
    expect(holdAction({ ...lark, status: "held" }).request.op).toMatchObject({ hold: false });
  });

  it("lists Pause, Restart…, Stop… with the destructive ones confirmed and never undone", () => {
    const [pause, restart, stop] = agentMenu(lark);
    expect([pause!.operation, restart!.operation, stop!.operation]).toEqual(["pause-worker", "restart-worker", "stop-worker"]);
    expect(restart).toMatchObject({ destructive: true, operation: "restart-worker" });
    expect(restart!.inverse).toBeUndefined();
    expect(stop!.question).toContain(lark.name);
    expect(stop!.question).toContain(lark.current_task);
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
    expect(focusEpicAction({ id: "cas-f29b", title: "Current build" }, { id: "cas-c4d3", title: "Previous build" }).request).toEqual({ op: { kind: "focus_epic", epic_id: "cas-f29b" }, expected: { epic_id: "cas-c4d3" } });
    expect(focusEpicAction({ id: "cas-f29b", title: "Current build" }, { id: "cas-c4d3", title: "Previous build" }).inverse!().request.op).toEqual({ kind: "focus_epic", epic_id: "cas-c4d3" });
    // With no focus before, Undo clears it (focus_epic clear=true, cas-566b).
    expect(focusEpicAction({ id: "cas-f29b", title: "Current build" }, null).inverse!().request).toEqual({ op: { kind: "focus_epic", clear: true }, expected: { epic_id: "cas-f29b" } });
    expect(spawnAction(9, "cas-2001").request.op).toEqual({ kind: "spawn_workers", count: 4, task_id: "cas-2001" });
    expect(spawnAction(1).inverse).toBeUndefined();
  });

  it("shows the exact merge message before it is sent, generated from the task", () => {
    // The hub's own template (ops::fleet::request_merge_text), so the preview is what is sent.
    const preview = mergeRequestMessage(parked);
    for (const value of [parked.id, parked.title, parked.branch, parked.tip]) expect(preview).toContain(value);
    expect(requestMergeAction(parked).request).toEqual({ op: { kind: "request_merge", task_id: "cas-1999" }, expected: { status: "awaiting_merge", tip: "9ffb3897" } });
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
  });

  it("offers Undo for 8 s after a reversible action, none after a destructive or additive one, and announces each step", () => {
    const state = new FleetOpsState();
    const pause = holdAction(lark);
    state.started("agent:swift-lark-3", pause);
    expect(state.announcement).toBe(pause.progress);
    expect(state.pending.has("agent:swift-lark-3")).toBe(true);
    state.succeeded("agent:swift-lark-3", pause, 1_000);
    expect(state.announcement).toBe(pause.done);
    expect(state.currentUndo(1_000 + UNDO_WINDOW_MS - 1)?.action.request.op).toMatchObject({ hold: false });
    expect(state.currentUndo(1_000 + UNDO_WINDOW_MS)).toBeUndefined();
    state.succeeded("agent:swift-lark-3", pause, 2_000);
    const undone = state.takeUndo(2_500)!;
    expect(undone.request.op).toMatchObject({ hold: false });
    expect(state.takeUndo(2_600)).toBeUndefined();
    // cas-97d58 F12: the Undo itself succeeds with no Undo of its own.
    state.succeeded("agent:swift-lark-3", undone, 2_700);
    expect(state.announcement).toBe(undone.done);
    expect(state.currentUndo(2_700)).toBeUndefined();
    state.succeeded("agent:swift-lark-3", stopAction(lark), 3_000);
    expect(state.currentUndo(3_000)).toBeUndefined();
    // cas-97d58 F12: a destructive result stays visible for the Undo window instead.
    expect(state.currentResult(3_000)?.label).toBe(stopAction(lark).done);
    expect(state.currentResult(3_000 + UNDO_WINDOW_MS)).toBeUndefined();
    state.succeeded("header", spawnAction(2), 3_000);
    expect(state.currentUndo(3_000)).toBeUndefined();
  });

  it("keeps a failure on its row, in words: stale says what changed, an error says what the hub said", () => {
    const state = new FleetOpsState();
    state.failed("agent:swift-lark-3", stopAction(lark), { stale: true, current: { generation: 3 } });
    expect(state.notes.get("agent:swift-lark-3")).toMatchObject({ tone: "stale" });
    expect(state.notes.get("agent:swift-lark-3")?.text).toContain(lark.name);
    expect(state.announcement).toBe(state.notes.get("agent:swift-lark-3")?.text);
    state.failed("agent:quiet-owl-7", holdAction(owl), { detail: "audit log unavailable" });
    expect(state.notes.get("agent:quiet-owl-7")?.text).toContain("audit log unavailable");
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
    expect(state.notes.get(row)).toMatchObject({ tone: "error" });
    expect(state.notes.get(row)?.text).toContain(expected);
    expect(state.announcement).toBe(state.notes.get(row)?.text);
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
  return { state, scopes, origin: ORIGIN, now: 0, agents: [lark, owl], tasks: [ready, parked], epics: [{ id: "cas-f29b", title: "Build work", status: "open", assignee: "lead" }, { id: "cas-c4d3", title: "Current work", status: "open", assignee: "lead" }], supervisors: ["lead"], currentEpic: "cas-c4d3", asked: new Map(), relative: () => "2m ago", on: handlers };
}

describe("the rail's controls draw that state (cas-a474)", () => {
  it("keeps unavailable release facts in evidence text instead of headings or count emphasis (cas-504e9)", () => {
    for (const epic of [undefined, { id: "train", title: "Release with missing counts" }]) {
      const header = releaseHeader(document, epic);
      expect(header.querySelector(".release-count, strong, b, .status-chip")).toBeNull();
      const evidence = header.querySelector("p.release-meta");
      expect(evidence?.textContent?.trim()).toBeTruthy();
      expect(header.querySelector("h3")?.textContent).toBe(epic?.title);
      if (!epic) {
        expect(header.children).toHaveLength(1);
        expect(header.querySelector("h1, h2, h3, h4, h5, h6, [role=heading]")).toBeNull();
      }
    }
  });
  it("shows a frozen train's measured ETA and offers the operator one-tap Lift, Freeze otherwise (cas-2f4d)", () => {
    const choose = vi.fn();
    const frozen = { id: "cas-tr", title: "Release train vNEXT", freeze: { frozen_at: "2026-11-07T19:05:00Z", eta: "2026-11-07T19:50:00Z", summary: "Frozen 2:05 PM ET · ETA 2:50 PM ET" } };
    expect(releaseHeader(document, frozen).querySelector(".release-freeze")?.textContent).toBe("Frozen 2:05 PM ET · ETA 2:50 PM ET");
    // cas-16c0a: release decisions work with a default read-only pairing.
    const readOnly: Scope[] = ["machine-read", "session-read", "pane-read"];
    const lift = trainControls(document, context(readOnly, new FleetOpsState(), { choose }), frozen).querySelector("button")!;
    expect(lift.disabled).toBe(false);
    expect(lift.getAttribute("aria-disabled")).not.toBe("true");
    lift.click();
    expect(choose.mock.calls[0]![1].request.op).toEqual({ kind: "lift_train_freeze", epic_id: "cas-tr" });
    const freeze = trainControls(document, context(readOnly, new FleetOpsState(), { choose }), { id: "cas-tr" }).querySelector("button")!;
    freeze.click();
    expect(choose.mock.calls[1]![1].request.op).toEqual({ kind: "freeze_train", epic_id: "cas-tr" });
    // cas-e060: a train row offers the operator Pause, and Resume once paused.
    for (const [paused, call] of [[false, 2], [true, 3]] as const) {
      const toggle = trainControls(document, context([...CONTROL, "factory-operate"], new FleetOpsState(), { choose }), { id: "cas-tr", auto_cut_paused: paused }).querySelectorAll("button")[1]!;
      toggle.click();
      expect(choose.mock.calls[call]![1].request.op).toEqual({ kind: "set_auto_cut_paused", epic_id: "cas-tr", paused: !paused });
    }
  });

  it("gates each menu item: a control pairing can pause but sees Stop disabled with the command that adds factory:manage", () => {
    const state = new FleetOpsState();
    state.menuFor = "agent:swift-lark-3";
    const view = agentControls(document, context([...CONTROL, "factory-operate"], state), lark);
    const trigger = view.querySelector<HTMLButtonElement>(".fleet-ops-trigger")!;
    expect(trigger.getAttribute("aria-haspopup")).toBe("menu");
    expect(trigger.getAttribute("aria-expanded")).toBe("true");
    expect(trigger.getAttribute("aria-label")).toBeTruthy();
    const items = [...view.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')];
    expect(items.map((item) => item.dataset.fleetFocus)).toEqual(["agent:swift-lark-3:item:pause:swift-lark-3", "agent:swift-lark-3:item:restart:swift-lark-3", "agent:swift-lark-3:item:stop:swift-lark-3"]);
    expect(items[0]!.hasAttribute("aria-disabled")).toBe(false);
    const stop = items[2]!;
    expect(stop.getAttribute("aria-disabled")).toBe("true");
    expect(view.querySelector(`#${stop.getAttribute("aria-describedby")!}`)?.textContent).toBeTruthy();
    // A separator sets the destructive items apart.
    expect(view.querySelector('[role="separator"]')).not.toBeNull();
  });

  it("draws the inline confirmation with Cancel first and ignores a double-click's second press", () => {
    const state = new FleetOpsState();
    state.confirm = { rowKey: "agent:swift-lark-3", action: stopAction(lark) };
    const confirm = vi.fn();
    const view = agentControls(document, context([...CONTROL, "factory-manage"], state, { confirm }), lark);
    const buttons = [...view.querySelectorAll<HTMLButtonElement>(".fleet-ops-confirm button")];
    expect(buttons.map((button) => button.dataset.fleetFocus)).toEqual(["agent:swift-lark-3:cancel", "agent:swift-lark-3:confirm"]);
    expect(buttons[1]!.classList.contains("danger")).toBe(true);
    expect(view.querySelector(".fleet-ops-question")?.textContent).toMatch(new RegExp("(?=[\\s\\S]*cas\\-1234)(?=[\\s\\S]*swift\\-lark\\-3)"));
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
    expect([...view.querySelectorAll<HTMLElement>(".fleet-ops-preview button")].map((button) => button.dataset.fleetFocus)).toEqual(["task:cas-1999:preview-cancel", "task:cas-1999:send"]);
    const asked = taskControls(document, { ...context(CONTROL), asked: new Map([["cas-1999", 1]]) }, parked)!;
    expect(asked.querySelector(".fleet-ops-asked")?.textContent).toContain("2m");
    expect(asked.querySelector(".fleet-ops-ask")?.textContent).toBeTruthy();
    // A close gate can keep an integrated delivery parked. Neither screen
    // size may offer another merge, even if its preview was already open.
    for (const phone of [false, true]) {
      for (const evidence of [{ delivery_state: "merged" as const }, { merge_check_pending: true }]) {
        const controls = taskControls(document, { ...context(CONTROL, state), phone }, { ...parked, ...evidence });
        expect(controls?.querySelector("button")).toBeFalsy();
      }
    }
  });

  it("offers Assign… on a ready task with the idle workers, and the header's Add worker… and Focus epic…", () => {
    const state = new FleetOpsState();
    state.assignFor = "task:cas-2001";
    const view = taskControls(document, context([...CONTROL, "factory-operate"], state), ready)!;
    expect([...view.querySelectorAll('[role="menuitem"]')].map((item) => item.textContent)).toEqual([expect.stringContaining(owl.name)]);
    const catalog = [
      { id: "current", title: "Current work", status: "open", assignee: "lead", priority: 0 },
      { id: "low", title: "Small fixes", status: "open", epic_verification_owner: "lead-id", priority: 2 },
      { id: "foreign", title: "Other project", status: "open", assignee: "other", priority: 0 },
      { id: "closed", title: "Old release", status: "closed", assignee: "lead", priority: 0 },
      { id: "cancelled", title: "Abandoned", status: "cancelled", assignee: "lead", priority: 0 },
      { id: "high", title: "Launch repair", status: "in_progress", assignee: "lead", priority: 0 },
      { id: "train", title: "Release train next", status: "open", assignee: "lead", priority: 1, auto_cut_paused: false },
    ];
    const choose = vi.fn();
    const header = headerControls(document, { ...context([...CONTROL, "factory-operate"], state, { choose }), epics: catalog, currentEpic: "current", supervisors: ["lead", "lead-id"] }, "focus");
    const items = [...header.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')];
    for (const item of items) item.click();
    const actions = choose.mock.calls.map(call => call[1]);
    expect(actions.map(action => action.request.op.epic_id)).toEqual(["train", "high", "low"]);
    for (const [index, id] of ["train", "high", "low"].entries()) {
      const title = catalog.find(epic => epic.id === id)!.title;
      expect(items[index]!.textContent).toContain(title);
      expect(actions[index].progress).toContain(title);
      expect(actions[index].done).toContain(title);
    }
    expect(actions[0].inverse().request.op.epic_id).toBe("current");
    expect(actions[0].inverse().done).toContain(catalog[0]!.title);
    const empty = headerControls(document, { ...context([...CONTROL, "factory-operate"]), epics: catalog.filter(epic => epic.status === "closed") }, "focus");
    expect(empty.querySelectorAll('[role="menuitem"]')).toHaveLength(0);
    expect(empty.querySelector('[role="status"]')).not.toBeNull();
    const readOnly = headerControls(document, context(["machine-read", "session-read", "pane-read"]), undefined);
    expect(readOnly.querySelector(".fleet-ops-add")).toBeNull();
    expect(readOnly.querySelector("summary")?.textContent).toBeTruthy();
  });

  it("collapses permissions to one line, hides unavailable controls and links the pairing remedy", () => {
    const writeAccess: WriteAccessContext = { state: new WriteGrantState(), tasks: [], on: { changed: vi.fn(), review: vi.fn(), revoke: vi.fn(), confirm: vi.fn(), cancel: vi.fn() } };
    const header = headerControls(document, { ...context(CONTROL), writeAccess }, undefined);
    expect(header.querySelectorAll("button")).toHaveLength(0);
    expect(header.querySelector("details")?.open).toBe(false);
    expect(header.querySelector("summary")?.textContent).toBeTruthy();
    expect(header.querySelector("details")?.textContent).toBeTruthy();
    expect(header.querySelector("a")?.getAttribute("href")).toBe("#paired-machines");
    const manage = headerControls(document, { ...context([...CONTROL, "factory-manage"]), writeAccess }, undefined);
    expect(manage.querySelector(".fleet-ops-grant")).not.toBeNull();
    expect(manage.querySelector(".fleet-ops-add")).toBeNull();
    const all = headerControls(document, { ...context([...CONTROL, "factory-manage", "factory-operate"]), writeAccess }, undefined);
    expect(all.querySelector("details")).toBeNull();
    expect(all.querySelectorAll("button")).toHaveLength(3);
  });

  it("draws the Undo offer while it lasts", () => {
    const state = new FleetOpsState();
    state.succeeded("agent:swift-lark-3", holdAction(lark), 0);
    const undo = vi.fn();
    const bar = undoBar(document, context(CONTROL, state, { undo }))!;
    expect(bar.textContent).toContain("swift-lark-3");
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
    expect(allowed.querySelector("button")?.getAttribute("aria-label")).toBeTruthy();
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
    expect(headerControls(document, ctx, "focus").querySelector("input")?.getAttribute("aria-label")).toBeTruthy();
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
  expect(refused.textContent).toBeTruthy();
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
    expect(undoBar(document, { ...ctx, phone: false })!.querySelector<HTMLElement>("button")!.dataset.fleetFocus).toBe("undo");
    expect(state.takeUndo(0)?.request.op).toMatchObject({ hold: false });
  });
});
