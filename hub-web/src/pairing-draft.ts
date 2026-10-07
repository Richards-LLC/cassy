import type { PairingPrefill } from "./fragment";
import type { Scope } from "./types";
import { PAIRING_SCOPES } from "./pairing-scopes";

export type PairingStep = "create" | "code" | "authorized" | "link";

export interface PairingDraft {
  /** The machine's hub address. Seeded only from what the machine itself put
   *  in its `cas hub pair` link, never from the page: a hosted Commander origin
   *  is a plausible-looking wrong answer for a remote machine (F5). */
  hubUrl: string;
  /** Where this page is served from, offered as one tap for the page-served-by-hub case. */
  pageOrigin: string;
  machineLabel: string;
  /** "Where do I find this?" stays open across the dialog's re-renders once opened. */
  addressHelpOpen: boolean;
  /** The step whose "Technical details" the operator opened; it stays open
   *  across that step's re-renders and starts closed on the next step, so an
   *  opened list never pushes the next step's fields below the fold. */
  technicalOpen: PairingStep | undefined;
  deviceLabel: string;
  operatorLabel: string;
  scopes: Scope[];
  email: string;
}

/**
 * The name a browser offers for itself before the operator changes it, as
 * "Chrome on Linux" (cas-d043 G10): every browser defaulted to "Cassy Cloud
 * browser", so two left at the default looked the same in the installations
 * list. Read from the user agent; anything unrecognised keeps a plain name.
 */
export function defaultBrowserName(userAgent: string): string {
  const browser = /Edg(e|A|iOS)?\//.test(userAgent) ? "Edge"
    : /OPR\/|Opera/.test(userAgent) ? "Opera"
      : /Firefox\/|FxiOS\//.test(userAgent) ? "Firefox"
        : /SamsungBrowser\//.test(userAgent) ? "Samsung Internet"
          : /Chrome\/|CriOS\/|Chromium\//.test(userAgent) ? "Chrome"
            : /Safari\//.test(userAgent) ? "Safari" : undefined;
  const platform = /iPhone/.test(userAgent) ? "iPhone"
    : /iPad/.test(userAgent) ? "iPad"
      : /Android/.test(userAgent) ? "Android"
        : /CrOS/.test(userAgent) ? "ChromeOS"
          : /Mac OS X|Macintosh/.test(userAgent) ? "Mac"
            : /Windows/.test(userAgent) ? "Windows"
              : /Linux/.test(userAgent) ? "Linux" : undefined;
  if (browser && platform) return `${browser} on ${platform}`;
  return browser ?? (platform ? `Browser on ${platform}` : "Cassy Cloud browser");
}

/**
 * `scopes` is the invitation's ceiling when one is known, never a wider guess.
 * `prefill` is what the machine's own link said about itself; both values stay
 * editable in the form.
 */
export function createPairingDraft(controllerOrigin: string, scopes?: readonly Scope[], prefill: PairingPrefill = {}): PairingDraft {
  return {
    hubUrl: prefill.suggestedHubUrl ?? "",
    pageOrigin: controllerOrigin,
    machineLabel: prefill.suggestedMachineLabel ?? "",
    addressHelpOpen: false,
    technicalOpen: undefined,
    deviceLabel: defaultBrowserName(globalThis.navigator?.userAgent ?? ""),
    operatorLabel: "",
    scopes: scopes ? [...scopes] : [...PAIRING_SCOPES],
    email: "",
  };
}

export function updatePairingDraft(
  current: PairingDraft,
  entries: Iterable<readonly [string, unknown]>,
  captureScopes = false,
): PairingDraft {
  const next = { ...current, scopes: [...current.scopes] };
  const scopes: Scope[] = [];
  let sawEntry = false;
  for (const [name, value] of entries) {
    if (typeof value !== "string") continue;
    sawEntry = true;
    if (name === "url") next.hubUrl = value;
    else if (name === "label") next.machineLabel = value;
    else if (name === "device") next.deviceLabel = value;
    else if (name === "operator") next.operatorLabel = value;
    else if (name === "scope") scopes.push(value as Scope);
  }
  if (captureScopes && sawEntry) next.scopes = scopes;
  return next;
}
