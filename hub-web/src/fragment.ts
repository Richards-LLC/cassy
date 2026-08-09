export interface PairingFragment {
  token: string;
  hubId: string;
}

/** Consume the one-time capability before this module permits any network work. */
export function consumePairingFragment(location: Location, history: History): PairingFragment | null {
  const params = new URLSearchParams(location.hash.startsWith("#") ? location.hash.slice(1) : "");
  const token = params.get("pair");
  const hubId = params.get("hub");
  if (!token || !hubId) return null;
  history.replaceState(null, "", `${location.pathname}${location.search}`);
  return { token, hubId };
}
