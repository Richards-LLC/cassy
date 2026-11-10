import { expect, it } from "vitest";
import { qualityView, type LedgerRelease } from "./quality-model";

const turns = (wakes: number, noOp: number, input: number) => ({ wakes, no_op_wakes: noOp, input_tokens: input, output_tokens: 0 });
function release(name: string, from: string, to: string | null, fields: Partial<LedgerRelease>): LedgerRelease {
  return { release: name, published: to !== null, from, to, turns: turns(0, 0, 0), interventions_total: 0, interventions: {}, checks: {}, escapes: [], friction: [], ...fields };
}

// Shaped like the 4.12 → 4.13 ledger: 4.13 ran three times as long with ten times the deliveries.
const report = {
  supervisors: { "f056bf8a-0427": "cosmic-pelican-12" },
  releases: [
    release("v4.11.0", "2026-11-10T00:00:00Z", "2026-11-10T04:00:00Z", { deliveries: { unknown: "the merge record starts inside this window" }, changes: [] }),
    release("v4.12.0", "2026-11-10T04:00:00Z", "2026-11-10T12:00:00Z", {
      deliveries: 10, interventions_total: 20, turns: turns(100, 80, 100e6), checks: { verifier: { pass: 18, fail: 2 } },
      turns_by_actor: { "f056bf8a-0427": { daemon: turns(90, 80, 90e6) } }, changes: [],
    }),
    release("unreleased", "2026-11-10T12:00:00Z", null, {
      deliveries: 100, interventions_total: 50, turns: turns(400, 100, 2000e6), checks: { verifier: { pass: 90, fail: 10 } },
      turns_by_actor: { "f056bf8a-0427": { daemon: turns(300, 100, 1200e6), operator: turns(100, 0, 800e6) }, "7ec6ea4c-b248": { worker: turns(1, 0, 5e6) } },
      changes: ["close gate", "skill cas-worker"],
    }),
  ],
};

it("normalises rates per merged delivery, keeps per hour for the drill-down and heads with the biggest move worse", () => {
  const view = qualityView(report, new Date("2026-11-10T23:00:00Z"))!;
  expect([view.previous, view.current, view.inProgress]).toEqual(["4.12", "4.13", true]);
  const row = (name: string) => view.rubrics.find(item => item.name === name)!;
  // 2000M over 100 deliveries against 100M over 10: 20M vs 10M per delivery, though the raw total is 20×.
  expect(row("Context burn").points!.slice(1).map(point => point.value)).toEqual([10, 20]);
  expect(row("Context burn").points!.at(-1)!.perHour).toBeCloseTo(2000 / 11);
  expect(row("Gate overrides").points!.slice(1).map(point => point.value)).toEqual([2, 0.5]);
  expect(row("Gate overrides").trend!.word).toBe("better");
  expect(view.hero!.name).toBe("Context burn");
  expect(view.hero!.trend.ratio).toBe(2);
  // Awaiting rubrics carry no number, and sort after every measured row of their actor.
  expect(row("Claim honesty").points).toBeUndefined();
  const workers = view.rubrics.filter(item => item.actor === "workers");
  expect(workers.findIndex(item => !item.points)).toBe(workers.filter(item => item.points).length);
  // Only a release whose git range touched a skill or gate carries a change marker.
  expect(row("Context burn").points!.map(point => Boolean(point.change))).toEqual([false, false, true]);
});

it("breaks supervisor tokens down by wake source with the average against the previous release", () => {
  const view = qualityView(report, new Date("2026-11-10T23:00:00Z"))!;
  expect(view.wakeSources.map(row => [row.supervisor, row.source, row.average, row.was])).toEqual([
    ["cosmic-pelican-12", "daemon", 4e6, 1e6],
    ["cosmic-pelican-12", "operator", 8e6, undefined],
    ["supervisor 7ec6ea4c", "worker", 5e6, undefined],
  ]);
  // Any selected release has its own table: 4.12 against 4.11, which had no turns.
  expect(view.records[1].wakeSources.map(row => [row.supervisor, row.source, row.wakes, row.noOp, row.average, row.total, row.was])).toEqual([
    ["cosmic-pelican-12", "daemon", 90, 80, 1e6, 90e6, undefined],
  ]);
});
