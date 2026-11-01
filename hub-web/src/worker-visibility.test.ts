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

import { visibleCatalog } from './worker-visibility';
import type { HubSession } from './types';

const liveSession: HubSession = { name: 'live', supervisor: 'supervisor', workers: ['worker'], liveness: 'live' };
describe('staffed session catalog', () => {
  it('hides dead, worker-only and unreachable rows; keeps live supervisors, with or without workers (cas-7103, cas-645e)', () => {
    const rows = [{ ...liveSession, name: 'ended', ended: { at: '2026-11-01T00:00:00Z', reason: 'exit', workers_stopped: [], orphans: [] } }, liveSession, { ...liveSession, name: 'dead', dormant: true, liveness: 'stale_metadata' as const }, { ...liveSession, name: 'empty', workers: [] }, { ...liveSession, name: 'worker', supervisor: '' }, { ...liveSession, name: 'missing', liveness: 'missing_endpoint' as const }];
    expect(visibleCatalog(rows).map(row => row.name)).toEqual(['live', 'empty']);
    // Recovery cannot turn stopped processes into running conversations.
    expect(visibleCatalog(rows, true, true).map(row => row.name)).toEqual(['live', 'empty']);
  });
  it('does not retain a missing destination just because it has a pending message (cas-f8ab)', () => {
    const rows = visibleCatalog([{ ...liveSession, liveness: "stale_metadata" }]);
    expect(rows).toEqual([]);
    expect(visibleCatalog(rows)).toHaveLength(0);
    expect(visibleCatalog([liveSession])).toEqual([liveSession]);
  });
  it('retains last-known running and dormant daemons when catalog freshness expires', () => {
    expect(visibleCatalog([liveSession], false).map(row => row.name)).toEqual(['live']);
    expect(visibleCatalog([{ ...liveSession, dormant: true }]).map(row => row.name)).toEqual(['live']);
    expect(visibleCatalog([liveSession], true)[0].unreachable).toBeUndefined();
  });
  it('never reads an exited supervisor', () => {
    expect(readablePanes(panes.map(pane => ({ ...pane, exited: true })))).toEqual([]);
  });
});
