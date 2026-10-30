import { escapeHtml } from "./cloud-brand";

export const LATEST_RELEASE_URL = "https://api.github.com/repos/Richards-LLC/cassy/releases/latest";
export const RELEASE_CHECK_INTERVAL = 60 * 60_000;
const CACHE_KEY = "commander.latest-release.v1";
type ReleaseCache = { checkedAt: number; version?: string };

function parsed(version: string | undefined) {
  const match = /^v?(\d+)\.(\d+)\.(\d+)(-[\da-z.-]+)?(?:\+[\da-z.-]+)?$/i.exec(version ?? "");
  if (!match) return undefined;
  const parts = match.slice(1, 4).map(Number);
  return parts.every(Number.isSafeInteger) ? { parts, prerelease: Boolean(match[4]) } : undefined;
}

/** Only a real, stable published release can suggest an upgrade. */
export function isNewerRelease(current: string | undefined, latest: string | undefined): boolean {
  const installed = parsed(current), release = parsed(latest);
  if (!installed || !release || release.prerelease) return false;
  for (let i = 0; i < 3; i++) {
    if (release.parts[i] !== installed.parts[i]) return release.parts[i]! > installed.parts[i]!;
  }
  return installed.prerelease;
}

export function machineVersionMarkup(runtime: string | undefined, latest?: string, unknownLabel = "Version unknown"): string {
  const known = typeof runtime === "string" && runtime && runtime !== "unknown";
  const update = isNewerRelease(runtime, latest);
  return `<span class="machine-version"><span class="machine-version-installed${known ? "" : " unknown"}">${known ? `Cassy ${escapeHtml(runtime)}` : escapeHtml(unknownLabel)}</span>${update ? ` <span class="machine-version-update">Update available → ${escapeHtml(latest!.replace(/^v/, ""))}</span>` : ""}</span>`;
}

/** Persist attempts as well as successes: offline reloads never hammer GitHub.
 * A failed check keeps the last known release. Web Locks serialize tab checks.
 */
export class LatestRelease {
  private cache?: ReleaseCache;
  private pending?: Promise<void>;
  private storage?: Storage;

  constructor() {
    try { this.storage = localStorage; } catch { /* Memory-only on restricted browsers. */ }
    this.read();
  }

  get version(): string | undefined { return this.cache?.version; }

  private read(): void {
    try {
      const value = JSON.parse(this.storage?.getItem(CACHE_KEY) ?? "null") as ReleaseCache | null;
      if (value && Number.isFinite(value.checkedAt) && (!this.cache || value.checkedAt >= this.cache.checkedAt)) {
        const version = typeof value.version === "string" && parsed(value.version) && !parsed(value.version)!.prerelease ? value.version : undefined;
        this.cache = { checkedAt: value.checkedAt, version };
      }
    } catch { /* A corrupt cache is disposable. */ }
  }

  private save(): void {
    try { this.storage?.setItem(CACHE_KEY, JSON.stringify(this.cache)); } catch { /* In-memory throttle remains. */ }
  }

  check(): Promise<void> {
    if (this.pending) return this.pending;
    const run = async () => {
      this.read();
      const now = Date.now();
      if (this.cache && now >= this.cache.checkedAt && now - this.cache.checkedAt < RELEASE_CHECK_INTERVAL) return;
      this.cache = { checkedAt: now, version: this.version };
      this.save();
      try {
        // No machine address, credential, or pairing data accompanies this request.
        const response = await fetch(LATEST_RELEASE_URL, { credentials: "omit", referrerPolicy: "no-referrer", signal: AbortSignal.timeout(5000) });
        if (!response.ok) return;
        const release = await response.json() as { tag_name?: string; prerelease?: boolean; draft?: boolean };
        const version = release.tag_name;
        if (release.prerelease || release.draft || typeof version !== "string" || !parsed(version) || parsed(version)!.prerelease) return;
        this.cache.version = version.replace(/^v/, "");
        this.save();
      } catch { /* Keep the last known release without alarming or blocking the operator. */ }
    };
    const locks = globalThis.navigator?.locks;
    this.pending = (async () => { if (locks) await locks.request(CACHE_KEY, run); else await run(); })().finally(() => { this.pending = undefined; });
    return this.pending;
  }
}
