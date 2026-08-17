import { describe, expect, it, vi } from "vitest";
import { detectSpeechInput, SpeechDictationController, type SpeechRecognitionConstructorLike, type SpeechRecognitionLike } from "./speech-input";

function recognitionConstructor(statics: Partial<SpeechRecognitionConstructorLike> = {}): SpeechRecognitionConstructorLike {
  class Recognition implements SpeechRecognitionLike {
    continuous = true;
    interimResults = false;
    lang = "";
    processLocally?: boolean;
    onresult = null;
    onerror = null;
    onend = null;
    start(): void {}
    stop(): void {}
  }
  return Object.assign(Recognition, statics);
}

describe("mobile speech input detection", () => {
  it("prefers an available on-device recognizer", async () => {
    const standard = recognitionConstructor({ available: vi.fn(async () => "available") });
    const webkit = recognitionConstructor();
    const result = await detectSpeechInput({ secureContext: true, language: "en-US", standard, webkit });
    expect(result.mode).toBe("local");
    const recognition = result.create?.();
    expect(recognition).toBeInstanceOf(standard);
  });

  it("bounds a stuck local model install and falls back to webkit cloud recognition", async () => {
    vi.useFakeTimers();
    const standard = recognitionConstructor({
      available: vi.fn(async () => "downloading"),
      install: vi.fn(() => new Promise<boolean>(() => undefined)),
    });
    const webkit = recognitionConstructor();
    const pending = detectSpeechInput({ secureContext: true, language: "en-US", standard, webkit }, 25);
    await vi.advanceTimersByTimeAsync(25);
    const result = await pending;
    expect(result.mode).toBe("cloud");
    expect(result.create?.()).toBeInstanceOf(webkit);
    vi.useRealTimers();
  });

  it("uses the Chrome Android webkit engine when local models are unavailable", async () => {
    const standard = recognitionConstructor({ available: vi.fn(async () => "unavailable") });
    const webkit = recognitionConstructor();
    const result = await detectSpeechInput({ secureContext: true, language: "en-US", standard, webkit });
    expect(result.mode).toBe("cloud");
    expect(result.create?.()).toBeInstanceOf(webkit);
  });

  it("hides voice input on insecure or unsupported origins", async () => {
    await expect(detectSpeechInput({ secureContext: false, language: "en-US", webkit: recognitionConstructor() }))
      .resolves.toMatchObject({ mode: "typing" });
    await expect(detectSpeechInput({ secureContext: true, language: "en-US" }))
      .resolves.toMatchObject({ mode: "typing" });
  });
});

describe("speech dictation", () => {
  it("streams interim text into the editable draft and leaves sending explicit", () => {
    let recognition: SpeechRecognitionLike | undefined;
    const states: string[] = [];
    const writes: Array<[string, boolean]> = [];
    const controller = new SpeechDictationController({
      mode: "cloud",
      language: "en-US",
      create: () => {
        recognition = new (recognitionConstructor())();
        return recognition;
      },
    }, {
      read: () => "Existing draft",
      write: (value, interim) => writes.push([value, interim]),
      state: (state) => states.push(state),
      permissionDenied: vi.fn(),
    });

    controller.start();
    recognition?.onresult?.({
      results: [{ 0: { transcript: " dictated words" }, isFinal: false }],
    } as unknown as Event & { results: ArrayLike<{ readonly isFinal: boolean; readonly 0: { readonly transcript: string } }> });

    expect(writes).toEqual([["Existing draft dictated words", true]]);
    expect(states).toEqual(["listening"]);
    expect(controller.listening).toBe(true);
  });

  it("degrades permission denial without replacing it with a generic end state", () => {
    let recognition: SpeechRecognitionLike | undefined;
    const states: string[] = [];
    const denied = vi.fn();
    const controller = new SpeechDictationController({
      mode: "cloud",
      language: "en-US",
      create: () => {
        recognition = new (recognitionConstructor())();
        return recognition;
      },
    }, {
      read: () => "",
      write: vi.fn(),
      state: (state) => states.push(state),
      permissionDenied: denied,
    });

    controller.start();
    recognition?.onerror?.({ error: "not-allowed" } as unknown as Event & { error: string });
    recognition?.onend?.();

    expect(denied).toHaveBeenCalledOnce();
    expect(states).toEqual(["listening"]);
    expect(controller.listening).toBe(false);
  });
});
