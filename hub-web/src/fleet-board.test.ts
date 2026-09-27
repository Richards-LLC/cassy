// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { FleetBoardRenderer, fleetBoardSignature, fleetPlotLabels, fleetProvenance, type FleetBoardModel } from "./fleet-board";
import type { SessionPickerEntry } from "./session-selection";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const css = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "styles.css"), "utf8");

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

  it("leads each row with the project and names the codename once beneath it (journey F1)", () => {
    const renderer = new FleetBoardRenderer();
    const board = freshBoard();
    renderer.render(board, model({ sessions: [entry({ session: "keen-lynx-1", supervisor: "keen-lynx-1", project: "orion", workerCount: 1 }), entry({ session: "bright-otter", supervisor: "bright-otter" })] }), { open: vi.fn() });
    const [projectRow, bareRow] = [...board.querySelectorAll<HTMLButtonElement>(".fleet-session")];
    expect(projectRow!.querySelector(".session-name")?.textContent).toBe("orion");
    expect(projectRow!.querySelector(".session-meta")?.textContent).toBe("supervisor keen-lynx-1 · 1 worker · live");
    expect(projectRow!.textContent!.split("keen-lynx-1")).toHaveLength(2);
    expect(projectRow!.getAttribute("aria-label")).toBe("Open orion, keen-lynx-1 on Studio Mac");
    // No project: the session name heads the row and is not repeated.
    expect(bareRow!.querySelector(".session-name")?.textContent).toBe("bright-otter");
    expect(bareRow!.querySelector(".session-meta")?.textContent).toBe("supervisor · 3 workers · live");
    expect(bareRow!.getAttribute("aria-label")).toBe("Open bright-otter on Studio Mac");
    // The plot's row labels lead with the project as well.
    const plotRow = board.querySelector<HTMLElement>('.fleet-plot-row[data-fleet-session="keen-lynx-1"] th')!;
    expect(plotRow.querySelector(".fleet-plot-name")?.textContent).toBe("orion");
    expect(plotRow.getAttribute("aria-label")).toBe("orion, keen-lynx-1 on Studio Mac");
    expect(plotRow.title).toBe("orion · keen-lynx-1");
    // A project appearing later rebuilds the row.
    expect(fleetBoardSignature(model({ sessions: [entry({ project: "orion" })] }))).not.toBe(fleetBoardSignature(model({ sessions: [entry()] })));
  });

  it("tells plot rows that share a project apart by the codename's shortest distinct tail (cas-598e QA F01)", () => {
    const atlas = { machineId: "m-atlas", machineLabel: "Atlas · Linux" };
    const forge = { machineId: "m-forge", machineLabel: "Forge · Linux" };
    const sessions = [
      entry({ ...atlas, session: "patient-pelican-9", supervisor: "patient-pelican-9", project: "cas-src" }),
      entry({ ...forge, session: "brisk-otter-5", supervisor: "brisk-otter-5", project: "cas-src" }),
      entry({ ...forge, session: "quiet-heron-8", supervisor: "quiet-heron-8", project: "cas-src" }),
      entry({ ...forge, session: "loose-wren-7", supervisor: "loose-wren-7" }),
      entry({ session: "calm-otter-4", supervisor: "calm-otter-4", project: "gabber-studio" }),
    ];
    const labels = fleetPlotLabels(sessions);
    expect(labels.get("m-atlas/patient-pelican-9")).toEqual({ name: "cas-src", tag: "pelican-9" });
    expect(labels.get("m-forge/brisk-otter-5")).toEqual({ name: "cas-src", tag: "otter-5" });
    expect(labels.get("m-forge/quiet-heron-8")).toEqual({ name: "cas-src", tag: "heron-8" });
    // A label that does not repeat carries no tag.
    expect(labels.get("m-forge/loose-wren-7")).toEqual({ name: "loose-wren-7" });
    expect(labels.get("m-studio/calm-otter-4")).toEqual({ name: "gabber-studio" });
    // Tails that collide grow until they differ; identical codenames name the machine.
    const grown = fleetPlotLabels([entry({ session: "brave-otter-5", supervisor: "brave-otter-5", project: "p" }), entry({ session: "calm-otter-5", supervisor: "calm-otter-5", project: "p" })]);
    expect([...grown.values()].map((label) => label.tag)).toEqual(["brave-otter-5", "calm-otter-5"]);
    const twins = fleetPlotLabels([entry({ ...atlas, session: "keen-lynx-1", supervisor: "keen-lynx-1", project: "p" }), entry({ ...forge, session: "keen-lynx-1", supervisor: "keen-lynx-1", project: "p" })]);
    expect([...twins.values()].map((label) => label.tag)).toEqual(["keen-lynx-1 · Atlas · Linux", "keen-lynx-1 · Forge · Linux"]);
    // Rendered: the tag is its own span, after the project, so the project gives way first.
    const board = freshBoard();
    new FleetBoardRenderer().render(board, model({ sessions }), { open: vi.fn() });
    const names = [...board.querySelectorAll<HTMLElement>(".fleet-plot-row th .fleet-plot-name")].map((name) => [name.querySelector(".fleet-plot-project")?.textContent ?? name.textContent, name.querySelector(".fleet-plot-tag")?.textContent ?? ""]);
    expect(names.filter(([project]) => project === "cas-src").map(([, tag]) => tag).sort()).toEqual(["heron-8", "otter-5", "pelican-9"]);
    expect(new Set(names.map((pair) => pair.join(" "))).size).toBe(names.length);
    expect(css).toMatch(/\.fleet-plot-name\.tagged\s*\{[^}]*display: flex;/);
    expect(css).toMatch(/\.fleet-plot-project\s*\{[^}]*min-width: 0;[^}]*text-overflow: ellipsis;/);
    expect(css).toMatch(/\.fleet-plot-tag\s*\{[^}]*flex: none;/);
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

  it.each([3, 8])("keeps %i session names and inline marks in the same fixed table rows", (count) => {
    const board = freshBoard();
    new FleetBoardRenderer().render(board, model({ sessions: Array.from({ length: count }, (_, i) =>
      entry({ session: `cas-src-extraordinarily-long-falcon-${i}`, phase: "reviewing" })) }), { open: vi.fn() });
    const rows = [...board.querySelectorAll<HTMLTableRowElement>(".fleet-plot-row")];
    expect(rows).toHaveLength(count);
    for (const [index, row] of rows.entries()) {
      expect(row.cells).toHaveLength(6);
      expect(row.cells[0].title).toBe(row.dataset.fleetSession);
      expect(row.querySelector(".fleet-plot-name")?.textContent).toBe(`long-falcon-${index}`);
      expect(row.querySelector(".fleet-plot-mark > .fleet-dot + .fleet-dot-phase")?.textContent).toBe("reviewing");
    }
    // jsdom has no layout engine: pin the shared geometry here; the browser
    // receipt measures actual centers, all eight row heights and text bounds.
    expect(css).toMatch(/\.fleet-plot tbody th, \.fleet-track-cell\s*\{[^}]*height: var\(--button-compact-height\);[^}]*padding: 0;[^}]*vertical-align: middle;/);
    expect(css).toMatch(/\.fleet-plot-name\s*\{[^}]*line-height: var\(--button-compact-height\);[^}]*white-space: nowrap;[^}]*overflow: hidden;[^}]*text-overflow: ellipsis;/);
    expect(css).toMatch(/\.fleet-plot-mark\s*\{[^}]*display: inline-flex;[^}]*align-items: center;/);
    expect(css).toMatch(/\.fleet-track-cell::before\s*\{[^}]*bottom: 0;/);
  });

  it("keeps full state names accessible and supplies a nonbreaking narrow-figure legend", () => {
    const board = freshBoard();
    new FleetBoardRenderer().render(board, model(), { open: vi.fn() });
    expect(board.querySelector("wbr")).toBeNull();
    const labels = [...board.querySelectorAll("thead th")].slice(1).map(node => node.getAttribute("aria-label"));
    expect(labels).toEqual(["Needs you", "Working", "Idle", "Stale", "Unreachable"]);
    expect([...board.querySelectorAll(".fleet-track-key")].map(node => node.textContent)).toEqual(["1", "2", "3", "4", "5"]);
    expect([...board.querySelectorAll(".fleet-track-legend > span")].map(node => node.textContent)).toEqual(labels.map((label, i) => `${i + 1} ${label}`));
    expect(css).toMatch(/\.fleet-plot th\s*\{[^}]*white-space: nowrap;/);
    expect(css).toMatch(/\.fleet-track-legend > span\s*\{[^}]*white-space: nowrap;/);
  });

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

describe("the Fleet overview reads as a product, not a debug page (journey F2)", () => {
  it("shades the Working column only when a session is in it, and says so only then", () => {
    const idle = freshBoard();
    new FleetBoardRenderer().render(idle, model({ sessions: [entry({ phase: "idle" })] }), { open: vi.fn() });
    expect(idle.querySelector("table.fleet-plot")?.classList.contains("has-working")).toBe(false);
    expect(idle.querySelector(".fleet-figure-caption")?.textContent).toBe("One dot per session. Ringed: needs you.");
    const busy = freshBoard();
    new FleetBoardRenderer().render(busy, model({ sessions: [entry({ phase: "building" })] }), { open: vi.fn() });
    expect(busy.querySelector("table.fleet-plot")?.classList.contains("has-working")).toBe(true);
    expect(busy.querySelector(".fleet-figure-caption")?.textContent).toBe("One dot per session. Ringed: needs you. Shaded: working.");
    expect(css).toContain(".fleet-plot.has-working th:nth-child(3), .fleet-plot.has-working .track-working {");
    expect(css).not.toMatch(/^\.fleet-plot th:nth-child\(3\), \.track-working \{/m);
  });

  it("replaces the run-on provenance paragraph with one short line, details on hover", () => {
    const updated = model({ machines: [
      { id: "m-studio", label: "Studio Mac", state: "live", phase: "Live", selected: true, hubVersion: "3.31.0", catalogUpdatedAt: "2026-09-07T12:59:00Z" },
      { id: "m-attic", label: "Attic Linux", state: "backoff", phase: "Reconnecting", selected: false, catalogUpdatedAt: "2026-09-07T13:04:00Z" },
    ] });
    const line = fleetProvenance(updated);
    expect(line.text).toMatch(/^Last updated \d{2}:\d{2}$/);
    expect(line.details.split("\n")).toHaveLength(2);
    expect(line.details).toContain("Studio Mac · Live · Hub 3.31.0");
    const board = freshBoard();
    new FleetBoardRenderer().render(board, updated, { open: vi.fn() });
    const provenance = board.querySelector<HTMLElement>(".fleet-provenance")!;
    expect(provenance.textContent).toBe(line.text);
    expect(provenance.title).toBe(line.details);
    expect(provenance.textContent).not.toMatch(/ \/ |catalog|Hub /);
    // The time is said once: the header no longer repeats it.
    expect(board.querySelector(".fleet-catalog-time")).toBeNull();
    expect(fleetProvenance(model()).text).toBe("Waiting for the first update");
    expect(fleetProvenance(model({ machines: [] })).text).toBe("");
  });

  it("stacks the verdict above the figure when the board is too narrow for the state words", () => {
    expect(css).toContain(".fleet-board { container: fleet-board / inline-size; }");
    expect(css).toMatch(/@container fleet-board \(max-width: 60rem\) \{\n  \.fleet-hero \{ grid-template-columns: minmax\(0, 1fr\);/);
  });
});
