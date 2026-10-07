import type { PendingPairing } from "./pending-pairing";
import type { Scope } from "./types";

/** Every scope a Commander pairing may request, in the order the form lists them. */
export const PAIRING_SCOPES: Scope[] = ["machine-read", "session-read", "pane-read", "pane-input", "message-send", "pane-interrupt"];

/**
 * Starting new sessions on a machine. Never part of a default pairing: it is
 * granted only when the machine's `cas hub pair --scopes` names it, so the
 * form offers it only when an invitation's ceiling includes it.
 */
export const LAUNCH_SCOPE: Scope = "session-launch";

/**
 * Fleet operations (cas-d382, fleet-operations brief S4). factory-operate
 * covers the non-destructive ones (focus an epic, add workers, pause and
 * resume, assign a task) and may be allowed once from this browser, like
 * session launch; factory-manage covers Stop, Restart and End session and
 * is granted only by a pairing invitation the machine's owner mints.
 */
export const FACTORY_OPERATE_SCOPE: Scope = "factory-operate";
export const FACTORY_MANAGE_SCOPE: Scope = "factory-manage";

/**
 * Hub administration (cas-5e53 F08): seeing and revoking this machine's other
 * browser installations, and forcing a takeover. An invitation that names it
 * must not fall back to read-only; the form offers it unticked, so holding it
 * is always the operator's explicit choice.
 */
export const HUB_ADMIN_SCOPE: Scope = "hub-admin";

/** What hub:admin lets a browser do, beside its box. */
export const HUB_ADMIN_CONSENT = "See and revoke this machine's other browser installations";
/** The rest of what hub:admin allows, under its box (cas-d043 G08). */
export const HUB_ADMIN_NOTE = "Also lets this browser take control of a session from another device.";

/** Scopes outside the default pairing that a form offers only when an invitation grants them. */
const OPTIONAL_INVITATION_SCOPES: readonly Scope[] = [LAUNCH_SCOPE, FACTORY_OPERATE_SCOPE, FACTORY_MANAGE_SCOPE, HUB_ADMIN_SCOPE];

/** Granted by an invitation, but held only when the operator ticks it. */
const EXPLICIT_CONSENT_SCOPES: readonly Scope[] = [HUB_ADMIN_SCOPE];

/** Every scope an invitation link may declare that this build understands. */
const KNOWN_INVITATION_SCOPES: readonly Scope[] = [...PAIRING_SCOPES, ...OPTIONAL_INVITATION_SCOPES];

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
  if (parsed.some((scope) => !KNOWN_INVITATION_SCOPES.includes(scope))) return undefined;
  return parsed;
}

/**
 * What the form should arrive pre-ticked with. A scope-aware invitation ticks
 * exactly what it granted; an invitation that predates the declaration falls
 * back to the CLI's read-only default instead of the six that produced a 401.
 */
export function preselectedScopes(pending: PendingPairing | null | undefined): Scope[] {
  if (pending?.kind !== "invitation") return [...PAIRING_SCOPES];
  return pending.scopes ? pending.scopes.filter((scope) => !EXPLICIT_CONSENT_SCOPES.includes(scope)) : [...READ_ONLY_PAIRING_SCOPES];
}

/** What a scope set lets this browser do, in the operator's words (F7). */
export const READ_CAPABILITY = "See its sessions and raw output";
export const CONTROL_CAPABILITY = "Type, send messages and interrupt";
const CONTROL_SCOPES: readonly Scope[] = ["pane-input", "message-send", "pane-interrupt"];

export function canEnableSessionLaunch(scopes: readonly Scope[]): boolean {
  return CONTROL_SCOPES.every((scope) => scopes.includes(scope));
}

/** The one-time "Allow managing workers" grant has the session-launch rule: the device already holds the control scopes. */
export function canEnableFactoryOperate(scopes: readonly Scope[]): boolean {
  return CONTROL_SCOPES.every((scope) => scopes.includes(scope));
}

export const FACTORY_OPERATE_CAPABILITY = "Manage workers and tasks";
export const FACTORY_MANAGE_CAPABILITY = "Stop and restart workers and sessions";

/** One capability per scope, for a grant that is not a whole group. */
const SCOPE_CAPABILITY: Readonly<Record<Scope, string>> = {
  "machine-read": "See this machine",
  "session-read": "See its sessions",
  "pane-read": "Read raw output",
  "pane-input": "Type into its sessions",
  "message-send": "Send messages to supervisors",
  "pane-interrupt": "Interrupt supervisors",
  "session-launch": "Start new sessions",
  "factory-operate": FACTORY_OPERATE_CAPABILITY,
  "factory-manage": FACTORY_MANAGE_CAPABILITY,
  "hub-admin": HUB_ADMIN_CONSENT,
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
  /** What ticking it allows, for a scope that needs explicit consent. */
  note?: string;
}

export function scopeChoices(granted: readonly Scope[] | undefined, selected: readonly Scope[]): ScopeChoice[] {
  // Launch and the fleet scopes are listed only when the invitation grants
  // them: offering a box the machine never grants would only ever render
  // disabled.
  const offered = [...PAIRING_SCOPES, ...OPTIONAL_INVITATION_SCOPES.filter((scope) => granted?.includes(scope))];
  return offered.map((scope) => {
    const allowed = !granted || granted.includes(scope);
    return { scope, label: scopeLabel(scope), granted: allowed, checked: allowed && selected.includes(scope), ...(scope === HUB_ADMIN_SCOPE ? { note: HUB_ADMIN_CONSENT } : {}) };
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

/**
 * The command that re-pairs this browser with its current access plus
 * session launch. Pairing again replaces the machine's credential here, so the
 * new link must carry everything the old one granted.
 */
export function launchGrantCommand(controllerOrigin: string, current: readonly Scope[]): string {
  return scopeGrantCommand(controllerOrigin, current, LAUNCH_SCOPE);
}

/**
 * The command whose link re-pairs this browser with its current access plus
 * one more scope: pairing again replaces the credential, so the link must
 * carry everything the old one granted.
 */
export function scopeGrantCommand(controllerOrigin: string, current: readonly Scope[], scope: Scope): string {
  return pairCommand(controllerOrigin, [...current.filter((held) => held !== scope), scope]);
}

/**
 * cas-0e14 (journey F29): a code pairing requests only the default scopes, so
 * re-pairing by code replaces a credential that could start sessions with one
 * that can't. True when that is what happened.
 */
export function launchDropped(previous: readonly Scope[] | undefined, next: readonly Scope[]): boolean {
  return previous?.includes(LAUNCH_SCOPE) === true && !next.includes(LAUNCH_SCOPE);
}

/**
 * What the Re-pair dialog says before a code is created. When the saved
 * pairing can start sessions, it says the code won't keep that, and names the
 * command whose link would (cas-0e14 F29).
 */
export function repairStatus(label: string, scopes: readonly Scope[]): string {
  const plain = `Re-pairing ${label}: create a new code and approve it on that machine. Its saved access here is replaced when the new credential is installed.`;
  if (!scopes.includes(LAUNCH_SCOPE)) return plain;
  // cas-093d F02: the command is not set as prose (it broke mid-token when the
  // sentence wrapped); it follows as its own copyable code (repairCommand).
  return `Re-pairing ${label}: a new code replaces its saved access here, and starting sessions will need to be allowed again afterwards. To keep it, run this on ${label} and open the link it prints instead:`;
}

/** The command the Re-pair dialog offers to copy, when a code re-pair would drop session launch (cas-093d F02). */
export function repairCommand(scopes: readonly Scope[], controllerOrigin: string): string | undefined {
  return scopes.includes(LAUNCH_SCOPE) ? launchGrantCommand(controllerOrigin, scopes) : undefined;
}

/** The Attention item after a code re-pair dropped session launch (cas-0e14 F29). */
export function launchDroppedNotice(label: string): { headline: string; detail: string } {
  return {
    headline: "Starting sessions needs allowing again",
    detail: `Re-pairing ${label} with a code didn't include starting sessions. Open New session to allow it again.`,
  };
}
