import { afterEach, expect, it, vi } from "vitest";
import { CoalescedRefresh } from "./catalog-refresh";

afterEach(() => vi.useRealTimers());

it("keeps one flight while a burst arrives and reads its last change", async () => {
  vi.useFakeTimers();
  let finish!: () => void;
  let revision = 0;
  let latest = -1;
  let flights = 0;
  let peak = 0;
  const starts: number[] = [];
  const lane = new CoalescedRefresh(async () => {
    starts.push(Date.now()); peak = Math.max(peak, ++flights);
    const snapshot = revision;
    if (starts.length === 1) await new Promise<void>(resolve => { finish = resolve; });
    latest = snapshot;
    flights--;
  }, error => { throw error; });
  const drained = lane.request();
  await vi.advanceTimersByTimeAsync(0);
  for (revision = 1; revision <= 1_000; revision++) lane.request();
  revision = 1_000;
  finish();
  await vi.advanceTimersByTimeAsync(999);
  expect(starts).toHaveLength(1);
  await vi.advanceTimersByTimeAsync(1);
  await drained;
  expect(starts).toHaveLength(2);
  expect(starts[1]! - starts[0]!).toBe(1_000);
  expect(peak).toBe(1);
  expect(latest).toBe(1_000);
});

it("preserves a reentrant trailing request after a failure, then stays idle", async () => {
  vi.useFakeTimers();
  const failed = vi.fn();
  let attempts = 0;
  const lane = new CoalescedRefresh(async () => {
    if (++attempts === 1) { void lane.request(); throw new Error("temporary"); }
  }, failed);
  const drained = lane.request();
  await vi.advanceTimersByTimeAsync(1_000);
  await drained;
  expect(attempts).toBe(2);
  expect(failed).toHaveBeenCalledOnce();
  await vi.advanceTimersByTimeAsync(60_000);
  expect(attempts).toBe(2);
  await lane.request();
  expect(attempts).toBe(3);
});
