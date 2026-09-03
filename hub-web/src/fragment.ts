export interface PairingFragment {
  token: string;
  hubId: string;
}

const BASE64URL_32_BYTES = /^[A-Za-z0-9_-]{43}$/;

/** Events that can deliver a pairing URL to a tab that never navigates. */
const FRAGMENT_EVENTS = ["hashchange", "pageshow", "focus", "visibilitychange"] as const;

export interface FragmentWatchTarget {
  readonly location: Location;
  readonly history: History;
  addEventListener(type: string, listener: () => void): void;
  removeEventListener(type: string, listener: () => void): void;
}

interface LegacyPairingStore<T extends PairingFragment> {
  saveLegacy(token: string, hubId: string): T;
}

/** Consume the one-time capability before this module permits any network work. */
export function consumePairingFragment<T extends PairingFragment>(location: Location, history: History, store: LegacyPairingStore<T>): T | null;
export function consumePairingFragment(location: Location, history: History): PairingFragment | null;
export function consumePairingFragment<T extends PairingFragment>(location: Location, history: History, store?: LegacyPairingStore<T>): T | PairingFragment | null {
  const params = new URLSearchParams(location.hash.startsWith("#") ? location.hash.slice(1) : "");
  if (!params.has("pair")) return null;
  try {
    const token = params.get("pair");
    const hubId = params.get("hub");
    if (params.getAll("pair").length !== 1 || params.getAll("hub").length !== 1 || !token || !hubId || !BASE64URL_32_BYTES.test(token) || hubId.length > 128) return null;
    return store?.saveLegacy(token, hubId) ?? { token, hubId };
  } finally {
    history.replaceState(null, "", `${location.pathname}${location.search}`);
  }
}

/**
 * Keep consuming pairing fragments after boot.
 *
 * Android delivers a VIEW intent for a URL that differs only by its `#fragment`
 * to the tab that is already open. Nothing navigates, the SPA never re-inits,
 * and `consumePairingFragment` — which runs once at module load — never sees the
 * invitation. The one-time capability is then dropped in complete silence.
 *
 * Re-reading on hash change and on every return to the foreground covers the
 * intent, the back-forward cache, and a tab switched to from the launcher. The
 * check is safe to repeat: a URL without a `pair` parameter is left untouched,
 * hash and all, and a consumed capability is scrubbed so the next event is a
 * no-op.
 */
export function watchPairingFragment<T extends PairingFragment>(
  target: FragmentWatchTarget,
  store: LegacyPairingStore<T>,
  onFragment: (fragment: T) => void,
): () => void {
  const check = (): void => {
    const fragment = consumePairingFragment(target.location, target.history, store);
    if (fragment) onFragment(fragment);
  };
  for (const event of FRAGMENT_EVENTS) target.addEventListener(event, check);
  return () => {
    for (const event of FRAGMENT_EVENTS) target.removeEventListener(event, check);
  };
}
