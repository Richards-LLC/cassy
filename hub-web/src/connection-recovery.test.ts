import { webcrypto } from "node:crypto";
import { afterEach, describe, expect, it, vi } from "vitest";
import { CoalescedRefresh, EventRecovery } from "./event-recovery";
import { withRequestDeadline } from "./request-deadline";
import { ConnectionDiagnostics } from "./connection-diagnostics";
import { HubConnectionSupervisor } from "./connection";
import { createDeviceKey } from "./dpop";
import { createPairingRequest, pollPairingRequest, acknowledgePairing } from "./pairing-relay";
import type { PendingRelayRequest } from "./pending-pairing";

afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });
const pending: PendingRelayRequest = { kind: "relay-request", pairingRequestId: "request-secret", userCode: "ABCD-EFGH", pollSecret: "poll-secret", controllerOrigin: "https://controller.test", requestedScopes: ["machine-read"], expiresAt: "2027-01-01T00:00:00Z", interval: 1 };
describe("bounded connection recovery (cas-2b3a5)", () => {
  it("settles an uncooperative fetch and aborts its signal at the deadline", async () => {
    vi.useFakeTimers();
    let signal: AbortSignal | undefined;
    const result = withRequestDeadline(s => { signal = s; return new Promise(() => {}); });
    const rejected = expect(result).rejects.toMatchObject({ name: "TimeoutError" });
    await vi.advanceTimersByTimeAsync(10_000);
    await rejected;
    expect(signal?.aborted).toBe(true);
    expect(vi.getTimerCount()).toBe(0);
  });
  it("cancels promptly and removes its deadline even if transport ignores cancellation", async () => {
    vi.useFakeTimers();
    const parent = new AbortController();
    const result = withRequestDeadline(() => new Promise(() => {}), parent.signal);
    const rejected = expect(result).rejects.toMatchObject({ name: "AbortError" });
    parent.abort(); await rejected;
    expect(vi.getTimerCount()).toBe(0);
  });
  it.each(["create", "poll", "ack"])("bounds pairing %s including the body", async name => {
    vi.useFakeTimers();
    const fetcher = vi.fn().mockResolvedValue({ status: name === "create" ? 201 : 202, json: () => new Promise(() => {}) });
    if (name === "ack") fetcher.mockImplementation(() => new Promise(() => {}));
    const result = name === "create" ? createPairingRequest(fetcher, "https://relay.test", pending.controllerOrigin)
      : name === "poll" ? pollPairingRequest(fetcher, "https://relay.test", pending)
        : acknowledgePairing(fetcher, "https://relay.test", { pairingRequestId: "request-secret", pollSecret: "poll-secret", deliveryId: "delivery-secret" });
    const rejected = expect(result).rejects.toMatchObject({ name: "TimeoutError" });
    await vi.advanceTimersByTimeAsync(10_000); await rejected;
    expect(fetcher).toHaveBeenCalledTimes(1); // uncertain mutations are never automatically repeated
  });
  it.each(["/v1/auth/refresh", "/v1/diagnostics"])("bounds signing/fetch/body for %s", async path => {
    vi.stubGlobal("crypto", webcrypto);
    const machine = { id: "hub", label: "Secret label", baseUrl: "https://hub.test", credential: "secret-credential", credentialId: "credential", deviceId: "device", scopes: [] as [], expiresAt: "2027-01-01T00:00:00Z", ...await createDeviceKey() };
    const connection = new HubConnectionSupervisor(machine, { onState: () => {}, onSessions: () => {}, onMachineEvent: () => {}, onSessionState: () => {}, onOutput: () => {}, onPaneKeyframe: () => {}, onSocketError: () => {} });
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue({ status: 200, ok: true, json: () => new Promise(() => {}) }));
    vi.useFakeTimers();
    const result = connection.request(path.includes("refresh") ? "POST" : "GET", path);
    const rejected = expect(result).rejects.toMatchObject({ name: "TimeoutError" });
    await vi.advanceTimersByTimeAsync(10_000); await rejected;
    expect(vi.getTimerCount()).toBe(0);
  });
  it("processes a flood without parallel catalog requests or dropping the trailing refresh", async () => {
    vi.useFakeTimers();
    const releases: (() => void)[] = [];
    let active = 0, peak = 0, calls = 0;
    const refresh = new CoalescedRefresh(() => { active++; peak = Math.max(peak, active); calls++; return new Promise<void>(resolve => releases.push(() => { active--; resolve(); })); }, () => {});
    const result = refresh.request();
    for (let i = 0; i < 1000; i++) void refresh.request();
    expect(calls).toBe(1);
    releases.shift()!(); await Promise.resolve(); await Promise.resolve();
    await vi.advanceTimersByTimeAsync(1000);
    expect(calls).toBe(2);
    releases.shift()!(); await result;
    expect(peak).toBe(1); expect(calls).toBe(2);
  });
  it("caps a sustained event flood to one catalog start per second", async () => {
    vi.useFakeTimers();
    const starts: number[] = [];
    const refresh = new CoalescedRefresh(async () => { starts.push(Date.now()); }, () => {});
    for (let i = 0; i < 100; i++) { void refresh.request(); await vi.advanceTimersByTimeAsync(10); }
    await vi.advanceTimersByTimeAsync(1000);
    expect(starts.length).toBeLessThanOrEqual(3);
    expect(starts.every((at, i) => i === 0 || at - starts[i - 1]! >= 1000)).toBe(true);
  });
  it("dedupes replay, accepts enrichment revisions, detects gaps and resets on epoch change", () => {
    const cursor = new EventRecovery();
    expect(cursor.begin("epoch-1", 1, 3)).toBeUndefined();
    expect(cursor.accept(1, 0)).toEqual({ deliver: true, gap: false });
    expect(cursor.accept(1, 0).deliver).toBe(false);
    expect(cursor.accept(3, 0)).toEqual({ deliver: true, gap: true });
    expect(cursor.accept(1, 1).deliver).toBe(true);
    expect(cursor.accept(2, 0).deliver).toBe(true);
    expect(cursor.begin("epoch-1", 10, 12)).toBe("retention_gap");
    expect(cursor.begin("epoch-2", 1, 1)).toBe("epoch_changed");
    expect(cursor.accept(1, 0).deliver).toBe(true);
  });
  it("exports a bounded typed ledger with recovery, never raw diagnostics or secrets", () => {
    const ledger = new ConnectionDiagnostics();
    for (let i = 0; i < 1000; i++) ledger.record({ phase: "backoff", stage: "auth", since: i, attempt: i, missedHeartbeats: 0, degraded: false,
      reason: "secret-token prompt-text", cause: { code: "network_or_browser_policy_unknown", layer: "browser", retryable: true, permission: "unknown" } }, i);
    ledger.record({ phase: "live", stage: "live", since: 1001, attempt: 0, missedHeartbeats: 0, degraded: false, lastSuccessAt: 1001 }, 1001);
    const report = ledger.export({ tailscale_status: { secret: "secret-token" }, credential: "secret-token", connection_recovery: { counts: Array.from({ length: 1000 }, () => ({ category: "secret-token", count: 1, status: 401, request_id: "secret-token", prompt: "prompt-text" })), refusals: { "secret-token": 4, revoked: 2 } } }, true);
    const json = JSON.stringify(report);
    expect(json).not.toMatch(/secret-token|prompt-text|tailscale_status|credential/);
    expect(json.length).toBeLessThan(65_536);
    expect((report.transitions as unknown[]).length).toBe(64);
    expect((report.transitions as Record<string, unknown>[]).at(-1)?.recovered).toBe("network_or_browser_policy_unknown");
    expect((report.hub as Record<string, unknown>).refusals).toEqual({ revoked: 2 });
  });
});
