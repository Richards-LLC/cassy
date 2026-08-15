export interface PaneLayout {
  readonly primaryPaneId: string;
  readonly paneIds: readonly string[];
}

export interface PaneLayoutStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

const STORAGE_PREFIX = "cas-commander:pane-layout:";
const LAYOUT_SCHEMA_VERSION = 1;

interface StoredPaneLayout extends PaneLayout {
  readonly version: number;
}

function validPaneIds(ids: readonly string[], activePaneIds: readonly string[]): string[] {
  const active = new Set(activePaneIds);
  const retained = ids.filter((id, index) => active.has(id) && ids.indexOf(id) === index);
  return [...retained, ...activePaneIds.filter((id) => !retained.includes(id))];
}

export function normalizePaneLayout(
  activePaneIds: readonly string[],
  saved: Partial<PaneLayout> | undefined,
  fallbackPrimaryPaneId: string | undefined,
): PaneLayout | undefined {
  if (activePaneIds.length === 0) return undefined;
  const paneIds = validPaneIds(saved?.paneIds ?? [], activePaneIds);
  const primaryPaneId = saved?.primaryPaneId && paneIds.includes(saved.primaryPaneId)
    ? saved.primaryPaneId
    : fallbackPrimaryPaneId && paneIds.includes(fallbackPrimaryPaneId)
      ? fallbackPrimaryPaneId
      : paneIds[0];
  return { primaryPaneId, paneIds };
}

export function loadPaneLayout(
  storage: PaneLayoutStorage,
  session: string,
  activePaneIds: readonly string[],
  fallbackPrimaryPaneId: string | undefined,
): PaneLayout | undefined {
  try {
    const stored = storage.getItem(`${STORAGE_PREFIX}${session}`);
    const candidate = stored ? JSON.parse(stored) as Partial<StoredPaneLayout> : undefined;
    const parsed = candidate?.version === LAYOUT_SCHEMA_VERSION ? candidate : undefined;
    return normalizePaneLayout(activePaneIds, parsed, fallbackPrimaryPaneId);
  } catch {
    return normalizePaneLayout(activePaneIds, undefined, fallbackPrimaryPaneId);
  }
}

export function savePaneLayout(storage: PaneLayoutStorage, session: string, layout: PaneLayout): void {
  try {
    storage.setItem(`${STORAGE_PREFIX}${session}`, JSON.stringify({ version: LAYOUT_SCHEMA_VERSION, ...layout }));
  } catch {
    // A private or full browser store must not prevent terminal interaction.
  }
}

export function promotePane(layout: PaneLayout, paneId: string): PaneLayout {
  if (!layout.paneIds.includes(paneId)) return layout;
  return { primaryPaneId: paneId, paneIds: [paneId, ...layout.paneIds.filter((id) => id !== paneId)] };
}

export function movePane(layout: PaneLayout, paneId: string, direction: -1 | 1): PaneLayout {
  if (paneId === layout.primaryPaneId) return layout;
  const secondaryPaneIds = layout.paneIds.filter((id) => id !== layout.primaryPaneId);
  const from = secondaryPaneIds.indexOf(paneId);
  const to = from + direction;
  if (from < 0 || to < 0 || to >= secondaryPaneIds.length) return layout;
  [secondaryPaneIds[from], secondaryPaneIds[to]] = [secondaryPaneIds[to], secondaryPaneIds[from]];
  return { ...layout, paneIds: [layout.primaryPaneId, ...secondaryPaneIds] };
}

export function orderedPaneIds(layout: PaneLayout): string[] {
  return [layout.primaryPaneId, ...layout.paneIds.filter((id) => id !== layout.primaryPaneId)];
}
