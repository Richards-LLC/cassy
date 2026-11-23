/**
 * One honest capacity reading for both Accounts and New session.
 *
 * Every number is "% left" (one polarity everywhere). A window whose reset has
 * passed since the read is unknown, never an old number; an older read can
 * only have gone down, so it says "≤ N% left" with its age.
 */
import { escapeHtml } from "./cloud-brand";
import { accountTime, type AccountWindow } from "./accounts-model";

export type WindowSlot = "5h" | "week";
export const SLOTS: readonly WindowSlot[] = ["5h", "week"];
export const SLOT_LABEL: Record<WindowSlot, string> = { "5h": "5h", week: "Week" };
/** A read older than this is shown as an upper bound with its age. */
export const STALE_MS = 60 * 60_000;

export type WindowReading =
  | { slot: WindowSlot; kind: "live" | "stale"; left: number; reset?: string; resetsIn?: string; age?: string }
  | { slot: WindowSlot; kind: "reset" | "unknown" | "missing" };

const slotOf = (name: string): WindowSlot | undefined => /week|7d|seven/i.test(name) ? "week" : /^5h$|five|session/i.test(name) ? "5h" : undefined;

/** "in 42 min", "in 2 h", "in 3 d": whole units, never a break inside (nbsp). */
export function relativeIn(instant: string, now = Date.now()): string | undefined {
  const ms = new Date(instant).getTime() - now;
  if (!Number.isFinite(ms) || ms <= 0) return undefined;
  const minutes = Math.round(ms / 60_000);
  const text = minutes < 60 ? `${Math.max(1, minutes)} min` : minutes < 48 * 60 ? `${Math.round(minutes / 60)} h` : `${Math.round(minutes / 1440)} d`;
  return `in ${text.replace(" ", " ")}`;
}

/** "read 3 d ago" for an observation instant. */
export function readAge(instant: string, now = Date.now()): string | undefined {
  const ms = now - new Date(instant).getTime();
  if (!Number.isFinite(ms) || ms < 0) return undefined;
  const minutes = Math.round(ms / 60_000);
  const text = minutes < 60 ? `${Math.max(1, minutes)} min` : minutes < 48 * 60 ? `${Math.round(minutes / 60)} h` : `${Math.round(minutes / 1440)} d`;
  return `read ${text.replace(" ", " ")} ago`;
}

/** Both slots, always in order, from whatever windows the source reported. */
export function readings(windows: readonly AccountWindow[] | undefined, observed: string | null | undefined, now = Date.now()): WindowReading[] {
  const observedAt = observed ? new Date(observed).getTime() : NaN;
  const stale = Number.isFinite(observedAt) && now - observedAt > STALE_MS;
  return SLOTS.map(slot => {
    const window = windows?.find(item => slotOf(item.name) === slot);
    if (!window) return { slot, kind: "missing" };
    const reset = window.resets_at ? new Date(window.resets_at).getTime() : undefined;
    if (reset !== undefined && Number.isFinite(reset) && reset <= now) return { slot, kind: "reset" };
    const used = window.used_percent;
    if (used === null || !Number.isFinite(used) || used < 0 || used > 100) return { slot, kind: "unknown" };
    return {
      slot, kind: stale ? "stale" : "live", left: Math.round(100 - used),
      reset: window.resets_at && reset !== undefined && Number.isFinite(reset) ? accountTime(window.resets_at) : undefined,
      resetsIn: window.resets_at ? relativeIn(window.resets_at, now) : undefined,
      age: stale && observed ? readAge(observed, now) : undefined,
    };
  });
}

export const tone = (left: number): "ok" | "low" | "crit" => left < 10 ? "crit" : left < 25 ? "low" : "ok";

/**
 * One meter. `labelled` adds the window label (phone and New session rows;
 * desktop Accounts names windows in its column headers). `brief` keeps only
 * the relative reset.
 */
export function meterMarkup(reading: WindowReading, options: { labelled: boolean; brief?: boolean }): string {
  const label = options.labelled ? `<span class="meter-window">${SLOT_LABEL[reading.slot]}</span>` : "";
  const name = SLOT_LABEL[reading.slot];
  if (reading.kind === "live" || reading.kind === "stale") {
    const value = reading.kind === "stale" ? `≤ ${reading.left}%` : `${reading.left}%`;
    const when = reading.kind === "stale" ? `<span class="meter-age">${escapeHtml(reading.age ?? "")}</span>` : reading.resetsIn ? `<span class="meter-in">${escapeHtml(reading.resetsIn)}</span>` : "";
    const at = !options.brief && reading.reset ? `<span class="meter-at">resets ${escapeHtml(reading.reset)}</span>` : "";
    const aria = `${name}: ${reading.kind === "stale" ? "at most " : ""}${reading.left}% left${reading.reset ? `, resets ${reading.reset}` : ""}${reading.age ? `, ${reading.age}` : ""}`;
    return `<div class="meter" data-testid="account-meter" data-window="${reading.slot}" data-state="${reading.kind}" data-left="${reading.left}" data-tone="${tone(reading.left)}" role="img" aria-label="${escapeHtml(aria)}">${label}<span class="meter-bar" aria-hidden="true"><span data-fill="${reading.left}"></span></span><span class="meter-value">${value}<span class="meter-unit"> left</span></span><span class="meter-when">${when}${when && at ? " · " : ""}${at}</span></div>`;
  }
  const words = reading.kind === "reset" ? "Unknown · reset since last read" : reading.kind === "unknown" ? "Unknown" : "Not reported";
  return `<div class="meter" data-testid="account-meter" data-window="${reading.slot}" data-state="${reading.kind}" role="img" aria-label="${escapeHtml(`${name}: ${words}`)}">${label}<span class="meter-bar meter-bar--none" aria-hidden="true"></span><span class="meter-value meter-value--none">${words}</span></div>`;
}

/** "77% 5h · 62% week · resets in 2 h": the plan strip's one-line summary. */
export function capacityLine(list: readonly WindowReading[]): string {
  const parts = list.map(reading => reading.kind === "live" ? `${reading.left}% ${SLOT_LABEL[reading.slot].toLowerCase()}` : reading.kind === "stale" ? `≤${reading.left}% ${SLOT_LABEL[reading.slot].toLowerCase()}` : `${SLOT_LABEL[reading.slot].toLowerCase()} unknown`);
  const soonest = list.filter((reading): reading is Extract<WindowReading, { kind: "live" | "stale" }> => (reading.kind === "live" || reading.kind === "stale") && Boolean(reading.resetsIn))[0];
  return `${parts.join(" · ")}${soonest?.resetsIn ? ` · resets ${soonest.resetsIn}` : ""}`;
}

/** Bar widths go through CSSOM: the hub's style-src refuses inline style attributes. */
export function fillMeters(root: ParentNode): void {
  for (const bar of root.querySelectorAll<HTMLElement>(".meter-bar > [data-fill]")) bar.style.inlineSize = `${Number(bar.dataset.fill) || 0}%`;
}
