// @vitest-environment jsdom
import { webcrypto } from "node:crypto";
import { afterEach, describe, expect, it, vi } from "vitest";
import { artifactFailureFollowsConnection, artifactFailureIsAboutTheFile, artifactIdFromHref, artifactIsLocalOnly, artifactLinkFor, artifactOpenFailure, openArtifact, type ArtifactViewResult } from "./artifact-open";
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
    expect(tab.document.title).toBeTruthy();
    resolveView({ ok: true, view: { artifact_id: "art-1", url: "https://store.private.blob.example/report.pdf?sig=abc" } });
    await expect(opened).resolves.toBe(true);
    expect(tab.location.href).toBe("https://store.private.blob.example/report.pdf?sig=abc");
    expect(tab.closed).toBe(false);
    expect(notify).not.toHaveBeenCalled();
  });

  it("closes the tab and says why when the machine has no view URL", async () => {
    for (const result of [
      { ok: false, status: 409, code: "artifact_not_in_cloud", detail: "local" },
      { ok: false, status: 409, code: "artifact_not_committed", detail: "pending" },
      { ok: false, status: 503, code: "cloud_not_configured" },
      { ok: false, status: 503, code: "cloud_storage_not_live" },
      { ok: false, status: 404, code: "not_found" },
      { ok: false, status: 403 },
      { ok: false, status: 502, code: "cloud_failed" },
      { ok: false, status: 0 },
      { ok: false, status: 500 },
    ] as const) {
      const tab = fakeTab();
      const notify = vi.fn();
      const opened = await openArtifact({ fetchView: async () => result, openWindow: () => tab as unknown as Window, notify, machineLabel: "Atlas · Linux" });
      expect(opened).toBe(false);
      expect(tab.closed).toBe(true);
      expect(notify).toHaveBeenCalledTimes(1);
      expect(notify.mock.calls[0]?.[0]).toEqual(expect.any(String));
    }
  });

  it("reads a request that never reached the machine as an unreachable machine, and names a blocked tab", async () => {
    const tab = fakeTab();
    const notify = vi.fn();
    await openArtifact({ fetchView: async () => { throw new Error("network"); }, openWindow: () => tab as unknown as Window, notify, machineLabel: "Atlas" });
    expect(tab.closed).toBe(true);
    expect(notify).toHaveBeenLastCalledWith(expect.any(String), { ok: false, status: 0 });

    await openArtifact({ fetchView: async () => ({ ok: true, view: { artifact_id: "a", url: "https://x.example/" } }), openWindow: () => null, notify, machineLabel: "Atlas" });
    expect(notify).toHaveBeenLastCalledWith(expect.any(String), expect.objectContaining({ ok: true }));
  });

  it("says a file card's failure without contradicting a live connection, and without 'tap' (journey F6)", () => {
    const silent = { ok: false, status: 0 } as const;
    expect(artifactOpenFailure(silent, "Atlas · Linux", true)).toContain("Atlas · Linux");
    expect(artifactOpenFailure(silent, "Atlas · Linux", false)).toContain("Atlas · Linux");
    // Journey F28: while the header says Reconnecting, a card says it in the banner's words.
    expect(artifactOpenFailure(silent, "Atlas · Linux", "reconnecting")).toContain("Atlas · Linux");
    expect(artifactOpenFailure(silent, "Atlas · Linux", "live")).toBe(artifactOpenFailure(silent, "Atlas · Linux", true));
    expect(artifactOpenFailure(silent, "Atlas · Linux", "unreachable")).toBe(artifactOpenFailure(silent, "Atlas · Linux", false));
    // Only a no-answer failure follows the connection; Cloud's and the file's own reasons do not.
    expect(artifactFailureFollowsConnection(silent)).toBe(true);
    for (const result of [{ ok: false, status: 502, code: "cloud_failed" }, { ok: false, status: 409, code: "artifact_not_in_cloud" }, { ok: false, status: 403 }, { ok: false, status: 500 }] as const) {
      expect(artifactFailureFollowsConnection(result)).toBe(false);
      expect(artifactOpenFailure(result, "Atlas · Linux", "live")).toBe(artifactOpenFailure(result, "Atlas · Linux", "reconnecting"));
    }
    expect(artifactFailureFollowsConnection({ ok: true, view: { artifact_id: "a", url: "https://x.example/" } })).toBe(false);
    expect(artifactIsLocalOnly({ ok: false, status: 409, code: "artifact_not_in_cloud" })).toBe(true);
    expect(artifactIsLocalOnly({ ok: false, status: 0 })).toBe(false);
    expect(artifactIsLocalOnly(undefined)).toBe(false);
    // cas-c808 QA F01: only a failure about the file itself outlives a reconnect.
    expect(artifactFailureIsAboutTheFile({ ok: false, status: 409, code: "artifact_not_in_cloud" })).toBe(true);
    expect(artifactFailureIsAboutTheFile({ ok: false, status: 404, code: "not_found" })).toBe(true);
    for (const result of [silent, { ok: false, status: 502, code: "cloud_failed" }, { ok: false, status: 409, code: "artifact_not_committed" }, { ok: false, status: 500 }] as const) {
      expect(artifactFailureIsAboutTheFile(result)).toBe(false);
    }
    expect(artifactFailureIsAboutTheFile({ ok: true, view: { artifact_id: "a", url: "https://x.example/" } })).toBe(false);
  });

  it("opens no tab for a file already known to be only on the machine, and says when it reaches Cloud (journey F6)", async () => {
    const openWindow = vi.fn(() => fakeTab() as unknown as Window);
    const notify = vi.fn();
    const local: ArtifactViewResult = { ok: false, status: 409, code: "artifact_not_in_cloud" };
    await expect(openArtifact({ fetchView: async () => local, openWindow, notify, machineLabel: "Atlas · Linux", knownLocalOnly: true })).resolves.toBe(false);
    expect(openWindow).not.toHaveBeenCalled();
    expect(notify).toHaveBeenLastCalledWith(expect.any(String), local);
    const uploaded: ArtifactViewResult = { ok: true, view: { artifact_id: "a", url: "https://x.example/" } };
    await expect(openArtifact({ fetchView: async () => uploaded, openWindow, notify, machineLabel: "Atlas · Linux", knownLocalOnly: true })).resolves.toBe(false);
    expect(openWindow).not.toHaveBeenCalled();
    expect(notify).toHaveBeenLastCalledWith(expect.any(String), uploaded);
  });

  it("asks the machine with a DPoP proof and reads a refusal as an answer", async () => {
    const { privateKey, publicKey } = await createDeviceKey();
    const machine = {
      id: "atlas", label: "Atlas · Linux", baseUrl: "https://atlas.test", deviceId: "device",
      credentialId: "credential-id", credential: "opaque", expiresAt: new Date(Date.now() + 60_000).toISOString(),
      scopes: ["session-read"], publicKey, privateKey,
    } satisfies StoredMachine;
    const seen: Array<{ url: string; method?: string; dpop: string | undefined }> = [];
    vi.stubGlobal("fetch", async (input: URL | string, init?: RequestInit) => {
      const url = String(input);
      seen.push({ url, method: init?.method, dpop: (init?.headers as Record<string, string> | undefined)?.DPoP });
      if (url.includes("art-preview")) return new Response(JSON.stringify({ ticket: "opaque-ticket", expires_at: "2030-01-01T00:00:00Z", url: "https://tracker.test/ignored" }), { status: 200 });
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
    const preview = await connection.artifactPreview("patient pelican", "art-preview");
    expect(preview).toEqual({ ok: true, view: { artifact_id: "art-preview", url: "https://atlas.test/v1/sessions/patient%20pelican/artifacts/art-preview/content?t=opaque-ticket", expires_at: "2030-01-01T00:00:00.000Z" } });
    expect(seen.at(-1)?.url).toBe("https://atlas.test/v1/sessions/patient%20pelican/artifacts/art-preview/ticket");
    expect(seen.at(-1)?.method).toBe("POST");
    expect(seen.at(-1)?.dpop).toBeTruthy();
  });
});
