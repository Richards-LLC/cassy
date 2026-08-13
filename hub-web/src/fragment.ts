export interface PairingFragment {
  token: string;
  hubId: string;
}

const BASE64URL_32_BYTES = /^[A-Za-z0-9_-]{43}$/;

interface LegacyPairingStore<T extends PairingFragment> {
  saveLegacy(token: string, hubId: string): T;
}

/** Consume the one-time capability before this module permits any network work. */
export function consumePairingFragment<T extends PairingFragment>(location: Location, history: History, store: LegacyPairingStore<T>): T | null;
export function consumePairingFragment(location: Location, history: History): PairingFragment | null;
export function consumePairingFragment<T extends PairingFragment>(location: Location, history: History, store?: LegacyPairingStore<T>): T | PairingFragment | null {
  const params = new URLSearchParams(location.hash.startsWith("#") ? location.hash.slice(1) : "");
  const token = params.get("pair");
  const hubId = params.get("hub");
  if (!token || !hubId) return null;
  try {
    if (params.getAll("pair").length !== 1 || params.getAll("hub").length !== 1 || !BASE64URL_32_BYTES.test(token) || hubId.length > 128) return null;
    return store?.saveLegacy(token, hubId) ?? { token, hubId };
  } finally {
    history.replaceState(null, "", `${location.pathname}${location.search}`);
  }
}
