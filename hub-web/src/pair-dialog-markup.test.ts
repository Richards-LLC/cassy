// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { firstEmptyField, pairDialogMarkup } from "./pair-dialog-markup";
import { createPairingDraft } from "./pairing-draft";

describe("invitation scope ceiling in the actual pairing form", () => {
  it("names factory:manage in plain words and requests it when a link grants it (cas-d382)", () => {
    const origin = "https://commander.example";
    const granted = ["machine-read", "session-read", "pane-read", "pane-input", "message-send", "pane-interrupt", "factory-manage"] as const;
    const html = pairDialogMarkup({
      cleanupFailed: false, cleanupContext: { cause: "cancel", storeOpen: false, rollbackPending: false },
      pendingPairing: { kind: "invitation", token: "A".repeat(43), hubId: "studio", scopes: [...granted] },
      draft: createPairingDraft(origin, [...granted]), status: "", createInFlight: false, exchangeInFlight: false,
      relayOrigin: origin, pageOrigin: origin,
    });
    const doc = new DOMParser().parseFromString(html, "text/html");
    expect(doc.querySelector(".pair-lead")?.textContent).toBe("This browser will be able to: See its sessions and raw output · Type, send messages and interrupt · Stop and restart workers and sessions");
    const manage = doc.querySelector<HTMLInputElement>('input[name="scope"][value="factory-manage"]');
    expect(manage?.checked).toBe(true);
    expect(manage?.disabled).toBe(false);
    expect(new FormData(doc.querySelector("form")!).getAll("scope")).toEqual([...granted]);
    // A full-control link with factory:manage withholds nothing.
    expect(doc.querySelector(".pair-withheld")).toBeNull();
  });

  it("disables and excludes ungranted control scopes even when the draft selects them", () => {
    const origin = "https://commander.example";
    const html = pairDialogMarkup({
      cleanupFailed: false, cleanupContext: { cause: "cancel", storeOpen: false, rollbackPending: false },
      pendingPairing: { kind: "invitation", token: "A".repeat(43), hubId: "studio", scopes: ["machine-read", "session-read", "pane-read"] },
      draft: createPairingDraft(origin), status: "", createInFlight: false, exchangeInFlight: false,
      relayOrigin: origin, pageOrigin: origin,
    });
    const doc = new DOMParser().parseFromString(html, "text/html");
    const inputs = [...doc.querySelectorAll<HTMLInputElement>('input[name="scope"]')];
    expect(inputs).toHaveLength(6);
    for (const input of inputs) {
      const granted = ["machine-read", "session-read", "pane-read"].includes(input.value);
      expect(input.disabled, input.value).toBe(!granted);
      expect(input.checked, input.value).toBe(granted);
      if (!granted) expect(input.closest("label")?.textContent).toContain("not granted by this invitation");
    }
    expect(new FormData(doc.querySelector("form")!).getAll("scope")).toEqual(["machine-read", "session-read", "pane-read"]);
    const command = doc.querySelector<HTMLButtonElement>("#pair-copy")?.dataset.pairCommand;
    expect(command).toBe("cas hub pair --origin https://commander.example --scopes machine:read,session:read,pane:read,pane:input,message:send,pane:interrupt");
  });

  it("names what a read-only link withholds, and the command, beside what it grants rather than inside Technical details (cas-b52d, journey F26)", () => {
    const origin = "https://commander.example";
    const render = (scopes?: ("machine-read" | "session-read" | "pane-read" | "pane-input" | "message-send" | "pane-interrupt")[]) => new DOMParser().parseFromString(pairDialogMarkup({
      cleanupFailed: false, cleanupContext: { cause: "cancel", storeOpen: false, rollbackPending: false },
      pendingPairing: { kind: "invitation", token: "A".repeat(43), hubId: "studio", scopes },
      draft: createPairingDraft(origin), status: "", createInFlight: false, exchangeInFlight: false,
      relayOrigin: origin, pageOrigin: origin,
    }), "text/html");
    const doc = render(["machine-read", "session-read", "pane-read"]);
    const withheld = doc.querySelector(".pair-withheld")!;
    expect(withheld.querySelector(".pair-lead")?.textContent).toBe("This link does not let it: Type, send messages and interrupt");
    // Straight after "This browser will be able to…", before any field, and not in the disclosure.
    expect(withheld.previousElementSibling?.textContent).toBe("This browser will be able to: See its sessions and raw output");
    expect(withheld.closest("details")).toBeNull();
    expect(withheld.compareDocumentPosition(doc.querySelector('input[name="url"]')!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(withheld.querySelector("code")?.textContent).toBe("cas hub pair --origin https://commander.example --scopes machine:read,session:read,pane:read,pane:input,message:send,pane:interrupt");
    expect(withheld.querySelector("#pair-copy")?.textContent).toBe("Copy command");
    expect(doc.querySelectorAll("#pair-copy")).toHaveLength(1);
    // A link that grants everything, or one that declares no ceiling, withholds nothing.
    expect(render(["machine-read", "session-read", "pane-read", "pane-input", "message-send", "pane-interrupt"]).querySelector(".pair-withheld")).toBeNull();
    expect(render(undefined).querySelector(".pair-withheld")).toBeNull();
  });
});

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

describe("Technical details open state", () => {
  it("stays open on the step where it was opened and starts closed on the next", () => {
    const origin = "https://commander.example";
    const base = { cleanupFailed: false, cleanupContext: { cause: "cancel" as const, storeOpen: false, rollbackPending: false }, status: "", createInFlight: false, exchangeInFlight: false, relayOrigin: origin, pageOrigin: origin };
    const draft = { ...createPairingDraft(origin), technicalOpen: "create" as const };
    const create = new DOMParser().parseFromString(pairDialogMarkup({ ...base, pendingPairing: null, draft }), "text/html");
    expect(create.querySelector("details.pair-technical")?.hasAttribute("open")).toBe(true);
    const code = new DOMParser().parseFromString(pairDialogMarkup({ ...base, pendingPairing: { kind: "relay-request", pairingRequestId: "r", userCode: "KQ7M-4XTR", pollSecret: "p", controllerOrigin: origin, requestedScopes: ["machine-read"], expiresAt: new Date(Date.now() + 600_000).toISOString(), interval: 3 }, draft }), "text/html");
    expect(code.querySelector("details.pair-technical")?.hasAttribute("open")).toBe(false);
  });
});

describe("the Re-pair dialog's command (cas-093d F02)", () => {
  const origin = "https://hub.example";
  const command = "cas hub pair --origin https://hub.example --scopes machine:read,session:read,pane:read,pane:input,message:send,pane:interrupt,session:launch";
  const base = { cleanupFailed: false, cleanupContext: { cause: "cancel" as const, storeOpen: false, rollbackPending: false }, pendingPairing: null, draft: createPairingDraft(origin), status: "Re-pairing Atlas · Linux: … To keep it, run this on Atlas · Linux and open the link it prints instead:", createInFlight: false, exchangeInFlight: false, relayOrigin: origin, pageOrigin: origin };
  it("is its own code token with a labelled Copy and an in-dialog announcement, after the status", () => {
    const doc = new DOMParser().parseFromString(pairDialogMarkup({ ...base, repairCommand: command }), "text/html");
    const block = doc.querySelector(".pair-command")!;
    const code = block.querySelector("code.pair-command-token")!;
    expect(code.textContent).toBe(command);
    // QA F01: every word is its own unbreakable span; only the spaces between them can break.
    expect([...code.querySelectorAll(".pair-command-word")].map((word) => word.textContent)).toEqual(command.split(" "));
    const copy = block.querySelector<HTMLButtonElement>("button.pair-command-copy")!;
    expect(copy.dataset.pairCommand).toBe(command);
    // QA F02: the visible label is the accessible name; the command describes it.
    expect(copy.hasAttribute("aria-label")).toBe(false);
    expect(copy.textContent).toBe("Copy command");
    expect(copy.getAttribute("aria-describedby")).toBe(code.id);
    expect(block.querySelector('.pair-command-status[role="status"]')).not.toBeNull();
    expect(doc.querySelector(".pair-status")!.compareDocumentPosition(block) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(doc.querySelector(".pair-status")!.textContent).not.toContain("cas hub pair");
  });
  it("is absent when there is no command to keep", () => {
    const doc = new DOMParser().parseFromString(pairDialogMarkup(base), "text/html");
    expect(doc.querySelector(".pair-command")).toBeNull();
  });
});

describe("the pairing dialog's name and admin consent (cas-d043 G08, G11)", () => {
  const origin = "https://commander.example";
  const base = { cleanupFailed: false, cleanupContext: { cause: "cancel" as const, storeOpen: false, rollbackPending: false }, status: "", createInFlight: false, exchangeInFlight: false, relayOrigin: origin, pageOrigin: origin };
  const parse = (html: string) => new DOMParser().parseFromString(html, "text/html");
  const nameOf = (doc: Document) => {
    const dialog = doc.querySelector("dialog")!;
    return doc.getElementById(dialog.getAttribute("aria-labelledby") ?? "")?.textContent;
  };

  it("names every variant of the dialog by its heading", () => {
    const draft = createPairingDraft(origin);
    expect(nameOf(parse(pairDialogMarkup({ ...base, pendingPairing: null, draft })))).toBe("Pair a machine");
    expect(nameOf(parse(pairDialogMarkup({ ...base, pendingPairing: { kind: "invitation", token: "A".repeat(43), hubId: "atlas", scopes: ["machine-read"] }, draft })))).toBe("Pair a machine");
    expect(nameOf(parse(pairDialogMarkup({ ...base, cleanupFailed: true, pendingPairing: null, draft })))).toBeTruthy();
  });

  it("labels hub:admin in plain words and claims it only once ticked", () => {
    const granted = ["machine-read", "session-read", "pane-read", "hub-admin"] as const;
    const draft = { ...createPairingDraft(origin, ["machine-read", "session-read", "pane-read"]), machineLabel: "Atlas" };
    const doc = parse(pairDialogMarkup({ ...base, pendingPairing: { kind: "invitation", token: "A".repeat(43), hubId: "atlas", scopes: [...granted] }, draft }));
    const label = doc.querySelector(".pair-admin-consent label.scope")!;
    expect(label.firstChild?.nextSibling?.textContent).toBe("See and revoke other browsers on Atlas");
    expect(doc.querySelector<HTMLInputElement>('.pair-admin-consent input[value="hub-admin"]')!.checked).toBe(false);
    // The lead names the power in a part CSS shows only while the box is ticked.
    const admin = doc.querySelector(".pair-summary .pair-summary-admin");
    expect(admin?.textContent).toBe(" · See and revoke other browsers on Atlas");
    expect(doc.querySelector(".pair-summary")!.textContent!.replace(admin!.textContent!, "")).not.toContain("revoke");
  });
});
