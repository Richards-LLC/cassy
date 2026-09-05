import type { PendingPairing } from "./pending-pairing";
import type { Scope } from "./types";

/** Every scope a Commander pairing may request, in the order the form lists them. */
export const PAIRING_SCOPES: Scope[] = ["machine-read", "session-read", "pane-read", "pane-input", "message-send", "pane-interrupt"];

/** What `cas hub pair` grants when its `--scopes` flag is left at its default. */
export const READ_ONLY_PAIRING_SCOPES: Scope[] = ["machine-read", "session-read", "pane-read"];

/** The colon spelling the CLI prints, from the hyphen spelling the wire uses. */
export function scopeLabel(scope: Scope): string {
  return scope.replaceAll("-", ":");
}

/**
 * Read the ceiling an invitation link declares. A link that names no scopes, or
 * names one this build does not know, leaves the ceiling unknown rather than
 * failing the whole invitation: pre-scope links must still pair.
 */
export function parseGrantedScopes(value: string | null | undefined): Scope[] | undefined {
  if (!value) return undefined;
  const parsed = value.split(",").map((part) => part.trim().replaceAll(":", "-") as Scope);
  if (!parsed.length || new Set(parsed).size !== parsed.length) return undefined;
  if (parsed.some((scope) => !PAIRING_SCOPES.includes(scope))) return undefined;
  return parsed;
}

/**
 * What the form should arrive pre-ticked with. A scope-aware invitation ticks
 * exactly what it granted; an invitation that predates the declaration falls
 * back to the CLI's read-only default instead of the six that produced a 401.
 */
export function preselectedScopes(pending: PendingPairing | null | undefined): Scope[] {
  if (pending?.kind !== "invitation") return [...PAIRING_SCOPES];
  return pending.scopes ? [...pending.scopes] : [...READ_ONLY_PAIRING_SCOPES];
}

/** What a scope set lets this browser do, in the operator's words (F7). */
export const READ_CAPABILITY = "Read sessions and terminals";
export const CONTROL_CAPABILITY = "Type, send messages and interrupt";
const CONTROL_SCOPES: readonly Scope[] = ["pane-input", "message-send", "pane-interrupt"];

/** One capability per scope, for a grant that is not a whole group. */
const SCOPE_CAPABILITY: Readonly<Record<Scope, string>> = {
  "machine-read": "See this machine",
  "session-read": "See its sessions",
  "pane-read": "Read its terminals",
  "pane-input": "Type into terminals",
  "message-send": "Send messages to supervisors",
  "pane-interrupt": "Interrupt panes",
  "factory-manage": "Manage the factory",
  "hub-admin": "Administer the hub",
};

/**
 * A plain summary beside the exact scope list, never instead of it: consent
 * still names each scope and the exact origin, this just says what they add
 * up to. A complete group collapses to one phrase; a partial grant names
 * exactly the capabilities granted and nothing more — a summary that claims
 * "interrupt" for a message-send-only credential is not made honest by the
 * scope list under it.
 */
export function scopeSummary(scopes: readonly Scope[]): string[] {
  const granted = new Set(scopes);
  const summary: string[] = [];
  const group = (members: readonly Scope[], whole: string): void => {
    const present = members.filter((scope) => granted.has(scope));
    if (present.length === members.length) summary.push(whole);
    else for (const scope of present) summary.push(SCOPE_CAPABILITY[scope]);
  };
  group(READ_ONLY_PAIRING_SCOPES, READ_CAPABILITY);
  group(CONTROL_SCOPES, CONTROL_CAPABILITY);
  for (const scope of scopes) {
    if (!READ_ONLY_PAIRING_SCOPES.includes(scope) && !CONTROL_SCOPES.includes(scope)) summary.push(SCOPE_CAPABILITY[scope] ?? `Also ${scopeLabel(scope)}`);
  }
  return summary;
}

export interface ScopeChoice {
  scope: Scope;
  label: string;
  /** Within the invitation's ceiling, so the box may be ticked at all. */
  granted: boolean;
  checked: boolean;
}

export function scopeChoices(granted: readonly Scope[] | undefined, selected: readonly Scope[]): ScopeChoice[] {
  return PAIRING_SCOPES.map((scope) => {
    const allowed = !granted || granted.includes(scope);
    return { scope, label: scopeLabel(scope), granted: allowed, checked: allowed && selected.includes(scope) };
  });
}

export function ungrantedScopes(granted: readonly Scope[] | undefined): Scope[] {
  if (!granted) return [];
  return PAIRING_SCOPES.filter((scope) => !granted.includes(scope));
}

/** The exact command that mints an invitation with these scopes. */
export function pairCommand(controllerOrigin: string, scopes: readonly Scope[]): string {
  return `cas hub pair --origin ${controllerOrigin} --scopes ${scopes.map(scopeLabel).join(",")}`;
}
