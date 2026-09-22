import { escapeHtml } from "./cloud-brand";

/**
 * The supervisor composer, exactly as the app renders it. The fixtures build
 * their composer from this same function so visual QA measures the production
 * controls — including the mic — rather than a hand-copied string (D11).
 */
export const MIC_GLYPH = '<svg class="mic-glyph" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"><rect x="8" y="3" width="8" height="12" rx="4"></rect><path d="M5 11a7 7 0 0 0 14 0M12 18v3M9 21h6"></path></svg>';

export function composerMarkup(supervisor: string | undefined, threadMarkup = ""): string {
  return `<div class="message"><h2><label for="message-text">Talk to ${escapeHtml(supervisor ?? "supervisor")}</label></h2>${threadMarkup}<textarea aria-describedby="message-status" id="message-text" placeholder="Speak or type a message, then review it before sending"></textarea><p class="control-disabled-reason" role="note" hidden></p><div class="composer-actions"><button id="message-mic" type="button" disabled aria-label="Voice input unavailable" aria-description="Checking voice input support…" title="Checking voice input support…" aria-pressed="false">${MIC_GLYPH}</button><button id="message-keyboard" type="button">Keyboard</button><button id="message-send" class="primary">Send message</button></div><p id="message-status" class="message-status" role="status" hidden></p><p id="message-delivery" class="message-delivery" role="status" hidden></p></div>`;
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

/** Paint the mic button for a state; the one place its label, title and pressed state are decided. */
export function applyMicState(mic: HTMLButtonElement, state: MicState): void {
  const unavailable = state.mode !== "speech";
  const listening = state.listening;
  const reason = state.mode === "checking" ? CHECKING_VOICE_INPUT : state.detail || VOICE_INPUT_UNSUPPORTED;
  mic.disabled = unavailable;
  mic.classList.toggle("listening", listening);
  mic.setAttribute("aria-pressed", String(listening));
  const label = unavailable ? "Voice input unavailable" : listening ? "Stop listening" : "Start listening";
  mic.setAttribute("aria-label", label);
  mic.title = unavailable ? reason : listening ? "Stop listening" : state.detail || "Start listening";
  if (unavailable || state.detail) mic.setAttribute("aria-description", reason);
  else mic.removeAttribute("aria-description");
}
