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
    const notes = [...board.querySelectorAll(".fleet-empty-sessions")].map((node) => node.textContent);
    expect(notes).toEqual(["No live sessions.", "Sessions appear once the machine is reachable."]);
  });
});
