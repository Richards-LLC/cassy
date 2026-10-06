// @vitest-environment jsdom
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { openInstallationInventory, type InstallationSummary } from "./installation-inventory";
import type { StoredMachine } from "./types";

beforeAll(() => {
  HTMLDialogElement.prototype.showModal = function () { this.open = true; };
  HTMLDialogElement.prototype.close = function () { this.open = false; this.dispatchEvent(new Event("close")); };
});
afterEach(() => { document.body.innerHTML = ""; vi.useRealTimers(); vi.restoreAllMocks(); });

const machine: StoredMachine = {
  id: "atlas", label: "Atlas", baseUrl: "https://atlas.test", deviceId: "own-id",
  credentialId: "credential", credential: "fixture", expiresAt: "2027-01-01T00:00:00Z",
  scopes: ["machine-read", "hub-admin"], publicKey: {}, privateKey: {} as CryptoKey,
};
const own: InstallationSummary = {
  device_id: "own-id", device_label: "Work laptop", operator_label: "Daniel",
  controller_origin: "https://commander.example", credential_generation: 7,
  key_fingerprint: "opaque-signing-fingerprint", issued_at: "2026-10-05T12:00:00Z",
  last_used_at: "2026-10-05T14:00:00Z", revoked_at: null,
  account_enrollment: { state: "enrolled", account_id: "account", relay_device_id: "relay", grant_generation: "2", feed_generation: "1", epoch: "4", verified_at: "2026-10-05T14:00:00Z" },
};

async function inventory(rows: InstallationSummary[]) {
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(new Date("2026-10-05T16:00:00Z"));
  const request = vi.fn().mockResolvedValueOnce(rows).mockResolvedValue(undefined);
  const client = { request: async <T>(method: string, path: string): Promise<T> => await request(method, path) as T };
  const ownRevoke = vi.fn(async () => {});
  await openInstallationInventory(document, machine, client, ownRevoke);
  return { request, ownRevoke, dialog: document.querySelector<HTMLDialogElement>(".installation-inventory")! };
}

describe("recognizable browser installations (cas-2e77)", () => {
  it("leads with browser names and ownership, with internals only in closed details", async () => {
    const { dialog } = await inventory([{ ...own, device_id: "peer-id", device_label: "Old tablet", account_enrollment: { state: "unenrolled" } }, own]);
    expect([...dialog.querySelectorAll("h3")].map(node => node.textContent)).toEqual(["Work laptop · This browser", "Old tablet · Another browser"]);
    const visible = dialog.cloneNode(true) as HTMLElement;
    visible.querySelectorAll("details").forEach(node => node.remove());
    expect(visible.textContent).toContain("Daniel");
    expect(visible.textContent).toContain("In your operator inbox");
    expect(visible.textContent).toContain("Active");
    expect(visible.textContent).not.toMatch(/Generation|generation|Signing key|key epoch|own-id|peer-id|opaque-signing|\d{4}-\d{2}-\d{2}T/);
    const details = dialog.querySelector<HTMLDetailsElement>("details")!;
    expect(details.open).toBe(false);
    expect(details.querySelector("summary")?.textContent).toBe("Technical details");
    expect(details.textContent).toContain("own-id");
    expect(details.textContent).toContain("Credential generation7");
    expect(details.textContent).toContain("Signing keyopaque-signing-fingerprint");
    expect(details.textContent).toContain("Account key epoch4");
  });

  it("uses relative activity with a local full time, and handles missing or invalid dates", async () => {
    const { dialog } = await inventory([own, { ...own, device_id: "unknown", issued_at: "", last_used_at: "bad date" }]);
    const times = [...dialog.querySelectorAll("time")];
    expect(times.map(node => node.textContent)).toEqual(["4 hours ago", "2 hours ago"]);
    expect(times[1]?.dateTime).toBe(own.last_used_at);
    expect(times[1]?.title).toBeTruthy();
    expect(times[1]?.title).not.toContain("T14:00:00Z");
    const unknown = dialog.querySelectorAll(".installation-inventory-row")[1]!;
    expect(unknown.querySelector("time")).toBeNull();
    const visible = unknown.cloneNode(true) as HTMLElement;
    visible.querySelector("details")?.remove();
    expect(visible.textContent?.match(/Not recorded/g)).toHaveLength(2);
    expect(visible.textContent).not.toMatch(/NaN|Invalid Date/);
  });

  it("revokes the consented exact peer ID while keeping human access state and status consistent", async () => {
    const peer = { ...own, device_id: "peer/id", device_label: "  Old tablet  " };
    const { dialog, request, ownRevoke } = await inventory([peer]);
    const consent = vi.spyOn(window, "confirm").mockReturnValue(true);
    const button = dialog.querySelector<HTMLButtonElement>(".installation-inventory-row button")!;
    button.click();
    await vi.waitFor(() => expect(button.textContent).toBe("Revoked"));
    expect(consent).toHaveBeenCalledWith(expect.stringContaining("Old tablet (peer/id)"));
    expect(request).toHaveBeenLastCalledWith("POST", "/v1/auth/devices/peer%2Fid/revoke");
    expect(ownRevoke).not.toHaveBeenCalled();
    expect(button.disabled).toBe(true);
    const visible = dialog.cloneNode(true) as HTMLElement;
    visible.querySelector("details")?.remove();
    expect(visible.textContent).toContain("Revoked Old tablet. Its live connections are closing.");
    expect(visible.textContent).not.toContain("Active");
  });

  it("clamps hub activity ahead of this browser's clock while retaining the exact local and UTC times (QA F03)", async () => {
    const future = { ...own, issued_at: "2026-10-09T12:00:00Z", last_used_at: "2026-10-10T14:00:00Z" };
    const { dialog } = await inventory([future]);
    const times = [...dialog.querySelectorAll("time")];
    expect(times.map(node => node.textContent)).toEqual(["Just now", "Just now"]);
    expect(times.map(node => node.dateTime)).toEqual([future.issued_at, future.last_used_at]);
    expect(times[0]?.title).toBe(new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" }).format(Date.parse(future.issued_at)));
    expect(dialog.querySelector("details")?.textContent).toContain(future.last_used_at);
  });

  it("never revokes without consent and retains own-browser cleanup", async () => {
    const { dialog, request, ownRevoke } = await inventory([own]);
    const consent = vi.spyOn(window, "confirm").mockReturnValue(false);
    const button = dialog.querySelector<HTMLButtonElement>(".installation-inventory-row button")!;
    button.click();
    expect(request).toHaveBeenCalledTimes(1);
    expect(ownRevoke).not.toHaveBeenCalled();
    consent.mockReturnValue(true);
    button.click();
    await vi.waitFor(() => expect(ownRevoke).toHaveBeenCalledTimes(1));
    expect(request).toHaveBeenLastCalledWith("POST", "/v1/auth/devices/own-id/revoke");
    expect(document.querySelector(".installation-inventory")).toBeNull();
  });
});
