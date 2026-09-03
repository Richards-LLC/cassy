import type { HubSession, Scope } from "./types";

export function supervisorTarget(session: HubSession | undefined): string | undefined {
  const target = session?.supervisor.trim();
  return target || undefined;
}

export function supervisorMessage(target: string, text: string): Record<string, unknown> {
  return {
    SendMessage: {
      target,
      text,
      summary: "Cassy Commander message",
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
  | "unsupported-hub"
  | "missing-scope"
  | "controlled-elsewhere";

export type SupervisorSendPlan =
  | { kind: "send" }
  | { kind: "take-control-then-send"; notice: string }
  | { kind: "blocked"; block: SupervisorSendBlock; reason: string };

export interface SupervisorSendContext {
  readonly text: string;
  readonly machineLabel: string | undefined;
  readonly session: string | undefined;
  readonly supervisor: string | undefined;
  readonly daemonAttach: boolean;
  readonly scopes: readonly Scope[];
  readonly leaseHeldByMe: boolean;
  readonly leaseControllerLabel: string | undefined;
  readonly commanderOrigin: string;
}

const CONTROL_PAIRING_SCOPES = "machine:read,session:read,pane:read,pane:input,message:send,pane:interrupt";

/**
 * Every refusal the hub can hand back for a supervisor message, decided before
 * the send so the composer can print the actual reason. A disabled button that
 * explains nothing is why an operator concludes the feature is broken: the hub
 * requires both the message:send scope and this device's session lease, and
 * neither is visible from the button.
 */
export function planSupervisorSend(context: SupervisorSendContext): SupervisorSendPlan {
  // Structural refusals come first: they are evaluated to label the Send button
  // while the composer is still empty, and "type a message first" would hide
  // the fact that this device is refused whatever it types.
  if (!context.machineLabel || !context.session) {
    return { kind: "blocked", block: "no-session", reason: "Choose a paired machine and a live session before sending a message." };
  }
  if (!context.daemonAttach) {
    return {
      kind: "blocked",
      block: "unsupported-hub",
      reason: "This hub is too old to accept Cassy Commander messages. Upgrade the hub, then reconnect this machine.",
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
      // documented recipe is refused by the hub with nothing on screen to say so.
      reason: `This device was paired without the message:send scope, so the hub refuses its messages. Run cas hub pair --origin ${context.commanderOrigin} --scopes ${CONTROL_PAIRING_SCOPES} on ${context.machineLabel}, then open the new pairing URL here.`,
    };
  }
  if (context.text.trim().length === 0) {
    return { kind: "blocked", block: "empty", reason: "Type a message before sending it to the supervisor." };
  }
  if (context.leaseHeldByMe) return { kind: "send" };
  if (context.leaseControllerLabel) {
    return {
      kind: "blocked",
      block: "controlled-elsewhere",
      reason: `${context.leaseControllerLabel} controls this session, and the hub only accepts a message from its controller. Wait for control to be released, or take over with an administrator credential.`,
    };
  }
  // The hub treats a supervisor message as a leased mutation, so observing
  // alone cannot deliver it. Taking control is the operator's own next step;
  // doing it for them beats a button that does nothing.
  return {
    kind: "take-control-then-send",
    notice: `Taking control of ${context.session} to deliver this message…`,
  };
}

/**
 * A render replaces app.innerHTML, so the terminal restore and the composer
 * restore both fire afterwards. The composer must win: a terminal that steals
 * focus back swallows the rest of the sentence being typed.
 */
export function composerFocusWinner(state: { composerWasFocused: boolean; terminalWasFocused: boolean }): "composer" | "terminal" | "none" {
  if (state.composerWasFocused) return "composer";
  return state.terminalWasFocused ? "terminal" : "none";
}
