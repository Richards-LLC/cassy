import { escapeHtml } from "./cloud-brand";

/**
 * The supervisor composer, exactly as the app renders it. The fixtures build
 * their composer from this same function so visual QA measures the production
 * controls — including the mic — rather than a hand-copied string (D11).
 */
export const MIC_GLYPH = '<svg class="mic-glyph" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"><rect x="8" y="3" width="8" height="12" rx="4"></rect><path d="M5 11a7 7 0 0 0 14 0M12 18v3M9 21h6"></path></svg>';

export function composerMarkup(supervisor: string | undefined, threadMarkup = ""): string {
  return `<div class="message"><h2><label for="message-text">Talk to ${escapeHtml(supervisor ?? "supervisor")}</label></h2>${threadMarkup}<textarea aria-describedby="message-status" id="message-text" placeholder="Speak or type a message, then review it before sending"></textarea><p class="control-disabled-reason" role="note" hidden></p><div class="composer-actions"><button id="message-mic" type="button" disabled data-mic-state="checking" aria-label="Checking voice input" aria-description="Checking voice input support…" title="Checking voice input support…" aria-pressed="false">${MIC_GLYPH}</button><button id="message-keyboard" type="button">Keyboard</button><button id="message-send" class="primary">Send message</button></div><p id="message-status" class="message-status" role="status" hidden></p><p id="message-delivery" class="message-delivery" role="status" hidden></p></div>`;
}

/** What the mic knows: still detecting, typing only, or dictation available. */
export interface MicState {
  /** "checking" while detection runs; "typing" when dictation is unavailable. */
  mode: "checking" | "typing" | "speech";
  listening: boolean;
  /** The dictation controller's last detail (a permission or error sentence). */
  detail: string;
}

export const CHECKING_VOICE_INPUT = "Checking voice input support…";
export const VOICE_INPUT_UNSUPPORTED = "Voice input is not supported in this browser. Type your message instead.";

/** While the mic listens the empty field says so, even with motion reduced (P6). */
export const LISTENING_PLACEHOLDER = "Listening — speak, then review";

/** The four things the mic can be; `data-mic-state` carries it for CSS and tests. */
export type MicPresentation = "checking" | "unavailable" | "idle" | "listening";

export function micPresentation(state: MicState): MicPresentation {
  if (state.mode === "checking") return "checking";
  if (state.mode === "typing") return "unavailable";
  return state.listening ? "listening" : "idle";
}

const MIC_LABEL: Record<MicPresentation, string> = {
  checking: "Checking voice input",
  unavailable: "Voice input unavailable",
  idle: "Start listening",
  listening: "Stop listening",
};

/**
 * Paint the mic button for a state; the one place its label, title, pressed
 * state and the field's listening placeholder are decided. Checking and
 * unavailable are quiet (a dashed neutral ring, never the warning amber);
 * listening is the critical fill with a halo, never the Send accent (P6).
 */
export function applyMicState(mic: HTMLButtonElement, state: MicState): void {
  const presentation = micPresentation(state);
  const unavailable = presentation === "checking" || presentation === "unavailable";
  const listening = presentation === "listening";
  const reason = presentation === "checking" ? CHECKING_VOICE_INPUT : state.detail || VOICE_INPUT_UNSUPPORTED;
  mic.disabled = unavailable;
  mic.dataset.micState = presentation;
  mic.classList.toggle("listening", listening);
  mic.setAttribute("aria-pressed", String(listening));
  mic.setAttribute("aria-label", MIC_LABEL[presentation]);
  mic.title = unavailable ? reason : listening ? "Stop listening" : state.detail || "Start listening";
  if (unavailable || state.detail) mic.setAttribute("aria-description", reason);
  else mic.removeAttribute("aria-description");
  const field = mic.closest(".message")?.querySelector<HTMLTextAreaElement>("#message-text");
  if (!field) return;
  if (listening) {
    // Remember the resting placeholder (dressComposer may have re-set it on a re-render).
    if (field.placeholder !== LISTENING_PLACEHOLDER) field.dataset.restingPlaceholder = field.placeholder;
    field.placeholder = LISTENING_PLACEHOLDER;
  } else if (field.dataset.restingPlaceholder !== undefined) {
    if (field.placeholder === LISTENING_PLACEHOLDER) field.placeholder = field.dataset.restingPlaceholder;
    delete field.dataset.restingPlaceholder;
  }
}
