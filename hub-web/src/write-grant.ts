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

import { AuthenticationError, HubRequestError } from "./connection";
import { FACTORY_MANAGE_CAPABILITY } from "./pairing-scopes";

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
  /** The task a receipt names, drawn unbroken (cas-06e8). */
  readonly task?: string;
}

export type WriteGrantBody = Readonly<Record<string, unknown>>;
export type WriteGrantSend = (body: WriteGrantBody) => Promise<Readonly<Record<string, unknown>> | undefined>;

const PATH_PROMPT = "Enter the folder to grant, as an absolute path or ~/…";

export class WriteGrantState {
  draft: WriteGrantDraft = { task: "", path: "", modes: new Set(["create", "edit"]), reason: "" };
  stage: WriteGrantStage = "editing";
  result: WriteGrantResult | undefined;

  /** A fresh form on the first listed task, with the default create+edit. */
  reset(tasks: readonly { readonly id: string }[]): void {
    this.draft = { task: tasks[0]?.id ?? "", path: "", modes: new Set(["create", "edit"]), reason: "" };
    this.stage = "editing";
    this.result = undefined;
  }

  /** The first thing missing, in words, or undefined when the grant is complete. */
  validate(): string | undefined {
    if (!this.draft.task) return "Choose the task the grant is for.";
    const path = this.draft.path.trim();
    if (!(path.startsWith("/") || path.startsWith("~/"))) return PATH_PROMPT;
    if (!this.draft.reason.trim()) return "Say why: the reason is recorded on the task.";
    if (this.draft.modes.size === 0) return "Choose at least one of create, edit or delete.";
    return undefined;
  }

  /** Open the confirmation, or show what is missing. */
  review(): boolean {
    const problem = this.validate();
    if (problem) {
      this.result = { tone: "error", text: problem };
      return false;
    }
    this.result = undefined;
    this.stage = "confirm-grant";
    return true;
  }

  askRevoke(): void {
    if (!this.draft.task) {
      this.result = { tone: "error", text: "Choose the task the grant is for." };
      return;
    }
    this.result = undefined;
    this.stage = "confirm-revoke";
  }

  cancel(): void {
    this.stage = "editing";
  }

  modeText(): string {
    return WRITE_MODES.filter((mode) => this.draft.modes.has(mode)).join("+");
  }

  question(): string {
    return this.stage === "confirm-revoke"
      ? `Revoke every write grant for ${this.draft.task}?`
      : `Grant agents on ${this.draft.task} ${this.modeText()} in ${this.draft.path.trim()} until the task closes?`;
  }

  grantBody(): WriteGrantBody {
    return { action: "grant", task: this.draft.task, path: this.draft.path.trim(), mode: this.modeText(), reason: this.draft.reason.trim() };
  }

  revokeBody(): WriteGrantBody {
    return { action: "revoke", task: this.draft.task };
  }
}

// cas-a217: write grants need factory:manage; name that permission, not
// factory:operate's "managing workers".
const MISSING_MANAGE = `this pairing lacks the ${FACTORY_MANAGE_CAPABILITY} permission. Add it in Paired machines.`;

function refusal(error: unknown): string {
  // cas-42c0: a 403 arrives as AuthenticationError("scope-mismatch"): the
  // pairing works but lacks factory:manage. Only a lost pairing re-pairs.
  if (error instanceof AuthenticationError) {
    return error.kind === "scope-mismatch"
      ? MISSING_MANAGE
      : "the pairing is no longer accepted. Pair the machine again.";
  }
  if (!(error instanceof HubRequestError)) return "the machine could not be reached. Check its connection and try again.";
  if (error.status === 403) return MISSING_MANAGE;
  if (error.status === 401) return "the pairing is no longer accepted. Pair the machine again.";
  if (error.detail?.trim()) return error.detail.trim();
  return error.status >= 500 ? "the machine returned an error. Try again." : "the machine refused the request.";
}

/** Send the confirmed grant or revoke and record the receipt or refusal. */
export async function sendWriteGrant(state: WriteGrantState, send: WriteGrantSend, kind: "grant" | "revoke"): Promise<void> {
  const body = kind === "grant" ? state.grantBody() : state.revokeBody();
  const task = state.draft.task;
  state.stage = "sending";
  try {
    const answer = await send(body);
    if (kind === "grant") {
      const grant = (answer?.grant ?? {}) as Record<string, unknown>;
      const modes = Array.isArray(grant.modes) ? (grant.modes as string[]).join("+") : state.modeText();
      const path = typeof grant.path === "string" ? grant.path : state.draft.path.trim();
      state.result = { tone: "ok", text: `Write access granted for ${task}: ${path} (${modes}) until the task closes.`, task };
      // cas-06e8: a sent grant leaves no primed form; keep the task and modes.
      state.draft.path = "";
      state.draft.reason = "";
    } else {
      const removed = Number(answer?.removed ?? 0);
      state.result = { tone: "ok", text: `Write access revoked for ${task}: ${removed} ${removed === 1 ? "grant" : "grants"} removed.`, task };
    }
  } catch (error) {
    state.result = { tone: "error", text: `Could not ${kind}: ${refusal(error)}` };
  } finally {
    state.stage = "editing";
  }
}

/**
 * cas-5020: where keyboard focus goes once a result line is shown. Never the
 * status line itself (it drew the browser's default outline through its
 * text): after a send, the receipt's next action (Revoke… after a grant,
 * Review grant after a revoke) or the button to retry a refusal; after a
 * refused review, the control that supplies what is missing. The line is
 * announced through the live region instead.
 */
export function focusAfterResult(state: WriteGrantState, sent?: "grant" | "revoke"): string {
  if (sent && state.result) {
    if (state.result.tone === "ok") return sent === "grant" ? "header:grant-revoke" : "header:grant-review";
    return sent === "grant" ? "header:grant-review" : "header:grant-revoke";
  }
  if (!state.draft.task) return "header:grant-task";
  const path = state.draft.path.trim();
  if (!(path.startsWith("/") || path.startsWith("~/"))) return "header:grant-path";
  if (!state.draft.reason.trim()) return "header:grant-reason";
  if (state.draft.modes.size === 0) return "header:grant-mode:create";
  return "header:grant-review";
}
