import { expect, it, vi } from "vitest";
import { StatusRefresh } from "./status-refresh";

it("keeps the newer phase after slow-then-fast status reads, including an older failure", async () => {
  const refresh = new StatusRefresh();
  let phase = "not reported";
  const received = (value: string) => { phase = value; };
  const failed = vi.fn();
  for (const rejected of [false, true]) {
    let resolve!: (value: string) => void;
    let reject!: () => void;
    const slow = refresh.request("machine:session", () => new Promise<string>((yes, no) => { resolve = yes; reject = () => no(new Error("old read")); }), received, failed);
    // Other sessions have independent reads.
    await refresh.request("machine:other", async () => "Waiting on you", received, failed);
    await refresh.request("machine:session", async () => "Compacted", received, failed);
    if (rejected) reject(); else resolve("Compacting");
    await slow;
    expect(phase).toBe("Compacted");
    expect(failed).not.toHaveBeenCalled();
  }
  await refresh.request("machine:session", async () => { throw new Error("current read"); }, received, failed);
  expect(failed).toHaveBeenCalledOnce();
});
