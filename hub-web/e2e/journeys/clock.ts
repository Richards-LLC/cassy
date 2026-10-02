// One starting instant for the browser and Node-side protocol fixtures. Date
// advances normally so receipt deadlines and heartbeat aging still work.
export const JOURNEY_TIMEZONE = "UTC";
const instant = process.env.HUB_JOURNEY_NOW ?? "2026-09-30T12:00:00.000Z";
if (!/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{3})?Z$/.test(instant) || !Number.isFinite(Date.parse(instant))) {
  throw new Error("HUB_JOURNEY_NOW must be an ISO UTC instant, e.g. 2026-09-30T00:30:00Z");
}
export const JOURNEY_NOW = Date.parse(instant);
let started = performance.now();
/** A test is running: its clock has started and not yet stopped. */
let running = false;

/** Each test gets the same starting instant, independent of worker lifetime. */
export function startJourneyClock(): number {
  started = performance.now();
  running = true;
  return JOURNEY_NOW;
}

/** The test is over; until the next one starts, time stands at its instant. */
export function stopJourneyClock(): void {
  running = false;
}

/**
 * The journey's "now". Inside a test it runs from the test's start instant.
 * Outside one, when Playwright imports a spec file between tests and its
 * module-level fixtures call journeyStamp, it is the start instant itself:
 * the time the previous test in the worker has spent no longer leaks into
 * those fixtures (cas-4e52, cas-6acf QA F01 — "3h" read "2h" and "2m" read
 * "1m" whenever the spec ran after another one in its worker).
 */
export function journeyNow(): number {
  return running ? JOURNEY_NOW + performance.now() - started : JOURNEY_NOW;
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
