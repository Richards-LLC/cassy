// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from "vitest";
import { applyLiveRegions } from "./live-regions";

/**
 * The shapes render() emits. An invariant in invariants.test.ts pins these
 * selectors against the real template, so this fixture cannot drift into
 * testing markup the app does not ship.
 */
const SHELL = `
  <div class="shell">
    <header class="session-header">
      <span class="mode-badge observer" data-compact-label="OBS">OBSERVER</span>
      <span class="connection-summary connecting" title="connecting">
        <span class="connection-dot"></span>
        <span data-machine-latency="machine-1">Status unavailable</span>
      </span>
      <div class="actions">
        <span class="control-action" title="Take control">
          <button id="lease" data-compact-label="Ctrl" aria-label="Take control">Take control</button>
          <span id="control-disabled-reason" class="sr-only" hidden></span>
        </span>
        <button id="interrupt" class="danger">Interrupt</button>
      </div>
    </header>
    <section class="status-context">
      <p class="status-stale" role="status" hidden></p>
      <div id="status-view"></div>
      <div class="message">
        <textarea id="message-text"></textarea>
        <p class="control-disabled-reason" role="note" hidden></p>
        <div class="composer-actions"><button id="message-send" class="primary">Send message</button></div>
        <p id="message-status" class="message-status" role="status" hidden></p>
        <p id="message-delivery" class="message-delivery" role="status" hidden></p>
      </div>
    </section>
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

const live = {
  connection: { state: "live", title: "live", latencyText: "41ms" },
  mode: { badge: "CONTROL", compact: "CTL" },
  controlAction: { label: "Release control" },
} as const;

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
      applyLiveRegions(root, { ...live, connection: { state: "live", title: "live", latencyText: `${40 + beat}ms` } });
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
        connection: { state: beat % 3 === 0 ? "degraded" : "live", title: "hub", latencyText: `${30 + beat}ms` },
        mode: { badge: beat % 5 === 0 ? "OBSERVER" : "CONTROL", compact: "CTL" },
        controlAction: { label: "Release control" },
        ...(beat % 4 === 0 ? { staleNotice: "Not live — reconnecting." } : {}),
      });
    }
    observer.takeRecords();
    observer.disconnect();

    expect(replacements).toBe(0);
    expect(blurs).toBe(0);
    expect(document.activeElement).toBe(composer);
    expect(composer.value).toHaveLength(20);
    // The regions kept moving the whole time — this is not a frozen page.
    expect(root.querySelector("[data-machine-latency]")!.textContent).toBe("49ms");
  });
});

describe("live region values", () => {
  it("writes the connection state, title and latency in place", () => {
    applyLiveRegions(root, live);

    const summary = root.querySelector<HTMLElement>(".connection-summary")!;
    expect(summary.className).toBe("connection-summary live");
    expect(summary.title).toBe("live");
    expect(summary.querySelector("[data-machine-latency]")!.textContent).toBe("41ms");
    // The dot is a child of the summary and must survive the class rewrite.
    expect(summary.querySelector(".connection-dot")).not.toBeNull();
  });

  it("moves the mode badge between observer and control", () => {
    applyLiveRegions(root, live);

    const mode = root.querySelector<HTMLElement>(".mode-badge")!;
    expect(mode.className).toBe("mode-badge control");
    expect(mode.textContent).toBe("CONTROL");
    expect(mode.dataset.compactLabel).toBe("CTL");
  });

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

  it("relabels the control action and mirrors its reason onto the wrapper", () => {
    applyLiveRegions(root, { ...live, controlAction: { label: "Force takeover", disabledReason: "Daniel controls this session" } });

    const lease = root.querySelector<HTMLButtonElement>("#lease")!;
    expect(lease.textContent).toBe("Force takeover");
    expect(lease.getAttribute("aria-label")).toBe("Force takeover");
    expect(lease.getAttribute("aria-disabled")).toBe("true");
    expect(root.querySelector<HTMLElement>(".control-action")!.title).toBe("Daniel controls this session");
    expect(root.querySelector<HTMLElement>("#control-disabled-reason")!.hidden).toBe(false);
  });

  it("states why interrupt is unavailable and restores its plain title", () => {
    applyLiveRegions(root, { ...live, interruptReason: "Interrupt is unavailable for this session." });
    const interrupt = root.querySelector<HTMLButtonElement>("#interrupt")!;
    expect(interrupt.title).toBe("Interrupt is unavailable for this session.");

    applyLiveRegions(root, live);

    expect(interrupt.title).toBe("Interrupt selected pane");
    expect(interrupt.hasAttribute("aria-disabled")).toBe(false);
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
    document.body.innerHTML = '<div class="shell"></div>';

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
