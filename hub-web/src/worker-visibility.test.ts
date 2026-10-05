import { describe, expect, it } from "vitest";
import { readablePanes, sessionsPath } from "./worker-visibility";
import type { PaneInfo } from "./types";

function pane(id: string, kind: PaneInfo["kind"]): PaneInfo {
  return { id, kind, focused: false, title: id, exited: false };
}

const panes = [pane("director", "Director"), pane("bright-otter", "Supervisor"), pane("agile-octopus", "Worker"), pane("steady-badger", "Worker")];

describe("worker visibility", () => {
  it("asks the catalog for supervisors only, dormant ones for recovery", () => {
    expect(sessionsPath()).toBe("/v1/sessions");
    expect(sessionsPath(true)).toBe("/v1/sessions?dormant=1");
  });

  it("reads the supervisor's pane, never the director or a worker (cas-0546)", () => {
    expect(readablePanes(panes).map((pane) => pane.id)).toEqual(["bright-otter"]);
  });
});

import { retainPendingSessions, visibleCatalog } from './worker-visibility';
import type { HubSession } from './types';

const liveSession: HubSession = { name: 'live', supervisor: 'supervisor', workers: ['worker'], liveness: 'live' };
describe('staffed session catalog', () => {
  it('hides dead, worker-only and unreachable rows; keeps live supervisors, with or without workers (cas-7103, cas-645e)', () => {
    const rows = [liveSession, { ...liveSession, name: 'dead', dormant: true }, { ...liveSession, name: 'empty', workers: [] }, { ...liveSession, name: 'worker', supervisor: '' }, { ...liveSession, name: 'missing', liveness: 'missing_endpoint' as const }];
    expect(visibleCatalog(rows, () => false).map(row => row.name)).toEqual(['live', 'empty']);
    // Recovery lists every supervisor, whatever its state, but never a row
    // with no supervisor: the list cannot show one (cas-645e QA F02).
    expect(visibleCatalog(rows, () => false, true, true).map(row => row.name)).toEqual(['live', 'dead', 'empty', 'missing']);
  });
  it('retains missing in-flight destinations as unreachable through sending and acknowledgment', () => {
    const rows = retainPendingSessions([liveSession], [], () => true);
    expect(rows).toEqual([{ ...liveSession, unreachable: true }]);
    expect(visibleCatalog(rows, () => true)).toHaveLength(1);
    expect(visibleCatalog(rows, () => false)).toHaveLength(0);
    expect(retainPendingSessions(rows, [liveSession], () => true)).toEqual([liveSession]);
  });
  it('expires cached catalogs but retains pending work, and recovers on a fresh response', () => {
    expect(visibleCatalog([liveSession], () => false, false)).toEqual([]);
    expect(visibleCatalog([liveSession], () => true, false)[0].unreachable).toBe(true);
    expect(visibleCatalog([liveSession], () => true, true)[0].unreachable).toBeUndefined();
  });
  it('never reads an exited supervisor', () => {
    expect(readablePanes(panes.map(pane => ({ ...pane, exited: true })))).toEqual([]);
  });
});
