import type { Scope } from "./types";
import { PAIRING_SCOPES } from "./pairing-scopes";

export interface PairingDraft {
  /** The machine's hub address. Never seeded from the page: a hosted Commander
   *  origin is a plausible-looking wrong answer for a remote machine (F5). */
  hubUrl: string;
  /** Where this page is served from, offered as one tap for the page-served-by-hub case. */
  pageOrigin: string;
  machineLabel: string;
  deviceLabel: string;
  operatorLabel: string;
  scopes: Scope[];
  email: string;
}

/** `scopes` is the invitation's ceiling when one is known, never a wider guess. */
export function createPairingDraft(controllerOrigin: string, scopes?: readonly Scope[]): PairingDraft {
  return {
    hubUrl: "",
    pageOrigin: controllerOrigin,
    machineLabel: "",
    deviceLabel: "Cassy Commander browser",
    operatorLabel: "",
    scopes: scopes ? [...scopes] : [...PAIRING_SCOPES],
    email: "",
  };
}

export function updatePairingDraft(
  current: PairingDraft,
  entries: Iterable<readonly [string, unknown]>,
  captureScopes = false,
): PairingDraft {
  const next = { ...current, scopes: [...current.scopes] };
  const scopes: Scope[] = [];
  let sawEntry = false;
  for (const [name, value] of entries) {
    if (typeof value !== "string") continue;
    sawEntry = true;
    if (name === "url") next.hubUrl = value;
    else if (name === "label") next.machineLabel = value;
    else if (name === "device") next.deviceLabel = value;
    else if (name === "operator") next.operatorLabel = value;
    else if (name === "scope") scopes.push(value as Scope);
  }
  if (captureScopes && sawEntry) next.scopes = scopes;
  return next;
}
