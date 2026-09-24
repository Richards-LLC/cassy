// @vitest-environment jsdom
import { webcrypto } from "node:crypto";
import { afterEach, describe, expect, it, vi } from "vitest";
import { artifactIdFromHref, artifactLinkFor, artifactOpenFailure, openArtifact, type ArtifactViewResult } from "./artifact-open";
import { HubConnectionSupervisor, type HubCallbacks } from "./connection";
import { createDeviceKey } from "./dpop";
import type { StoredMachine } from "./types";

Object.defineProperty(globalThis, "crypto", { value: webcrypto, configurable: true });

afterEach(() => {
  vi.unstubAllGlobals();
});

/** A stand-in for the tab `window.open` returns. */
function fakeTab() {
  return { opener: {} as unknown, closed: false, location: { href: "about:blank" }, document: { title: "" }, close() { this.closed = true; } };
}

describe("opening an artifact from Commander (cassy#910)", () => {
  it("reads the artifact id from the thread's #artifact: links only", () => {
    expect(artifactIdFromHref("#artifact:art-7f3a9c21")).toBe("art-7f3a9c21");
    expect(artifactIdFromHref("https://hub.example/commander/#artifact:report%2F3.26.0%2Fbrief.pdf")).toBe("report/3.26.0/brief.pdf");
    expect(artifactIdFromHref("#artifact:")).toBeUndefined();
    expect(artifactIdFromHref("#artifact:%E0%A4%A")).toBeUndefined();
    expect(artifactIdFromHref("https://example.com/")).toBeUndefined();
    expect(artifactIdFromHref(null)).toBeUndefined();

    document.body.innerHTML = `<a id="sheet" href="#artifact:art-1"><span class="fname">brief.pdf</span></a><a id="other" href="https://example.com">x</a>`;
    expect(artifactLinkFor(document.querySelector(".fname"))?.id).toBe("sheet");
    expect(artifactLinkFor(document.querySelector("#other"))).toBeUndefined();
  });

  it("points the tab it opened in the tap at the signed URL", async () => {
    const tab = fakeTab();
    const notify = vi.fn();
    let resolveView!: (result: ArtifactViewResult) => void;
    const opened = openArtifact({
      fetchView: () => new Promise((resolve) => { resolveView = resolve; }),
      openWindow: () => tab as unknown as Window,
      notify,
      machineLabel: "Atlas · Linux",
    });
    // The tab is open before the request settles: a browser blocks a window
    // opened after an await.
    expect(tab.opener).toBeNull();
    expect(tab.document.title).toBe("Opening file…");
    resolveView({ ok: true, view: { artifact_id: "art-1", url: "https://store.private.blob.example/report.pdf?sig=abc" } });
    await expect(opened).resolves.toBe(true);
    expect(tab.location.href).toBe("https://store.private.blob.example/report.pdf?sig=abc");
    expect(tab.closed).toBe(false);
    expect(notify).not.toHaveBeenCalled();
  });

  it("closes the tab and says why when the machine has no view URL", async () => {
    for (const [result, words] of [
      [{ ok: false, status: 409, code: "artifact_not_in_cloud", detail: "local" }, "only saved on Atlas · Linux"],
      [{ ok: false, status: 409, code: "artifact_not_committed", detail: "pending" }, "still checking"],
      [{ ok: false, status: 503, code: "cloud_not_configured" }, "isn't signed in to Cassy Cloud"],
      [{ ok: false, status: 503, code: "cloud_storage_not_live" }, "storage isn't available yet"],
      [{ ok: false, status: 404, code: "not_found" }, "isn't available any more"],
      [{ ok: false, status: 403 }, "Pair it again"],
      [{ ok: false, status: 502, code: "cloud_failed" }, "didn't open. Try again"],
    ] as const) {
      const tab = fakeTab();
      const notify = vi.fn();
      const opened = await openArtifact({ fetchView: async () => result, openWindow: () => tab as unknown as Window, notify, machineLabel: "Atlas · Linux" });
      expect(opened).toBe(false);
      expect(tab.closed).toBe(true);
      expect(notify).toHaveBeenCalledTimes(1);
      expect(notify.mock.calls[0]?.[0]).toContain(words);
      expect(artifactOpenFailure(result, "Atlas · Linux")).not.toMatch(/cloud_|artifact_|409|503/);
    }
  });

  it("treats a failed request like any other refusal and names a blocked tab", async () => {
    const tab = fakeTab();
    const notify = vi.fn();
    await openArtifact({ fetchView: async () => { throw new Error("network"); }, openWindow: () => tab as unknown as Window, notify, machineLabel: "Atlas" });
    expect(tab.closed).toBe(true);
    expect(notify).toHaveBeenLastCalledWith("The file didn't open. Try again.");

    await openArtifact({ fetchView: async () => ({ ok: true, view: { artifact_id: "a", url: "https://x.example/" } }), openWindow: () => null, notify, machineLabel: "Atlas" });
    expect(notify).toHaveBeenLastCalledWith(expect.stringContaining("blocked the new tab"));
  });

  it("asks the machine with a DPoP proof and reads a refusal as an answer", async () => {
    const { privateKey, publicKey } = await createDeviceKey();
    const machine = {
      id: "atlas", label: "Atlas · Linux", baseUrl: "https://atlas.test", deviceId: "device",
      credentialId: "credential-id", credential: "opaque", expiresAt: new Date(Date.now() + 60_000).toISOString(),
      scopes: ["session-read"], publicKey, privateKey,
    } satisfies StoredMachine;
    const seen: Array<{ url: string; dpop: string | undefined }> = [];
    vi.stubGlobal("fetch", async (input: URL | string, init?: RequestInit) => {
      const url = String(input);
      seen.push({ url, dpop: (init?.headers as Record<string, string> | undefined)?.DPoP });
      if (url.includes("art-ok")) {
        return new Response(JSON.stringify({ artifact_id: "art-ok", url: "https://store.example/v?sig=1", expires_at: "2026-09-24T21:10:00Z" }), { status: 200 });
      }
      return new Response(JSON.stringify({ error: "artifact_not_in_cloud", status: "local" }), { status: 409 });
    });
    const connection = new HubConnectionSupervisor(machine, {} as HubCallbacks);

    const ok = await connection.artifactView("patient pelican", "art-ok");
    expect(ok).toEqual({ ok: true, view: { artifact_id: "art-ok", url: "https://store.example/v?sig=1", expires_at: "2026-09-24T21:10:00Z" } });
    expect(seen[0]?.url).toBe("https://atlas.test/v1/sessions/patient%20pelican/artifacts/art-ok/url");
    expect(seen[0]?.dpop).toBeTruthy();

    const refused = await connection.artifactView("patient pelican", "art-local");
    expect(refused).toEqual({ ok: false, status: 409, code: "artifact_not_in_cloud", detail: "local" });
  });
});
