/**
 * One honest capacity reading for both Accounts and New session.
 *
 * Every number is "% left" (one polarity everywhere). A window whose reset has
 * passed since the read is unknown, never an old number; an older read can
 * only have gone down, so it says "≤ N% left" with its age.
 */
import { escapeHtml } from "./cloud-brand";
import { accountTime, type AccountWindow, type UsageSource } from "./accounts-model";

export type WindowSlot = "5h" | "week";
export const SLOTS: readonly WindowSlot[] = ["5h", "week"];
export const SLOT_LABEL: Record<WindowSlot, string> = { "5h": "5h", week: "Week" };
/** A read older than this is shown as an upper bound with its age. */
export const STALE_MS = 60 * 60_000;

export type WindowReading =
  | { slot: WindowSlot; kind: "live" | "stale" | "local"; left: number; reset?: string; resetsIn?: string; resetAt?: string; age?: string }
  | { slot: WindowSlot; kind: "reset" | "unknown" | "missing" | "unreachable" };

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

/** Both slots, always in order, from whatever windows the source reported. An unreachable hub has no number to show. */
/**
 * `checkedOn` names the machine whose own ↻ check these windows came from
 * while a hub is configured: such a number isn't the shared one, so it draws
 * muted and says whose check it was and when.
 */
export function readings(windows: readonly AccountWindow[] | undefined, observed: string | null | undefined, now = Date.now(), source?: UsageSource, checkedOn?: string): WindowReading[] {
  if (source === "hub_unreachable") return SLOTS.map(slot => ({ slot, kind: "unreachable" }));
  const observedAt = observed ? new Date(observed).getTime() : NaN;
  const stale = Number.isFinite(observedAt) && now - observedAt > STALE_MS;
  return SLOTS.map(slot => {
    const window = windows?.find(item => slotOf(item.name) === slot);
    if (!window) return { slot, kind: "missing" };
    const reset = window.resets_at ? new Date(window.resets_at).getTime() : undefined;
    if (reset !== undefined && Number.isFinite(reset) && reset <= now) return { slot, kind: "reset" };
    const used = window.used_percent;
    if (used === null || !Number.isFinite(used) || used < 0 || used > 100) return { slot, kind: "unknown" };
    const ago = observed ? readAge(observed, now)?.replace(/^read\s/, "") : undefined;
    return {
      slot, kind: checkedOn ? "local" : stale ? "stale" : "live", left: Math.round(100 - used),
      reset: window.resets_at && reset !== undefined && Number.isFinite(reset) ? accountTime(window.resets_at) : undefined,
      resetsIn: window.resets_at ? relativeIn(window.resets_at, now) : undefined,
      resetAt: window.resets_at && reset !== undefined && Number.isFinite(reset) ? new Date(reset).toISOString() : undefined,
      age: checkedOn ? `${checkedOn}'s check${ago ? `, ${ago}` : ""}` : stale && observed ? readAge(observed, now) : undefined,
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
  if (reading.kind === "live" || reading.kind === "stale" || reading.kind === "local") {
    const value = reading.kind === "stale" ? `≤ ${reading.left}%` : `${reading.left}%`;
    const when = reading.kind !== "live" ? `<span class="meter-age">${escapeHtml(reading.age ?? "")}</span>` : reading.resetsIn ? `<span class="meter-in">${escapeHtml(reading.resetsIn)}</span>` : "";
    const at = !options.brief && reading.reset ? `<span class="meter-at">${when ? " · " : ""}resets ${escapeHtml(reading.reset)}</span>` : "";
    const aria = `${name}: ${reading.kind === "stale" ? "at most " : ""}${reading.left}% left${reading.reset ? `, resets ${reading.reset}` : ""}${reading.age ? `, ${reading.age}` : ""}`;
    return `<div class="meter" data-testid="account-meter" data-window="${reading.slot}" data-state="${reading.kind}" data-left="${reading.left}"${reading.resetAt ? ` data-reset="${reading.resetAt}"` : ""} data-tone="${tone(reading.left)}" role="img" aria-label="${escapeHtml(aria)}">${label}<span class="meter-bar" aria-hidden="true"><span data-fill="${reading.left}"></span></span><span class="meter-value">${value}<span class="meter-unit"> left</span></span><span class="meter-when">${when}${at}</span></div>`;
  }
  const words = reading.kind === "reset" ? "Unknown · reset since last read" : reading.kind === "unknown" ? "Unknown" : reading.kind === "unreachable" ? "Hub unreachable" : "Not reported";
  // An unreachable hub is said once, above the ledger; each cell is a muted dash.
  const shown = reading.kind === "unreachable" ? "—" : words;
  return `<div class="meter" data-testid="account-meter" data-window="${reading.slot}" data-state="${reading.kind}" role="img" aria-label="${escapeHtml(`${name}: ${words}`)}">${label}<span class="meter-bar meter-bar--none" aria-hidden="true"></span><span class="meter-value meter-value--none">${shown}</span></div>`;
}

/** Least share left across known windows, or undefined when none is known. */
export function leastLeft(list: readonly WindowReading[]): number | undefined {
  const known = list.flatMap(reading => "left" in reading ? [reading.left] : []);
  return known.length ? Math.min(...known) : undefined;
}
/** Below this, a recommended account says on its row why it is still first. */
export const LOW_PICK = 20;
type Known = Extract<WindowReading, { kind: "live" | "stale" | "local" }>;
const weekOf = (list: readonly WindowReading[]): Known | undefined => list.find((reading): reading is Known => reading.slot === "week" && "left" in reading);
export interface PickCandidate { name: string; list: readonly WindowReading[]; exclusion?: string }
export interface PickReason { text: string; low: boolean; reset?: string; soonest?: boolean }
/**
 * Why the recommendation leads, built from the same readings its meters draw,
 * so the reason, the plan strip and the bars can never disagree. "Soonest"
 * is said only when a comparison with every other usable login shows it.
 */
export function pickReason(pick: PickCandidate, others: readonly PickCandidate[]): PickReason {
  const week = weekOf(pick.list);
  if (!week) return { text: "No weekly reading to rank by", low: false };
  const left = leastLeft(pick.list) ?? week.left;
  const at = (reading: Known | undefined) => reading?.resetAt ? Date.parse(reading.resetAt) : Infinity;
  const sooner = others.map(other => ({ other, week: weekOf(other.list) })).filter(item => at(item.week) < at(week)).sort((a, b) => at(a.week) - at(b.week))[0];
  const resets = week.resetsIn ? `resets ${week.resetsIn}` : "reset unknown";
  const low = left < LOW_PICK;
  const lead = low ? `Only ${left}% left; ${resets}.` : `Week ${resets}.`;
  const earlier = others.filter(other => at(weekOf(other.list)) < at(week));
  const first = sooner?.week;
  const why = !others.length ? " Only login here."
    : !sooner ? " Soonest here, so it's used before it expires."
    : earlier.every(other => other.exclusion?.includes("floor")) ? ` Sooner login${earlier.length === 1 ? " is" : "s are"} under the floor.`
    : earlier.every(other => other.exclusion) ? ` Sooner login${earlier.length === 1 ? " is" : "s are"} excluded: ${sooner.other.exclusion}.`
    // The recommender ranked on the reads it had; say which read differs rather than leave a bare "not chosen".
    : first?.kind === "stale" ? ` ${sooner.other.name} resets sooner, but its read is ${first.age ?? "old"}.`
    : first?.kind === "local" ? ` ${sooner.other.name} resets sooner by ${first.age ?? "this machine's check"}, which the recommender didn't use.`
    : ` ${sooner.other.name} resets sooner (${first?.resetsIn ?? "unknown"}) on this read, newer than the recommender's.`;
  const text = `${lead}${why}`;
  return { text, low, reset: week.resetAt, soonest: !sooner };
}

/**
 * A login the hub doesn't carry has no shared number: its machine's own read
 * is muted text with its date, never a meter.
 */
export function localReadLine(list: readonly WindowReading[], observed: string | null | undefined, now = Date.now()): string {
  const known = list.filter((reading): reading is Known => "left" in reading);
  const age = observed ? readAge(observed, now) : undefined;
  return `Not on the hub${known.length ? ` · ${known.map(reading => `${reading.kind === "stale" ? AT_MOST : ""}${reading.left}% ${SLOT_LABEL[reading.slot].toLowerCase()}`).join(", ")} left${age ? `, ${age}` : ""}` : ""}`;
}

/**
 * An exclusion in the meters' own terms. The selector reports a threshold in
 * used percent ("weekly at 87% (threshold 85%); resets …"); rows speak
 * percent left and leave the reset to the meter. "resets later than …" only
 * restates the meters and is dropped.
 */
export function exclusionText(exclusion: string | null | undefined): string | undefined {
  if (!exclusion || exclusion.startsWith("resets later than ")) return undefined;
  const threshold = /^(\S+) at (\d+)% \(threshold (\d+)%\)/.exec(exclusion);
  if (!threshold) return exclusion;
  const [, window, used, limit] = threshold;
  return `${100 - Number(used)}% ${window === "weekly" ? "week" : window} left, under the ${100 - Number(limit)}% floor`;
}

/** "≤ 81%": one form for an upper bound everywhere (no break after ≤). */
const AT_MOST = "≤\u00a0";

/** "77% 5h left · 62% week left · 5h resets in 2 h": the plan strip's one-line summary. */
export function capacityLine(list: readonly WindowReading[]): string {
  if (list.every(reading => reading.kind === "unreachable")) return "hub unreachable";
  // Every figure says "left"; a window the provider doesn't report is left out.
  const parts = list.filter(reading => reading.kind !== "missing").map(reading => "left" in reading ? `${reading.kind === "stale" ? AT_MOST : ""}${reading.left}% ${SLOT_LABEL[reading.slot].toLowerCase()} left` : `${SLOT_LABEL[reading.slot].toLowerCase()} unknown`);
  const known = list.filter((reading): reading is Known => "left" in reading && Boolean(reading.resetsIn));
  const soonest = known.sort((a, b) => Date.parse(a.resetAt ?? "") - Date.parse(b.resetAt ?? ""))[0];
  const local = list.find((reading): reading is Known => reading.kind === "local");
  return `${parts.join(" · ")}${soonest?.resetsIn ? ` · ${SLOT_LABEL[soonest.slot].toLowerCase()} resets ${soonest.resetsIn}` : ""}${local?.age ? ` · ${local.age}` : ""}`;
}

/** Bar widths go through CSSOM: the hub's style-src refuses inline style attributes. */
export function fillMeters(root: ParentNode): void {
  for (const bar of root.querySelectorAll<HTMLElement>(".meter-bar > [data-fill]")) bar.style.inlineSize = `${Number(bar.dataset.fill) || 0}%`;
}
