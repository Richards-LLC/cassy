import type { Scope } from "./types";

export interface PairingDraft {
  hubUrl: string;
  machineLabel: string;
  deviceLabel: string;
  operatorLabel: string;
  scopes: Scope[];
  email: string;
}

const DEFAULT_SCOPES: Scope[] = ["machine-read", "session-read", "pane-read", "pane-input", "message-send", "pane-interrupt"];

export function createPairingDraft(controllerOrigin: string): PairingDraft {
  return {
    hubUrl: controllerOrigin,
    machineLabel: "",
    deviceLabel: "Commander browser",
    operatorLabel: "",
    scopes: [...DEFAULT_SCOPES],
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
