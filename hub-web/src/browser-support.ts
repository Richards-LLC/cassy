/**
 * Commander used to fail on an old engine by spinning forever with no
 * explanation (report cas-b652, defect D3). Every entry here is an API the app
 * actually calls, with the browser version that shipped it, so a browser that
 * cannot run this build is told so in one line instead of pretending to
 * connect. AbortSignal.any is deliberately absent: abort-signals.ts carries a
 * fallback, so it is no longer a floor.
 */

export interface BrowserVersions {
  readonly chrome: number;
  readonly edge: number;
  readonly firefox: number;
  readonly safari: string;
}

/**
 * "transport" is the subset the hub connection itself needs. The connection
 * layer must refuse only for those: gating a socket on a terminal or dialog
 * API would fail an engine that could still have connected.
 */
export type BrowserArea = "transport" | "app";

export interface BrowserRequirement {
  readonly api: string;
  readonly usedBy: string;
  readonly area: BrowserArea;
  readonly since: BrowserVersions;
}

export interface ProbedRequirement extends BrowserRequirement {
  readonly present: boolean;
}

export interface BrowserSupport {
  readonly requirements: readonly ProbedRequirement[];
  readonly missing: readonly ProbedRequirement[];
  readonly supported: boolean;
}

export const REQUIRED_BROWSER_APIS: readonly BrowserRequirement[] = [
  { api: "AbortSignal.timeout", usedBy: "connection.ts", area: "transport", since: { chrome: 103, edge: 103, firefox: 100, safari: "16" } },
  { api: "Array.prototype.toSorted", usedBy: "main.ts, attention-view.ts", area: "app", since: { chrome: 110, edge: 110, firefox: 115, safari: "16.4" } },
  { api: "HTMLDialogElement.prototype.showModal", usedBy: "main.ts", area: "app", since: { chrome: 37, edge: 79, firefox: 98, safari: "15.4" } },
  { api: "crypto.subtle", usedBy: "dpop.ts", area: "transport", since: { chrome: 37, edge: 79, firefox: 34, safari: "11" } },
  { api: "ResizeObserver", usedBy: "terminal/ghostty/surface.ts", area: "app", since: { chrome: 64, edge: 79, firefox: 69, safari: "13.1" } },
  { api: "WebAssembly.instantiate", usedBy: "terminal/ghostty/runtime.ts", area: "app", since: { chrome: 57, edge: 16, firefox: 52, safari: "11" } },
];

type Probe = (api: string) => boolean;

const DEFAULT_PROBES: Record<string, () => boolean> = {
  "AbortSignal.timeout": () => typeof (AbortSignal as unknown as { timeout?: unknown }).timeout === "function",
  "Array.prototype.toSorted": () => typeof (Array.prototype as unknown as { toSorted?: unknown }).toSorted === "function",
  "HTMLDialogElement.prototype.showModal": () =>
    typeof (globalThis as { HTMLDialogElement?: { prototype?: { showModal?: unknown } } }).HTMLDialogElement?.prototype?.showModal === "function",
  "crypto.subtle": () => typeof globalThis.crypto?.subtle?.importKey === "function",
  "ResizeObserver": () => typeof (globalThis as { ResizeObserver?: unknown }).ResizeObserver === "function",
  "WebAssembly.instantiate": () =>
    typeof (globalThis as { WebAssembly?: { instantiate?: unknown } }).WebAssembly?.instantiate === "function",
};

function defaultProbe(api: string): boolean {
  return DEFAULT_PROBES[api]?.() ?? true;
}

export function browserSupport(probe: Probe = defaultProbe, area?: BrowserArea): BrowserSupport {
  const requirements = REQUIRED_BROWSER_APIS
    .filter((requirement) => area === undefined || requirement.area === area)
    .map((requirement) => ({ ...requirement, present: probe(requirement.api) }));
  const missing = requirements.filter((requirement) => !requirement.present);
  return { requirements, missing, supported: missing.length === 0 };
}

function listApis(missing: readonly ProbedRequirement[]): string {
  const names = [...missing].map((requirement) => requirement.api).sort();
  if (names.length === 1) return names[0];
  return `${names.slice(0, -1).join(", ")} and ${names.at(-1)}`;
}

/** Safari ships x.y versions, so its floor is compared as a version tuple. */
function newerSafari(left: string, right: string): string {
  const parts = (value: string) => value.split(".").map(Number);
  const [leftMajor = 0, leftMinor = 0] = parts(left);
  const [rightMajor = 0, rightMinor = 0] = parts(right);
  return leftMajor > rightMajor || (leftMajor === rightMajor && leftMinor > rightMinor) ? left : right;
}

export function unsupportedBrowserNotice(support: BrowserSupport): string | undefined {
  if (support.missing.length === 0) return undefined;
  const floor = support.missing.reduce<BrowserVersions>((worst, requirement) => ({
    chrome: Math.max(worst.chrome, requirement.since.chrome),
    edge: Math.max(worst.edge, requirement.since.edge),
    firefox: Math.max(worst.firefox, requirement.since.firefox),
    safari: newerSafari(worst.safari, requirement.since.safari),
  }), { chrome: 0, edge: 0, firefox: 0, safari: "0" });
  return `This browser is missing ${listApis(support.missing)}, which Cassy Commander needs. `
    + `Update to Chrome ${floor.chrome}, Edge ${floor.edge}, Firefox ${floor.firefox}, or Safari ${floor.safari} or newer.`;
}
