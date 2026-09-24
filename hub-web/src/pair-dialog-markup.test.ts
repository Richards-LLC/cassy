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
