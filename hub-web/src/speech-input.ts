export type SpeechInputMode = "local" | "cloud" | "typing";
export type SpeechInputState = "idle" | "listening" | "error";

interface SpeechRecognitionResultLike {
  readonly isFinal: boolean;
  readonly 0: { readonly transcript: string };
}

interface SpeechRecognitionEventLike extends Event {
  readonly results: ArrayLike<SpeechRecognitionResultLike>;
}

interface SpeechRecognitionErrorEventLike extends Event {
  readonly error: string;
}

export interface SpeechRecognitionLike {
  continuous: boolean;
  interimResults: boolean;
  lang: string;
  processLocally?: boolean;
  onresult: ((event: SpeechRecognitionEventLike) => void) | null;
  onerror: ((event: SpeechRecognitionErrorEventLike) => void) | null;
  onend: (() => void) | null;
  start(): void;
  stop(): void;
}

export interface SpeechRecognitionConstructorLike {
  new (): SpeechRecognitionLike;
  available?(options: SpeechRecognitionAvailabilityOptions): Promise<string>;
  install?(options: SpeechRecognitionAvailabilityOptions): Promise<boolean>;
}

interface SpeechRecognitionAvailabilityOptions {
  langs: string[];
  processLocally: true;
}

export interface SpeechEnvironment {
  secureContext: boolean;
  language: string;
  standard?: SpeechRecognitionConstructorLike;
  webkit?: SpeechRecognitionConstructorLike;
}

export interface SpeechInputCapability {
  mode: SpeechInputMode;
  language: string;
  create?: () => SpeechRecognitionLike;
}

const MODEL_CHECK_TIMEOUT_MS = 1_500;

function browserSpeechEnvironment(): SpeechEnvironment {
  const speechWindow = window as typeof window & {
    SpeechRecognition?: SpeechRecognitionConstructorLike;
    webkitSpeechRecognition?: SpeechRecognitionConstructorLike;
  };
  return {
    secureContext: window.isSecureContext,
    language: navigator.language || "en-US",
    standard: speechWindow.SpeechRecognition,
    webkit: speechWindow.webkitSpeechRecognition,
  };
}

async function bounded<T>(promise: Promise<T>, timeoutMs: number, fallback: T): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      promise.catch(() => fallback),
      new Promise<T>((resolve) => { timer = setTimeout(() => resolve(fallback), timeoutMs); }),
    ]);
  } finally {
    if (timer !== undefined) clearTimeout(timer);
  }
}

/**
 * Prefer the emerging local recognizer when its language model is genuinely
 * ready. Chromium Android currently reports it unavailable, so the working
 * webkit cloud recognizer remains the deliberate v1 fallback.
 */
export async function detectSpeechInput(
  environment = browserSpeechEnvironment(),
  timeoutMs = MODEL_CHECK_TIMEOUT_MS,
): Promise<SpeechInputCapability> {
  const typing = { mode: "typing", language: environment.language } as const;
  if (!environment.secureContext) return typing;

  const standard = environment.standard;
  if (standard?.available) {
    const options: SpeechRecognitionAvailabilityOptions = {
      langs: [environment.language],
      processLocally: true,
    };
    let availability = await bounded(standard.available(options), timeoutMs, "unavailable");
    if ((availability === "downloadable" || availability === "downloading") && standard.install) {
      const installed = await bounded(standard.install(options), timeoutMs, false);
      if (installed) {
        availability = await bounded(standard.available(options), timeoutMs, "unavailable");
      }
    }
    if (availability === "available") {
      return { mode: "local", language: environment.language, create: () => new standard() };
    }
  }

  const cloud = environment.webkit ?? (!standard?.available ? standard : undefined);
  return cloud
    ? { mode: "cloud", language: environment.language, create: () => new cloud() }
    : typing;
}

export interface SpeechDictationCallbacks {
  read(): string;
  write(value: string, interim: boolean): void;
  state(next: SpeechInputState, detail?: string): void;
  permissionDenied(): void;
}

function joinedTranscript(base: string, speech: string): string {
  const prefix = base.trimEnd();
  const suffix = speech.trimStart();
  if (!prefix) return suffix;
  if (!suffix) return prefix;
  return `${prefix} ${suffix}`;
}

export class SpeechDictationController {
  private recognition?: SpeechRecognitionLike;
  private active = false;

  constructor(
    private readonly capability: SpeechInputCapability,
    private readonly callbacks: SpeechDictationCallbacks,
  ) {}

  get listening(): boolean { return this.active; }

  toggle(): void {
    if (this.active) this.stop();
    else this.start();
  }

  start(): void {
    if (this.active || !this.capability.create) return;
    const recognition = this.capability.create();
    const base = this.callbacks.read();
    recognition.continuous = false;
    recognition.interimResults = true;
    recognition.lang = this.capability.language;
    if (this.capability.mode === "local") recognition.processLocally = true;
    let endedWithError = false;
    recognition.onresult = (event) => {
      let speech = "";
      let hasInterim = false;
      for (let index = 0; index < event.results.length; index += 1) {
        const result = event.results[index];
        speech += result?.[0]?.transcript ?? "";
        hasInterim ||= result?.isFinal === false;
      }
      this.callbacks.write(joinedTranscript(base, speech), hasInterim);
    };
    recognition.onerror = (event) => {
      endedWithError = true;
      this.active = false;
      this.recognition = undefined;
      if (event.error === "not-allowed" || event.error === "service-not-allowed") {
        this.callbacks.permissionDenied();
        return;
      }
      this.callbacks.state("error", event.error === "no-speech" ? "No speech heard — try again or type." : "Voice input stopped — type or try again.");
    };
    recognition.onend = () => {
      this.active = false;
      this.recognition = undefined;
      if (!endedWithError) this.callbacks.state("idle");
    };
    try {
      recognition.start();
      this.recognition = recognition;
      this.active = true;
      this.callbacks.state("listening");
    } catch {
      this.callbacks.state("error", "Voice input could not start — type or try again.");
    }
  }

  stop(): void {
    this.recognition?.stop();
    this.active = false;
    this.recognition = undefined;
    this.callbacks.state("idle");
  }
}
