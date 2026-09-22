// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { CHECKING_VOICE_INPUT, LISTENING_PLACEHOLDER, VOICE_INPUT_UNSUPPORTED, applyMicState, composerMarkup, micPresentation, type MicState } from "./composer-markup";
import { dressComposer } from "./conversation-shell";

// WCAG 2.x contrast, the same arithmetic as machine-accent.test.ts.
const luminance = (hex: string) => [1, 3, 5].map((o) => Number.parseInt(hex.slice(o, o + 2), 16) / 255).map((c) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4)).reduce((sum, c, i) => sum + c * [0.2126, 0.7152, 0.0722][i], 0);
const contrast = (a: string, b: string) => { const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x); return (hi + 0.05) / (lo + 0.05); };

/** The production composer, dressed the way the conversation shell dresses it. */
function composer(): { mic: HTMLButtonElement; field: HTMLTextAreaElement } {
  const slot = document.createElement("div");
  slot.innerHTML = composerMarkup("patient-pelican-9");
  dressComposer(slot.querySelector<HTMLElement>(".message")!, "patient-pelican-9");
  document.body.replaceChildren(slot);
  return { mic: slot.querySelector<HTMLButtonElement>("#message-mic")!, field: slot.querySelector<HTMLTextAreaElement>("#message-text")! };
}

const CHECKING: MicState = { mode: "checking", listening: false, detail: "" };
const UNAVAILABLE: MicState = { mode: "typing", listening: false, detail: "" };
const IDLE: MicState = { mode: "speech", listening: false, detail: "" };
const LISTENING: MicState = { mode: "speech", listening: true, detail: "" };

describe("mic states say what they mean (P6, cas-0c80)", () => {
  it("boots in the checking state, quiet and labelled as checking", () => {
    const { mic, field } = composer();
    expect(mic.disabled).toBe(true);
    expect(mic.dataset.micState).toBe("checking");
    expect(mic.getAttribute("aria-label")).toBe("Checking voice input");
    expect(mic.getAttribute("aria-description")).toBe(CHECKING_VOICE_INPUT);
    expect(mic.getAttribute("aria-pressed")).toBe("false");
    expect(field.placeholder).toBe("Message patient-pelican-9");
  });

  it.each([
    ["checking", CHECKING, { disabled: true, label: "Checking voice input", pressed: "false", listening: false, description: CHECKING_VOICE_INPUT, title: CHECKING_VOICE_INPUT, placeholder: "Message patient-pelican-9" }],
    ["unavailable", UNAVAILABLE, { disabled: true, label: "Voice input unavailable", pressed: "false", listening: false, description: VOICE_INPUT_UNSUPPORTED, title: VOICE_INPUT_UNSUPPORTED, placeholder: "Message patient-pelican-9" }],
    ["idle", IDLE, { disabled: false, label: "Start listening", pressed: "false", listening: false, description: null, title: "Start listening", placeholder: "Message patient-pelican-9" }],
    ["listening", LISTENING, { disabled: false, label: "Stop listening", pressed: "true", listening: true, description: null, title: "Stop listening", placeholder: LISTENING_PLACEHOLDER }],
  ] as const)("paints %s: class, accessible name and state, and the field placeholder", (name, state, expected) => {
    const { mic, field } = composer();
    applyMicState(mic, state);
    expect(micPresentation(state)).toBe(name);
    expect(mic.dataset.micState).toBe(name);
    expect(mic.disabled).toBe(expected.disabled);
    expect(mic.classList.contains("listening")).toBe(expected.listening);
    expect(mic.getAttribute("aria-label")).toBe(expected.label);
    expect(mic.getAttribute("aria-pressed")).toBe(expected.pressed);
    expect(mic.getAttribute("aria-description")).toBe(expected.description);
    expect(mic.title).toBe(expected.title);
    expect(field.placeholder).toBe(expected.placeholder);
  });

  it("restores the resting placeholder when listening stops, even after a re-dress while listening", () => {
    const { mic, field } = composer();
    applyMicState(mic, LISTENING);
    expect(field.placeholder).toBe(LISTENING_PLACEHOLDER);
    applyMicState(mic, LISTENING);
    expect(field.placeholder).toBe(LISTENING_PLACEHOLDER);
    // A heartbeat re-render re-dresses the composer mid-dictation; the next sync re-applies Listening.
    dressComposer(field.closest<HTMLElement>(".message")!, "patient-pelican-9");
    applyMicState(mic, LISTENING);
    expect(field.placeholder).toBe(LISTENING_PLACEHOLDER);
    applyMicState(mic, IDLE);
    expect(field.placeholder).toBe("Message patient-pelican-9");
    expect(field.dataset.restingPlaceholder).toBeUndefined();
    // A permission denial ends dictation in the unavailable state; the field is restored there too.
    applyMicState(mic, LISTENING);
    applyMicState(mic, { mode: "typing", listening: false, detail: "Mic permission was not granted. Type your message instead." });
    expect(field.placeholder).toBe("Message patient-pelican-9");
    expect(mic.getAttribute("aria-description")).toBe("Mic permission was not granted. Type your message instead.");
  });

  it("keeps the detail sentence on an idle mic after an error", () => {
    const { mic } = composer();
    applyMicState(mic, { mode: "speech", listening: false, detail: "No speech heard — try again or type." });
    expect(mic.dataset.micState).toBe("idle");
    expect(mic.title).toBe("No speech heard — try again or type.");
    expect(mic.getAttribute("aria-description")).toBe("No speech heard — try again or type.");
  });
});

describe("mic state styling (P6)", () => {
  const css = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "styles.css"), "utf8");
  const tokens = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "tokens.css"), "utf8");
  const rule = (selector: string) => {
    const start = css.indexOf(`${selector} {`);
    expect(start, selector).toBeGreaterThanOrEqual(0);
    return css.slice(start, css.indexOf("}", start));
  };

  it("paints listening in the critical fill with a static halo, never the Send accent", () => {
    for (const selector of ["#message-mic.listening", ".conversation-composer #message-mic.listening"]) {
      const body = rule(selector);
      expect(body).toContain("background: var(--crit-bg)");
      expect(body).toContain("var(--crit-fg)");
      expect(body).not.toMatch(/--accent|--you-bg|--color-action/);
    }
    // The halo is the outline itself; reduced motion only stops the breathing.
    expect(rule("#message-mic.listening")).toContain("outline: 4px solid color-mix(in srgb, var(--crit-bg) 28%, transparent)");
    const reduced = css.slice(css.indexOf("@media (prefers-reduced-motion: reduce) {\n  #message-mic.listening"));
    expect(reduced.slice(0, reduced.indexOf("}\n}"))).toContain("#message-mic.listening { animation: none;");
    expect(reduced.slice(0, reduced.indexOf("}\n}"))).not.toContain("outline");
  });

  it("paints checking and unavailable as a quiet dashed ring, never the warning amber", () => {
    for (const selector of ["#message-mic:disabled", ".conversation-composer #message-mic:disabled"]) {
      const body = rule(selector);
      expect(body).toContain("border-style: dashed");
      expect(body).toContain("border-color: var(--line-strong)");
      expect(body).toContain("background: var(--color-transparent)");
      expect(body).not.toMatch(/--warn-text|--state-warn/);
    }
  });

  it("meets text 4.5:1 and marks 3:1 for every mic state in both schemes", () => {
    for (const scheme of ["light", "dark"]) {
      const start = tokens.indexOf(`html[data-scheme="${scheme}"] {`);
      const block = tokens.slice(start, tokens.indexOf("}", start));
      const t = (name: string) => block.match(new RegExp(`\\s${name}: (#[0-9A-Fa-f]{6});`))![1];
      // The composer sits on the canvas.
      expect(contrast(t("--crit-fg"), t("--crit-bg")), `${scheme} listening glyph`).toBeGreaterThanOrEqual(4.5);
      expect(contrast(t("--crit-bg"), t("--canvas")), `${scheme} listening fill`).toBeGreaterThanOrEqual(3);
      expect(contrast(t("--ink-mid"), t("--canvas")), `${scheme} unavailable glyph`).toBeGreaterThanOrEqual(4.5);
      expect(contrast(t("--line-strong"), t("--canvas")), `${scheme} dashed ring`).toBeGreaterThanOrEqual(3);
      expect(contrast(t("--ink-mid"), t("--panel")), `${scheme} idle glyph`).toBeGreaterThanOrEqual(4.5);
    }
  });
});
