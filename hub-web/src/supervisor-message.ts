import type { HubSession, Scope } from "./types";

export function supervisorTarget(session: HubSession | undefined): string | undefined {
  const target = session?.supervisor.trim();
  return target || undefined;
}

/** `inReplyTo` is the notification_id of the supervisor ask this message answers. */
export function supervisorMessage(target: string, text: string, clientRef?: string, inReplyTo?: number): Record<string, unknown> {
  return {
    SendMessage: {
      ...(clientRef ? { client_ref: clientRef } : {}),
      ...(inReplyTo === undefined ? {} : { in_reply_to: inReplyTo }),
      target,
      text,
      summary: "Cassy Cloud message",
      urgent: false,
      attribution: {
        device_id: null,
        credential_id: null,
        device_label: null,
        operator_label: null,
        controller_origin: null,
        request_id: null,
      },
    },
  };
}

export interface ComposerKeyEvent {
  readonly key: string;
  readonly shiftKey: boolean;
  readonly altKey: boolean;
  readonly ctrlKey: boolean;
  readonly metaKey: boolean;
  readonly isComposing: boolean;
}

/**
 * Enter sends, Shift+Enter writes a second line. The IME guard matters on the
 * device this composer exists for: the first Enter of a Japanese or Pinyin
 * composition commits the candidate, and sending there truncates the sentence.
 */
export function sendsOnEnter(event: ComposerKeyEvent): boolean {
  if (event.key !== "Enter") return false;
  if (event.isComposing) return false;
  return !event.shiftKey && !event.altKey && !event.ctrlKey && !event.metaKey;
}

export type SupervisorSendBlock =
  | "empty"
  | "no-session"
  | "no-supervisor"
  | "checking-hub"
  | "unsupported-hub"
  | "missing-scope";

export type SupervisorSendPlan =
  | { kind: "send" }
  | { kind: "blocked"; block: SupervisorSendBlock; reason: string };

export interface SupervisorSendContext {
  readonly text: string;
  readonly machineLabel: string | undefined;
  readonly session: string | undefined;
  readonly supervisor: string | undefined;
  readonly daemonAttach: boolean | undefined;
  readonly scopes: readonly Scope[];
  readonly leaseHeldByMe: boolean;
  readonly leaseControllerLabel: string | undefined;
}

/**
 * The tone a blocked send's reason is shown in. A pairing that can only read
 * is a standing fact, said the same way before and after a tap on Send
 * (cas-a56c, QA F02); every other block is a refusal of this attempt.
 */
export function sendBlockTone(block: SupervisorSendBlock): "info" | "error" {
  return block === "missing-scope" ? "info" : "error";
}

/**
 * Every refusal the hub can hand back for a supervisor message, decided before
 * the send so the composer can print the actual reason. A disabled button that
 * explains nothing is why an operator concludes the feature is broken: the hub
 * requires the message:send scope, independently of terminal input control.
 */
export function planSupervisorSend(context: SupervisorSendContext): SupervisorSendPlan {
  // Structural refusals come first: they are evaluated to label the Send button
  // while the composer is still empty, and "type a message first" would hide
  // the fact that this device is refused whatever it types.
  if (!context.machineLabel || !context.session) {
    return { kind: "blocked", block: "no-session", reason: "Choose a paired machine and a live session before sending a message." };
  }
  if (context.daemonAttach === undefined) {
    return { kind: "blocked", block: "checking-hub", reason: "Checking this machine's message support. Commander retries when the connection returns." };
  }
  if (!context.daemonAttach) {
    return {
      kind: "blocked",
      block: "unsupported-hub",
      reason: "This hub is too old to accept Cassy Cloud messages. Upgrade the hub, then reconnect this machine.",
    };
  }
  if (!context.supervisor) {
    return { kind: "blocked", block: "no-supervisor", reason: "This session has no supervisor to message." };
  }
  if (!context.scopes.includes("message-send")) {
    return {
      kind: "blocked",
      block: "missing-scope",
      // `cas hub pair` defaults to read-only scopes, so a device paired by the
      // documented recipe is refused by the hub. The person holding it is often
      // not the one who runs the machine (cas-b7c0): say what to ask for, in
      // plain words, never a scope name or a command they cannot run here.
      reason: `This browser can read this conversation but not send messages. Ask whoever runs ${context.machineLabel} for a link that can send, then open it here.`,
    };
  }
  if (context.text.trim().length === 0) {
    return { kind: "blocked", block: "empty", reason: "Type a message before sending it to the supervisor." };
  }
  return { kind: "send" };
}
