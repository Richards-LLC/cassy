/**
 * Commander write grants (cas-ab04, GH #1169 part 2). From a paired device
 * holding factory:manage, the operator grants agents working on one task
 * write access to a folder outside their worktree until the task closes, or
 * revokes it. The hub authenticates the device and itself records the grant;
 * this module is the DOM-free form state.
 *
 * Wire: POST /v1/sessions/{s}/write-grants {action:"grant", task, path,
 * mode, reason} → {grant}; {action:"revoke", task} → {removed}.
 */

export type WriteMode = "create" | "edit" | "delete";
export const WRITE_MODES: readonly WriteMode[] = ["create", "edit", "delete"];

export interface WriteGrantDraft {
  task: string;
  path: string;
  modes: Set<WriteMode>;
  reason: string;
}

export type WriteGrantStage = "editing" | "confirm-grant" | "confirm-revoke" | "sending";

export interface WriteGrantResult {
  readonly tone: "ok" | "error";
  readonly text: string;
}

export type WriteGrantBody = Readonly<Record<string, unknown>>;
export type WriteGrantSend = (body: WriteGrantBody) => Promise<Readonly<Record<string, unknown>> | undefined>;

export class WriteGrantState {
  draft: WriteGrantDraft = { task: "", path: "", modes: new Set(["create", "edit"]), reason: "" };
  stage: WriteGrantStage = "editing";
  result: WriteGrantResult | undefined;

  reset(_tasks: readonly { readonly id: string }[]): void {}
  validate(): string | undefined { return undefined; }
  review(): boolean { return false; }
  askRevoke(): void {}
  cancel(): void {}
  modeText(): string { return ""; }
  question(): string { return ""; }
  grantBody(): WriteGrantBody { return {}; }
  revokeBody(): WriteGrantBody { return {}; }
}

export async function sendWriteGrant(_state: WriteGrantState, _send: WriteGrantSend, _kind: "grant" | "revoke"): Promise<void> {}
