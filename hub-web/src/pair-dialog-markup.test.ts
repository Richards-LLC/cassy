// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { firstEmptyField, pairDialogMarkup } from "./pair-dialog-markup";
import { createPairingDraft } from "./pairing-draft";

describe("pairing dialog for a `cas hub pair` link (HUB-J2)", () => {
  it("opens a prefilled link on the operator's name, with the address help behind a disclosure", () => {
    const origin = "https://commander.example";
    const pendingPairing = { kind: "invitation" as const, token: "A".repeat(43), hubId: "m", suggestedHubUrl: "https://studio.tail.ts.net", suggestedMachineLabel: "Studio" };
    const draft = createPairingDraft(origin, undefined, pendingPairing);
    const html = pairDialogMarkup({ cleanupFailed: false, cleanupContext: { cause: "cancel", storeOpen: false, rollbackPending: false }, pendingPairing, draft, status: "", createInFlight: false, exchangeInFlight: false, relayOrigin: origin, pageOrigin: origin });
    const doc = new DOMParser().parseFromString(html, "text/html");
    expect(doc.querySelector<HTMLInputElement>('input[name="url"]')?.value).toBe("https://studio.tail.ts.net");
    expect(doc.querySelector<HTMLInputElement>('input[name="label"]')?.value).toBe("Studio");
    expect(doc.querySelector('input[name="url"]')?.hasAttribute("readonly")).toBe(false);
    expect([...doc.querySelectorAll("[autofocus]")].map((element) => element.getAttribute("name"))).toEqual(["operator"]);
    const help = doc.querySelector("details.pair-address-help");
    expect(help?.hasAttribute("open")).toBe(false);
    expect(help?.querySelector("summary")?.textContent).toBe("Where do I find this?");
    const usePage = help?.querySelector<HTMLButtonElement>("#pair-use-page-origin");
    expect(usePage?.classList.contains("secondary")).toBe(true);
    expect(usePage?.textContent).toBe("Use this page's address");
    expect(usePage?.dataset.pageOrigin).toBe(origin);
    // Once opened, a re-render keeps it open.
    const reopened = new DOMParser().parseFromString(pairDialogMarkup({ cleanupFailed: false, cleanupContext: { cause: "cancel", storeOpen: false, rollbackPending: false }, pendingPairing, draft: { ...draft, addressHelpOpen: true }, status: "", createInFlight: false, exchangeInFlight: false, relayOrigin: origin, pageOrigin: origin }), "text/html");
    expect(reopened.querySelector("details.pair-address-help")?.hasAttribute("open")).toBe(true);

    expect(firstEmptyField(createPairingDraft(origin), false)).toBe("url");
    expect(firstEmptyField({ ...createPairingDraft(origin), hubUrl: "https://a.example" }, false)).toBe("label");
    expect(firstEmptyField(createPairingDraft(origin), true)).toBe("operator");
    expect(firstEmptyField({ ...draft, operatorLabel: "Daniel" }, false)).toBe("operator");
  });
});

describe("pairing dialog in plain words (F3)", () => {
  const origin = "https://commander.example";
  const base = { cleanupFailed: false, cleanupContext: { cause: "cancel" as const, storeOpen: false, rollbackPending: false }, status: "", createInFlight: false, exchangeInFlight: false, relayOrigin: origin, pageOrigin: origin };
  const scopes = ["machine-read", "session-read", "pane-read", "pane-input", "message-send", "pane-interrupt"] as const;
  const steps = {
    create: pairDialogMarkup({ ...base, pendingPairing: null, draft: createPairingDraft(origin) }),
    code: pairDialogMarkup({ ...base, pendingPairing: { kind: "relay-request", pairingRequestId: "r", userCode: "KQ7M-4XTR", pollSecret: "p", controllerOrigin: origin, requestedScopes: [...scopes], expiresAt: new Date(Date.now() + 600_000).toISOString(), interval: 3 }, draft: createPairingDraft(origin) }),
    authorized: pairDialogMarkup({ ...base, pendingPairing: { kind: "invitation", token: "A".repeat(43), hubId: "atlas", hubUrl: "https://atlas.test", machineLabel: "Atlas", controllerOrigin: origin, scopes: [...scopes], relay: { pairingRequestId: "r", pollSecret: "p", deliveryId: "d" } }, draft: createPairingDraft(origin, scopes) }),
    link: pairDialogMarkup({ ...base, pendingPairing: { kind: "invitation", token: "A".repeat(43), hubId: "atlas", scopes: [...scopes] }, draft: createPairingDraft(origin, scopes) }),
  };

  it.each(Object.entries(steps))("%s: leads with what the browser can do and keeps the exact origin and scopes collapsed", (_, html) => {
    const doc = new DOMParser().parseFromString(html, "text/html");
    const dialog = doc.querySelector("#pair-dialog")!;
    const lead = dialog.querySelector(".pair-lead");
    expect(lead?.textContent).toMatch(/^This browser will be able to: /);
    // The lead comes before any field, command, or detail.
    expect(dialog.querySelector("h2")?.nextElementSibling).toBe(lead);
    const technical = dialog.querySelector("details.pair-technical");
    expect(technical?.hasAttribute("open")).toBe(false);
    expect(technical?.querySelector("summary")?.textContent).toBe("Technical details");
    // Raw scope names and the origin appear only inside Technical details.
    const outside = dialog.cloneNode(true) as Element;
    outside.querySelector("details.pair-technical")?.remove();
    expect(outside.textContent).not.toMatch(/machine:read|pane:interrupt/);
    expect(outside.textContent).not.toMatch(/Cassy Cloud origin|device credential|target hub|Operator label|Device label|Email code/i);
    expect(technical?.textContent).toContain("machine:read");
  });

  it("names the fields in the operator's words", () => {
    for (const html of [steps.authorized, steps.link]) {
      const doc = new DOMParser().parseFromString(html, "text/html");
      const labels = [...doc.querySelectorAll("label")].map((label) => label.firstChild?.textContent);
      expect(labels).toContain("Your name (shown on the machine)");
      expect(labels).toContain("Name for this browser");
    }
    const create = new DOMParser().parseFromString(steps.create, "text/html");
    expect(create.querySelector("label:has(#pair-email)")?.firstChild?.textContent).toBe("Email me the code too (optional)");
  });
});
