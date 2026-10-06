// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from "vitest";
import { applyLiveRegions, type LiveRegionView } from "./live-regions";

/**
 * The shapes render() emits. An invariant in invariants.test.ts pins these
 * selectors against the real template, so this fixture cannot drift into
 * testing markup the app does not ship.
 */
const SHELL = `
  <div class="conversation-shell thread-open">
    <aside class="conversation-context">
      <p class="status-stale" role="status" hidden></p>
      <div id="status-view"></div>
    </aside>
    <div id="conversation-composer-slot">
      <div class="message conversation-composer">
        <textarea id="message-text"></textarea>
        <div class="composer-actions"><button id="message-send" class="primary">Send message</button></div>
        <p id="message-status" class="message-status" role="status" hidden></p>
        <p id="message-delivery" class="message-delivery" role="status" hidden></p>
      </div>
    </div>
  </div>
  <dialog id="pair-dialog" open>
    <form id="pair-form">
      <label>Device label<input name="device" value="Phone"></label>
      <p class="pair-status" role="status" hidden></p>
      <div class="dialog-actions">
        <button id="pair-cancel" type="button">Cancel</button>
        <button type="submit" class="primary">Pair</button>
      </div>
    </form>
  </dialog>`;

/** The cleanup step: Cancel discarded the invitation but storage has not proved it. */
const CLEANUP_STEP = `
  <dialog id="pair-dialog" open>
    <section class="pair-flow pair-cleanup">
      <p class="pair-status" role="status" hidden></p>
      <div class="dialog-actions">
        <button id="pair-close" type="button" data-role="cleanup">Close</button>
        <button id="pair-cleanup-retry" type="button" class="primary">Retry cleanup</button>
      </div>
    </section>
  </dialog>`;

/** The create-code step, where the dialog holds a section and not a form. */
const CREATE_STEP = `
  <dialog id="pair-dialog" open>
    <section class="pair-flow">
      <label>Email code (optional)<input id="pair-email" type="email"></label>
      <p class="pair-status" role="status" hidden></p>
      <div class="dialog-actions">
        <button id="pair-close" type="button">Close</button>
        <button id="pair-create" type="button" class="primary">Create pairing code</button>
      </div>
    </section>
  </dialog>`;

/** A live, unremarkable session: nothing to say in any region. */
const live: LiveRegionView = {};

let root: HTMLElement;

beforeEach(() => {
  document.body.innerHTML = SHELL;
  root = document.body;
});

describe("live regions and node identity", () => {
  it("leaves the composer node itself untouched across repeated heartbeats", () => {
    const composer = root.querySelector("#message-text");
    composer!.setAttribute("data-instance", "first");

    for (let beat = 0; beat < 6; beat += 1) {
      applyLiveRegions(root, { ...live, ...(beat % 2 ? { staleNotice: `Not live — reconnecting. Showing the last state received ${beat}m ago.` } : {}) });
    }

    expect(root.querySelector("#message-text")).toBe(composer);
    expect(root.querySelector("#message-text")!.getAttribute("data-instance")).toBe("first");
  });

  it("does not blur a focused composer", () => {
    const composer = root.querySelector<HTMLTextAreaElement>("#message-text")!;
    let blurs = 0;
    composer.addEventListener("blur", () => { blurs += 1; });
    composer.focus();
    composer.value = "half a sentence";

    applyLiveRegions(root, live);
    applyLiveRegions(root, live);

    expect(document.activeElement).toBe(composer);
    expect(composer.value).toBe("half a sentence");
    expect(blurs).toBe(0);
  });
});

describe("ten seconds of heartbeats, measured the way the defect was", () => {
  /**
   * mighty-raven-39's instrument: a MutationObserver over the app container
   * counting how often #message-text is a different node, plus a blur counter
   * on whichever node is current. Against the old render() it read 6 and 6.
   */
  it("counts zero composer replacements and zero blurs across a typing window", () => {
    const composer = root.querySelector<HTMLTextAreaElement>("#message-text")!;
    let replacements = 0;
    let blurs = 0;
    let current: Element | null = composer;
    composer.addEventListener("blur", () => { blurs += 1; });
    const observer = new MutationObserver(() => {
      const node = root.querySelector("#message-text");
      if (node && node !== current) {
        replacements += 1;
        current = node;
      }
    });
    observer.observe(root, { childList: true, subtree: true });
    composer.focus();

    // Two heartbeats a second for ten seconds, with the operator typing
    // through all of them.
    for (let beat = 0; beat < 20; beat += 1) {
      composer.value += "a";
      applyLiveRegions(root, {
        ...(beat % 3 === 0 ? { sendReason: "The conversation is reconnecting." } : {}),
        staleNotice: `Not live — reconnecting. Showing the last state received ${beat}s ago.`,
      });
    }
    observer.takeRecords();
    observer.disconnect();

    expect(replacements).toBe(0);
    expect(blurs).toBe(0);
    expect(document.activeElement).toBe(composer);
    expect(composer.value).toHaveLength(20);
    // The regions kept moving the whole time — this is not a frozen page.
    expect(root.querySelector(".status-stale")!.textContent).toBe("Not live — reconnecting. Showing the last state received 19s ago.");
  });
});

describe("live region values", () => {
  it("shows and then clears the stale-hub notice", () => {
    applyLiveRegions(root, { ...live, staleNotice: "Not live — reconnecting. Showing the last state received 2m ago." });
    const stale = root.querySelector<HTMLElement>(".status-stale")!;
    expect(stale.hidden).toBe(false);
    expect(stale.textContent).toContain("Not live");

    applyLiveRegions(root, live);

    expect(stale.hidden).toBe(true);
    expect(stale.textContent).toBe("");
  });

  it("carries a send block onto the button without disabling it", () => {
    applyLiveRegions(root, { ...live, sendReason: "Take control to send a message" });

    const send = root.querySelector<HTMLButtonElement>("#message-send")!;
    expect(send.getAttribute("aria-disabled")).toBe("true");
    expect(send.dataset.disabledReason).toBe("Take control to send a message");
    // A disabled Send swallows the tap and reads as broken; the block is stated,
    // never enforced by the disabled attribute.
    expect(send.disabled).toBe(false);
  });

  it("clears a send block once control is granted", () => {
    applyLiveRegions(root, { ...live, sendReason: "Take control to send a message" });
    applyLiveRegions(root, live);

    const send = root.querySelector<HTMLButtonElement>("#message-send")!;
    expect(send.hasAttribute("aria-disabled")).toBe(false);
    expect(send.hasAttribute("data-disabled-reason")).toBe(false);
  });

  it("shows a message result and its error tone, then hides it again", () => {
    applyLiveRegions(root, { ...live, messageStatus: { text: "Message failed to send", error: true } });
    const status = root.querySelector<HTMLElement>("#message-status")!;
    expect(status.hidden).toBe(false);
    expect(status.className).toBe("message-status error");

    applyLiveRegions(root, live);

    expect(status.hidden).toBe(true);
    expect(status.className).toBe("message-status");
  });

  it("shows the delivery confirmation only while there is one", () => {
    applyLiveRegions(root, { ...live, delivery: "Message sent to fast-kestrel-6" });
    const delivery = root.querySelector<HTMLElement>("#message-delivery")!;
    expect(delivery.hidden).toBe(false);

    applyLiveRegions(root, live);

    expect(delivery.hidden).toBe(true);
  });

  it("ignores a shell that does not carry the optional regions", () => {
    document.body.innerHTML = '<div class="conversation-shell"></div>';

    expect(() => applyLiveRegions(document.body, { ...live, staleNotice: "Not live" })).not.toThrow();
  });
});

describe("pairing dialog live regions (F1)", () => {
  it("re-enables Pair and states the failure without touching the focused field", () => {
    const device = root.querySelector<HTMLInputElement>('#pair-form input[name="device"]')!;
    device.focus();
    device.setSelectionRange(2, 2);
    const form = root.querySelector("#pair-form");
    applyLiveRegions(root, { ...live, pairing: { status: "Creating this browser credential…", exchangeInFlight: true, createInFlight: false } });
    const submit = root.querySelector<HTMLButtonElement>('#pair-form button[type="submit"]')!;
    expect(submit.disabled).toBe(true);
    expect(submit.textContent).toBe("Pairing…");
    expect(root.querySelector("#pair-form")?.getAttribute("aria-busy")).toBe("true");

    // The exchange fails while Device label still has focus: the same nodes
    // carry the sentence and the usable button. No rebuild, no blur.
    applyLiveRegions(root, { ...live, pairing: { status: "This device could not reach the hub. Tap Pair again.", exchangeInFlight: false, createInFlight: false } });
    expect(root.querySelector("#pair-form")).toBe(form);
    expect(document.activeElement).toBe(device);
    expect(device.selectionStart).toBe(2);
    expect(submit.disabled).toBe(false);
    expect(submit.textContent).toBe("Pair");
    const status = root.querySelector<HTMLElement>("#pair-dialog .pair-status")!;
    expect(status.hidden).toBe(false);
    expect(status.textContent).toBe("This device could not reach the hub. Tap Pair again.");
    expect(root.querySelector("#pair-form")?.getAttribute("aria-busy")).toBe("false");
  });

  it("hides an empty status and flips Close to Cancel while a code is minted", () => {
    document.body.innerHTML = CREATE_STEP;
    root = document.body;
    applyLiveRegions(root, { ...live, pairing: { exchangeInFlight: false, createInFlight: true } });
    expect(root.querySelector<HTMLElement>("#pair-dialog .pair-status")!.hidden).toBe(true);
    expect(root.querySelector<HTMLButtonElement>("#pair-create")!.disabled).toBe(true);
    expect(root.querySelector("#pair-create")!.textContent).toBe("Creating…");
    expect(root.querySelector("#pair-close")!.textContent).toBe("Cancel");
    applyLiveRegions(root, { ...live, pairing: { status: "Waiting for a machine to claim the code…", exchangeInFlight: false, createInFlight: false } });
    expect(root.querySelector("#pair-create")!.textContent).toBe("Create pairing code");
    expect(root.querySelector("#pair-close")!.textContent).toBe("Close");
  });
});

describe("cleanup step live regions (F2)", () => {
  it("holds Retry cleanup busy while a retry runs and keeps Close as Close", () => {
    document.body.innerHTML = CLEANUP_STEP;
    root = document.body;
    applyLiveRegions(root, { ...live, pairing: { status: "Retrying cleanup…", exchangeInFlight: false, createInFlight: false, cleanupRetryInFlight: true } });
    const retry = root.querySelector<HTMLButtonElement>("#pair-cleanup-retry")!;
    expect(retry.disabled).toBe(true);
    expect(retry.textContent).toBe("Retrying…");
    expect(root.querySelector("#pair-close")!.textContent).toBe("Close");
    applyLiveRegions(root, { ...live, pairing: { status: "Browser storage could not be checked. Keep this page open and retry once storage access is restored.", exchangeInFlight: false, createInFlight: false, cleanupRetryInFlight: false } });
    expect(retry.disabled).toBe(false);
    expect(retry.textContent).toBe("Retry cleanup");
    expect(root.querySelector<HTMLElement>("#pair-dialog .pair-status")!.hidden).toBe(false);
  });
});

describe("pairing feedback announcements (cas-2e77 QA F02)", () => {
  it("returns a failure alert to a polite status before announcing an in-flight retry", () => {
    const feedback = root.querySelector<HTMLElement>("#pair-dialog .pair-status")!;
    feedback.setAttribute("role", "alert");
    feedback.hidden = false;
    feedback.textContent = "Pairing timed out after 10s. Pair again.";
    applyLiveRegions(root, { pairing: { status: "Updating this browser installation…", exchangeInFlight: true, createInFlight: false } });
    expect(feedback.getAttribute("role")).toBe("status");
    expect(feedback.textContent).toBe("Updating this browser installation…");
    expect(root.querySelector('#pair-form button[type="submit"]')?.textContent).toBe("Pairing…");
  });
});
