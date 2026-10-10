/**
 * Which fleet operation needs which scope, and what a control says when the
 * scope is missing (cas-d382, fleet-operations brief S4). The one source of
 * truth for gating: End session uses it today, and the operations menu (S5)
 * reads it for every item.
 */

import {
  FACTORY_MANAGE_CAPABILITY,
  FACTORY_MANAGE_SCOPE,
  FACTORY_OPERATE_CAPABILITY,
  FACTORY_OPERATE_SCOPE,
  canEnableFactoryOperate,
  scopeGrantCommand,
  scopeLabel,
} from "./pairing-scopes";
import type { Scope } from "./types";

/** The operations in the fleet-operations brief (O1 to O8). */
export type FleetOperation =
  | "ask-merge"
  | "focus-epic"
  | "add-workers"
  | "pause-worker"
  | "assign-task"
  | "restart-worker"
  | "stop-worker"
  | "end-session"
  /** cas-ab04: grant or revoke a task's write access outside its worktree. */
  | "write-access";

/**
 * Operator decision: Stop, Restart and End session are destructive and need
 * factory:manage; the rest need factory:operate; asking the supervisor to
 * merge is an ordinary message.
 */
const OPERATION_SCOPE: Readonly<Record<FleetOperation, Scope>> = {
  "ask-merge": "message-send",
  "focus-epic": FACTORY_OPERATE_SCOPE,
  "add-workers": FACTORY_OPERATE_SCOPE,
  "pause-worker": FACTORY_OPERATE_SCOPE,
  "assign-task": FACTORY_OPERATE_SCOPE,
  "restart-worker": FACTORY_MANAGE_SCOPE,
  "stop-worker": FACTORY_MANAGE_SCOPE,
  "end-session": FACTORY_MANAGE_SCOPE,
  "write-access": FACTORY_MANAGE_SCOPE,
};

export function fleetOperationScope(operation: FleetOperation): Scope {
  return OPERATION_SCOPE[operation];
}

/** The state a missing scope is labelled with, in words, beside its command. */
export const NOT_ALLOWED_ON_PAIRING = "Not allowed on this pairing";

export type FleetControlGate =
  | { readonly allowed: true; readonly scope: Scope }
  | {
    readonly allowed: false;
    readonly scope: Scope;
    /** The row's state in words. */
    readonly state: string;
    /** Why the control is unavailable, for its description. */
    readonly reason: string;
    /** This browser may allow it once itself ("Allow managing workers"); otherwise only the command does. */
    readonly grantable: boolean;
    /** The exact command whose link adds the scope to this pairing. */
    readonly command: string;
  };

const PERMISSION_NAME: Partial<Record<Scope, string>> = {
  [FACTORY_OPERATE_SCOPE]: FACTORY_OPERATE_CAPABILITY,
  [FACTORY_MANAGE_SCOPE]: FACTORY_MANAGE_CAPABILITY,
  "message-send": "Send messages to supervisors",
};

/** Whether this pairing may run the operation, and if not, what the control says and which command grants it. */
export function fleetControlGate(scopes: readonly Scope[], operation: FleetOperation, controllerOrigin: string): FleetControlGate {
  const scope = fleetOperationScope(operation);
  if (scopes.includes(scope)) return { allowed: true, scope };
  const grantable = scope === FACTORY_OPERATE_SCOPE && canEnableFactoryOperate(scopes);
  return {
    allowed: false,
    scope,
    state: NOT_ALLOWED_ON_PAIRING,
    // cas-97d58 F06: name the permission by what it allows; the scope id
    // stays in the grant command, never in visible copy.
    reason: `Needs the ${PERMISSION_NAME[scope] ?? scopeLabel(scope)} permission.`,
    grantable,
    command: scopeGrantCommand(controllerOrigin, scopes, scope),
  };
}
