// One starting instant for the browser and Node-side protocol fixtures. Date
// advances normally so receipt deadlines and heartbeat aging still work.
export const JOURNEY_TIMEZONE = "UTC";
const instant = process.env.HUB_JOURNEY_NOW ?? "2026-09-30T12:00:00.000Z";
if (!/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{3})?Z$/.test(instant) || !Number.isFinite(Date.parse(instant))) {
  throw new Error("HUB_JOURNEY_NOW must be an ISO UTC instant, e.g. 2026-09-30T00:30:00Z");
}
export const JOURNEY_NOW = Date.parse(instant);
let started = performance.now();

/** Each test gets the same starting instant, independent of worker lifetime. */
export function startJourneyClock(): number {
  started = performance.now();
  return JOURNEY_NOW;
}

export function journeyNow(): number {
  return JOURNEY_NOW + performance.now() - started;
}

/** Machine clock skew or a recent turn, relative to the injected instant. */
export function journeyStamp(offsetMs = 0): string {
  return new Date(journeyNow() + offsetMs).toISOString();
}

/** Calendar placement, rather than subtracting hours to guess a day label. */
export function journeyDay(daysAgo: number, hour = 12, minute = 0): string {
  const day = new Date(JOURNEY_NOW);
  day.setUTCDate(day.getUTCDate() - daysAgo);
  day.setUTCHours(hour, minute, 0, 0);
  return day.toISOString();
}
