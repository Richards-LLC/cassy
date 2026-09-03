import { parseGrantedScopes } from "./pairing-scopes";
import type { Scope } from "./types";

export interface PairingFragment {
  token: string;
  hubId: string;
  /** Scope ceiling declared by `cas hub pair`; absent on links minted before it. */
  scopes?: readonly Scope[];
}

const BASE64URL_32_BYTES = /^[A-Za-z0-9_-]{43}$/;

interface LegacyPairingStore<T extends PairingFragment> {
  saveLegacy(token: string, hubId: string, scopes?: readonly Scope[]): T;
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
    // An unreadable scope list leaves the ceiling unknown rather than voiding a
    // usable invitation; the form then falls back to read-only preselection.
    const scopes: readonly Scope[] | undefined = params.getAll("scopes").length === 1 ? parseGrantedScopes(params.get("scopes")) : undefined;
    return store?.saveLegacy(token, hubId, scopes) ?? { token, hubId, ...(scopes ? { scopes } : {}) };
  } finally {
    history.replaceState(null, "", `${location.pathname}${location.search}`);
  }
}
