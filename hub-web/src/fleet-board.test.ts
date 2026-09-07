// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { FleetBoardRenderer, fleetBoardSignature, type FleetBoardModel } from "./fleet-board";
import type { SessionPickerEntry } from "./session-selection";

function entry(overrides: Partial<SessionPickerEntry> = {}): SessionPickerEntry {
  return {
    machineId: "m-studio",
    machineLabel: "Studio Mac",
    session: "gabber-studio-witty-panda-98",
    role: "supervisor",
    supervisor: "witty-panda-98",
    workerCount: 3,
    status: "live",
    current: false,
    ...overrides,
  };
}

function model(overrides: Partial<FleetBoardModel> = {}): FleetBoardModel {
  return {
    machines: [
      { id: "m-studio", label: "Studio Mac", state: "live", phase: "Live", selected: true },
      { id: "m-attic", label: "Attic Linux", state: "backoff", phase: "Reconnecting", selected: false },
    ],
    sessions: [entry(), entry({ session: "cas-src-brave-otter-12", supervisor: "brave-otter-12", workerCount: 1 })],
    ...overrides,
  };
}

/** What `render()` does on a shell rebuild: a brand-new, empty container. */
function freshBoard(): HTMLElement {
  const board = document.createElement("div");
  board.id = "fleet-board";
  board.className = "fleet-board";
  document.body.append(board);
  return board;
}

describe("fleet board region lifecycle", () => {
  beforeEach(() => { document.body.innerHTML = ""; });

  it("populates a brand-new container after a shell rebuild even when nothing it shows changed", () => {
    // The cas-c2ba review finding: opening the drawer, collapsing the panel or
    // switching a context tab replaces app.innerHTML; the updater then saw the
    // same signature and returned before filling the new empty board.
    const renderer = new FleetBoardRenderer();
    const callbacks = { open: vi.fn() };
    const first = freshBoard();
    expect(renderer.render(first, model(), callbacks)).toBe(true);
    expect(first.querySelectorAll(".fleet-session")).toHaveLength(2);

    for (const toggle of ["drawer open", "drawer closed", "panel collapsed", "context tab", "picker open"]) {
      first.remove();
      const rebuilt = freshBoard();
      expect(renderer.render(rebuilt, model(), callbacks), toggle).toBe(true);
      expect(rebuilt.querySelectorAll(".fleet-session"), toggle).toHaveLength(2);
      expect(rebuilt.querySelector(".fleet-board-summary")?.textContent, toggle).toBe("2 machines · 2 sessions · 1 not live");
    }
  });

  it("leaves the existing nodes and their focus alone on an unchanged heartbeat", () => {
    const renderer = new FleetBoardRenderer();
    const callbacks = { open: vi.fn() };
    const board = freshBoard();
    renderer.render(board, model(), callbacks);
    const card = board.querySelector<HTMLButtonElement>(".fleet-session")!;
    card.focus();
    expect(document.activeElement).toBe(card);

    // Six heartbeats' worth of region renders with identical data.
    for (let beat = 0; beat < 6; beat += 1) expect(renderer.render(board, model(), callbacks)).toBe(false);
    expect(board.querySelector(".fleet-session")).toBe(card);
    expect(document.activeElement).toBe(card);
    card.click();
    expect(callbacks.open).toHaveBeenCalledWith("m-studio", "gabber-studio-witty-panda-98");
  });

  it("rebuilds when a machine phase, a session or a summary changes", () => {
    const renderer = new FleetBoardRenderer();
    const callbacks = { open: vi.fn() };
    const board = freshBoard();
    renderer.render(board, model(), callbacks);
    const before = board.querySelector(".fleet-session");

    const attic = model().machines[1];
    const reconnected = model({ machines: [model().machines[0], { ...attic, state: "live", phase: "Live" }] });
    expect(renderer.render(board, reconnected, callbacks)).toBe(true);
    expect(board.querySelector("[data-fleet-machine='m-attic'] .fleet-machine-phase")?.textContent).toBe("Live");
    expect(board.querySelector(".fleet-board-summary")?.textContent).toBe("2 machines · 2 sessions");
    expect(board.querySelector(".fleet-session")).not.toBe(before);

    const summarised = model({ sessions: [entry({ title: "Visual overhaul", phase: "building" }), model().sessions[1]] });
    expect(renderer.render(board, summarised, callbacks)).toBe(true);
    expect(board.querySelector(".fleet-session .session-summary-title")?.textContent).toBe("Visual overhaul");
    expect(board.querySelector(".fleet-session .phase-chip")?.textContent).toBe("building");
  });

  it("keys on phase words, never on latency or counts", () => {
    // fleetConnectionLabel in main.ts maps a snapshot to one of these words; a
    // latency change inside `live` must produce the same signature.
    expect(fleetBoardSignature(model())).toBe(fleetBoardSignature(model()));
    const live = model().machines[0];
    expect(fleetBoardSignature(model({ machines: [{ ...live, phase: "Live" }] })))
      .not.toBe(fleetBoardSignature(model({ machines: [{ ...live, state: "backoff", phase: "Reconnecting" }] })));
  });

  it("forgets the board when a session opens and re-renders a later one from scratch", () => {
    const renderer = new FleetBoardRenderer();
    const callbacks = { open: vi.fn() };
    const board = freshBoard();
    renderer.render(board, model(), callbacks);
    // Session open: the canvas holds panes, there is no board.
    expect(renderer.render(null, model(), callbacks)).toBe(false);
    // Back to the fleet: same data, new container.
    board.remove();
    const again = freshBoard();
    expect(renderer.render(again, model(), callbacks)).toBe(true);
    expect(again.querySelectorAll(".fleet-machine")).toHaveLength(2);
  });

  it("says why a machine has no sessions", () => {
    const renderer = new FleetBoardRenderer();
    const board = freshBoard();
    renderer.render(board, model({ sessions: [] }), { open: vi.fn() });
    const notes = [...board.querySelectorAll(".fleet-machine .fleet-empty-sessions")].map((node) => node.textContent);
    expect(notes).toEqual(["No live sessions.", "Sessions appear once the machine is reachable."]);
  });
});

describe("fleet verdict and state track", () => {
  beforeEach(() => { document.body.innerHTML = ""; });

  it("puts the critical session first on Needs you with a matching verdict and ledger", () => {
    const board = freshBoard();
    new FleetBoardRenderer().render(board, model({ sessions: [
      entry({ session: "working-fox-12", phase: "building" }),
      { ...entry({ machineId: "m-attic", session: "blocked-owl-34" }), attentionSeverity: "critical" },
      entry({ session: "idle-bear-56", phase: "idle" }),
    ] }), { open: vi.fn() });
    expect(board.querySelector(".fleet-verdict")?.textContent).toBe("1 of 3 sessions needs you; 1 working, 1 idle.");
    const first = board.querySelector(".fleet-plot-row")!;
    expect(first.getAttribute("data-fleet-session")).toBe("blocked-owl-34");
    expect(first.classList.contains("needs-you")).toBe(true);
    expect(first.querySelector(".track-needs-you .fleet-dot")).not.toBeNull();
    expect(board.querySelector(".track-working .fleet-dot-phase")?.textContent).toBe("building");
    expect(board.querySelector(".fleet-session.needs-you")?.getAttribute("data-fleet-session")).toBe("blocked-owl-34");
    expect(board.querySelectorAll(".fleet-dot")).toHaveLength(3);
    expect(board.querySelector("table")?.querySelectorAll('th[scope="col"]')).toHaveLength(6);
  });

  it("states all-working without alarming status colour or a needs-you ring", () => {
    const board = freshBoard();
    new FleetBoardRenderer().render(board, model({ sessions: [entry({ phase: "planning" }), entry({ session: "reviewing-fox-12", phase: "reviewing" })] }), { open: vi.fn() });
    expect(board.querySelector(".fleet-verdict")?.textContent).toBe("All 2 sessions are working.");
    expect(board.querySelectorAll(".working .track-working .fleet-dot")).toHaveLength(2);
    expect(board.querySelector(".needs-you")).toBeNull();
  });

  it("maps liveness and unknown states to a visible fallback without throwing", () => {
    const board = freshBoard();
    expect(() => new FleetBoardRenderer().render(board, model({ sessions: [
      entry({ session: "stale-fox-12", status: "stale_metadata", phase: "editing" }),
      entry({ session: "missing-fox-12", status: "missing_endpoint" }),
      entry({ session: "unknown-fox-12", status: "future_state" }),
      entry({ session: "blocked-fox-12", phase: "blocked" }),
    ] }), { open: vi.fn() })).not.toThrow();
    expect(board.querySelectorAll(".track-stale .fleet-dot")).toHaveLength(2);
    expect(board.querySelectorAll(".track-unreachable .fleet-dot")).toHaveLength(1);
    expect(board.querySelectorAll(".track-needs-you .fleet-dot")).toHaveLength(1);
  });

  it("renders a designed zero-machine state and a short verdict for every mix", () => {
    const board = freshBoard();
    new FleetBoardRenderer().render(board, model({ machines: [], sessions: [] }), { open: vi.fn() });
    expect(board.querySelector("h2")?.textContent).toBe("Your fleet starts with one machine.");
    expect(board.querySelector(".fleet-empty-sessions")?.textContent).toContain("Pair the machine");
    expect(board.querySelector(".fleet-verdict")!.textContent!.split(/\s+/).length).toBeLessThanOrEqual(22);
  });

  it("updates critical arrival and dismissal, but retains focus for catalog receipt changes", () => {
    const renderer = new FleetBoardRenderer();
    const board = freshBoard();
    const initial = model({ sessions: [entry({ phase: "editing" })] });
    renderer.render(board, initial, { open: vi.fn() });
    const critical = model({ sessions: [{ ...initial.sessions[0], attentionSeverity: "critical" }] });
    expect(renderer.render(board, critical, { open: vi.fn() })).toBe(true);
    expect(board.querySelector(".needs-you .fleet-dot")).not.toBeNull();
    expect(renderer.render(board, initial, { open: vi.fn() })).toBe(true);
    expect(board.querySelector(".needs-you")).toBeNull();
    const button = board.querySelector<HTMLButtonElement>(".fleet-session")!;
    button.focus();
    const refreshed = { ...initial, machines: initial.machines.map((machine) => ({ ...machine, catalogUpdatedAt: "2026-09-07T13:00:00Z" })) };
    expect(renderer.render(board, refreshed, { open: vi.fn() })).toBe(false);
    expect(board.querySelector(".fleet-session")).toBe(button);
    expect(document.activeElement).toBe(button);
    expect(board.querySelector(".fleet-provenance")?.textContent).not.toContain("catalog not reported");
  });
});
