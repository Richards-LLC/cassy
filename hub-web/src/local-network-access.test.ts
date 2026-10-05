import { afterEach, expect, it, vi } from "vitest";
import { localNetworkAccessHelp } from "./local-network-access";

afterEach(() => { vi.unstubAllGlobals(); vi.useRealTimers(); });

it("explains denied and prompt tailnet access without claiming the pairing is gone", async () => {
  const query = vi.fn().mockResolvedValue({ state: "denied" });
  vi.stubGlobal("navigator", { permissions: { query } });
  const url = "https://soundwave-linux.tailf5a734.ts.net";
  expect(await localNetworkAccessHelp(url, "soundwave")).toBe("Can't reach soundwave. Allow Local network access for this page in your browser's site settings. Reconnecting…");
  expect(query).toHaveBeenCalledWith({ name: "local-network" });
  query.mockResolvedValue({ state: "prompt" });
  expect(await localNetworkAccessHelp(url, "soundwave")).toContain("when your browser asks");
  query.mockResolvedValue({ state: "granted" });
  expect(await localNetworkAccessHelp(url, "soundwave")).toBeUndefined();
});

it("keeps a useful fallback for unsupported queries without using the unsafe legacy alias", async () => {
  const query = vi.fn().mockRejectedValue(new TypeError("Unknown permission"));
  vi.stubGlobal("navigator", { permissions: { query } });
  expect(await localNetworkAccessHelp("https://soundwave-linux.tailf5a734.ts.net", "soundwave")).toContain("Check Tailscale");
  expect(query).toHaveBeenCalledTimes(1);
  expect(query).toHaveBeenCalledWith({ name: "local-network" });
});

it("does not attribute public or same-origin failures to local network permission", async () => {
  const query = vi.fn();
  vi.stubGlobal("navigator", { permissions: { query } });
  expect(await localNetworkAccessHelp("https://public.example", "Public hub")).toBeUndefined();
  vi.stubGlobal("location", { origin: "https://soundwave-linux.tailf5a734.ts.net" });
  expect(await localNetworkAccessHelp("https://soundwave-linux.tailf5a734.ts.net", "soundwave")).toBeUndefined();
  expect(query).not.toHaveBeenCalled();
});

it("bounds a permission query that never answers so it cannot park retries", async () => {
  vi.useFakeTimers();
  vi.stubGlobal("navigator", { permissions: { query: () => new Promise(() => {}) } });
  const help = localNetworkAccessHelp("https://soundwave-linux.tailf5a734.ts.net", "soundwave");
  await vi.advanceTimersByTimeAsync(500);
  expect(await help).toContain("Check Tailscale");
});
