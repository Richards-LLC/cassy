import { parseGrantedScopes } from "./pairing-scopes";
import type { Scope } from "./types";

export interface PairingFragment {
  token: string;
  hubId: string;
  /** Scope ceiling declared by `cas hub pair`; absent on links minted before it. */
  scopes?: readonly Scope[];
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
  saveLegacy(token: string, hubId: string, scopes?: readonly Scope[]): T;
}

/**
 * What reading the URL produced, with nothing secret in it. `invalid` is a
 * `#pair=` fragment that could not be a capability: the fragment is scrubbed
 * exactly as a valid one is, and the only thing that survives is the fact that
 * the operator opened a broken link and should get a fresh one (F6).
 */
export type PairingFragmentOutcome<T extends PairingFragment> =
  | { readonly kind: "none" }
  | { readonly kind: "invalid" }
  | { readonly kind: "fragment"; readonly fragment: T };

/** Consume the one-time capability before this module permits any network work. */
export function readPairingFragment<T extends PairingFragment>(location: Location, history: History, store: LegacyPairingStore<T>): PairingFragmentOutcome<T>;
export function readPairingFragment(location: Location, history: History): PairingFragmentOutcome<PairingFragment>;
export function readPairingFragment<T extends PairingFragment>(location: Location, history: History, store?: LegacyPairingStore<T>): PairingFragmentOutcome<T> | PairingFragmentOutcome<PairingFragment> {
  const params = new URLSearchParams(location.hash.startsWith("#") ? location.hash.slice(1) : "");
  if (!params.has("pair")) return { kind: "none" };
  try {
    const token = params.get("pair");
    const hubId = params.get("hub");
    if (params.getAll("pair").length !== 1 || params.getAll("hub").length !== 1 || !token || !hubId || !BASE64URL_32_BYTES.test(token) || hubId.length > 128) return { kind: "invalid" };
    // An unreadable scope list leaves the ceiling unknown rather than voiding a
    // usable invitation; the form then falls back to read-only preselection.
    const scopes: readonly Scope[] | undefined = params.getAll("scopes").length === 1 ? parseGrantedScopes(params.get("scopes")) : undefined;
    const fragment = store?.saveLegacy(token, hubId, scopes) ?? { token, hubId, ...(scopes ? { scopes } : {}) };
    return { kind: "fragment", fragment } as PairingFragmentOutcome<T>;
  } finally {
    history.replaceState(null, "", `${location.pathname}${location.search}`);
  }
}

export function consumePairingFragment<T extends PairingFragment>(location: Location, history: History, store: LegacyPairingStore<T>): T | null;
export function consumePairingFragment(location: Location, history: History): PairingFragment | null;
export function consumePairingFragment<T extends PairingFragment>(location: Location, history: History, store?: LegacyPairingStore<T>): T | PairingFragment | null {
  const outcome = store ? readPairingFragment(location, history, store) : readPairingFragment(location, history);
  return outcome.kind === "fragment" ? outcome.fragment : null;
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
  onInvalid?: () => void,
): () => void {
  const check = (): void => {
    const outcome = readPairingFragment(target.location, target.history, store);
    if (outcome.kind === "fragment") onFragment(outcome.fragment);
    else if (outcome.kind === "invalid") onInvalid?.();
  };
  for (const event of FRAGMENT_EVENTS) target.addEventListener(event, check);
  return () => {
    for (const event of FRAGMENT_EVENTS) target.removeEventListener(event, check);
  };
}
