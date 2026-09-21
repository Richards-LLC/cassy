// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_ASK_OPTIONS, askOptions, installAttentionObjects, renderAskObject, renderBlockerObject } from "./attention-objects";
import { ConversationHistory } from "./conversation-history";
import { ConversationView, type TurnRenderContext } from "./conversation-view";
import type { ThreadTurn } from "./thread-model";
import type { OperatorReply } from "./types";

const at = (hh: number, mm: number) => new Date(2026, 8, 21, hh, mm).getTime();
function reply(id: number, kind: "ask" | "blocker", message: string, extra: Partial<OperatorReply> = {}): OperatorReply {
  return { notification_id: id, reply_to: null, message, summary: "", device_id: "d", kind, ...extra };
}
function context(reply: OperatorReply, history: ConversationHistory, respond?: TurnRenderContext["respond"]): TurnRenderContext {
  const turn: ThreadTurn = { key: `reply:${reply.notification_id}`, side: "supervisor", kind: reply.kind ?? "answer", event: { kind: "reply", value: reply }, first: true, last: true };
  const ctx: TurnRenderContext = { document, turn, reply, supervisor: "atlas-sup", body: () => { const p = document.createElement("p"); p.textContent = reply.message; return [p]; }, history, respond };
  return ctx;
}

let uninstall: (() => void) | undefined;
afterEach(() => { uninstall?.(); uninstall = undefined; });

describe("ask object (Pebble 3, treatment A)", () => {
  it("is a fused object: body over a tray of quick-reply chips, defaults when the payload carries no options", () => {
    const history = new ConversationHistory();
    const ask = reply(50, "ask", "Fix in-train or ship?");
    const node = renderAskObject(ask, context(ask, history, () => {}));
    expect(node.className).toBe("obj t-a");
    expect(node.getAttribute("role")).toBe("group"); expect(node.getAttribute("aria-label")).toBe("Question from atlas-sup");
    expect(node.children[0]?.className).toBe("obj-body"); expect(node.querySelector(".obj-body p")?.textContent).toBe("Fix in-train or ship?");
    expect(node.children[1]?.className).toBe("obj-foot");
    const chips = [...node.querySelectorAll<HTMLButtonElement>(".obj-foot button.chip")];
    expect(chips.map((chip) => chip.textContent)).toEqual([...DEFAULT_ASK_OPTIONS]);
    expect(chips.every((chip) => chip.type === "button" && !chip.disabled)).toBe(true);
    expect(node.dataset.answered).toBe("false");
  });
  it("renders the payload's options as chips and sends the tapped one through respond", () => {
    const history = new ConversationHistory();
    const respond = vi.fn();
    const ask = reply(50, "ask", "Fix or ship?", { options: ["Fix in-train", " Ship with allowlist ", ""] });
    expect(askOptions(ask)).toEqual(["Fix in-train", "Ship with allowlist"]);
    const node = renderAskObject(ask, context(ask, history, respond));
    const chips = node.querySelectorAll<HTMLButtonElement>("button.chip");
    expect([...chips].map((chip) => chip.textContent)).toEqual(["Fix in-train", "Ship with allowlist"]);
    chips[1]!.click();
    expect(respond).toHaveBeenCalledWith(ask, "Ship with allowlist");
  });
  it("disables the chips when the view cannot send", () => {
    const ask = reply(50, "ask", "Fix or ship?");
    const node = renderAskObject(ask, context(ask, new ConversationHistory()));
    expect([...node.querySelectorAll<HTMLButtonElement>("button.chip")].every((chip) => chip.disabled)).toBe(true);
  });
  it("shows the chosen reply as sent once a send carries the ask's id", () => {
    const history = new ConversationHistory();
    const ask = reply(50, "ask", "Fix or ship?");
    history.reply(ask, at(9, 58));
    history.submit("q", "atlas-sup", "Yes, go ahead", at(10, 0), 50);
    const node = renderAskObject(ask, context(ask, history, () => {}));
    expect(node.dataset.answered).toBe("true");
    expect(node.querySelectorAll("button.chip")).toHaveLength(0);
    const sent = node.querySelector<HTMLElement>(".chip.sent")!;
    expect(sent.textContent).toBe("Yes, go ahead"); expect(sent.dataset.state).toBe("sending"); expect(sent.querySelector(".tick")).not.toBeNull();
    history.acknowledge({ client_ref: "q", notification_id: 60, target: "atlas-sup", stamped: true });
    expect(renderAskObject(ask, context(ask, history, () => {})).querySelector<HTMLElement>(".chip.sent")?.dataset.state).toBe("acknowledged");
  });
});

describe("blocker object", () => {
  it("carries the inset evidence window when the message ends in a file:line evidence line", () => {
    const blocker = reply(51, "blocker", "The release gate went red. The train is held.\nattention.rs:212 · needless_borrow");
    const node = renderBlockerObject(blocker, context(blocker, new ConversationHistory()));
    expect(node.className).toBe("obj t-a blk");
    expect(node.getAttribute("aria-label")).toBe("Blocker from atlas-sup");
    expect(node.querySelector(".obj-body p")?.textContent).toBe("The release gate went red. The train is held.");
    expect(node.querySelector(".obj-foot code.window")?.textContent).toBe("attention.rs:212 · needless_borrow");
  });
  it("has no tray without evidence", () => {
    const blocker = reply(51, "blocker", "The release gate went red.");
    const node = renderBlockerObject(blocker, context(blocker, new ConversationHistory()));
    expect(node.querySelector(".obj-body p")?.textContent).toBe("The release gate went red.");
    expect(node.querySelector(".obj-foot")).toBeNull();
  });
});

describe("installed on the Pebble 2 seam", () => {
  it("replaces the plain bubbles in the thread, pins the unanswered ask above the composer and unpins it on the quick reply", () => {
    uninstall = installAttentionObjects();
    const history = new ConversationHistory();
    const sent: Array<[number, string]> = [];
    const view = new ConversationView(document, history, {
      supervisor: "atlas-sup",
      respond: (ask, text) => { sent.push([ask.notification_id, text]); history.submit(`quick-${ask.notification_id}`, "atlas-sup", text, at(10, 0), ask.notification_id); view.update(); },
    });
    const composer = document.createElement("div"); composer.className = "conversation-composer";
    document.body.replaceChildren(view.element, view.pinned, composer);
    history.reply(reply(51, "blocker", "Gate red.\nattention.rs:212 · needless_borrow"), at(9, 58));
    history.reply(reply(52, "ask", "Fix or ship?"), at(9, 58));
    view.update();
    expect(view.element.querySelector('[data-kind="blocker"]')?.classList.contains("blk")).toBe(true);
    expect(view.element.querySelector('[data-kind="blocker"] .window')?.textContent).toBe("attention.rs:212 · needless_borrow");
    const inFlow = view.element.querySelector<HTMLElement>('[data-kind="ask"]')!;
    expect(inFlow.classList.contains("obj")).toBe(true); expect(inFlow.dataset.answered).toBe("false");
    expect(view.pinned.hidden).toBe(false);
    expect(view.pinned.querySelector(".pinned-label")?.textContent).toBe("Waiting on you");
    const pinned = view.pinned.querySelector<HTMLElement>('.obj[data-pinned="true"]')!;
    expect(pinned.dataset.notificationId).toBe("52");
    expect(history.waiting().map((item) => item.notification_id)).toEqual([51, 52]);
    // Tap a chip on the pinned copy: the send carries in_reply_to = 52, the
    // flow copy repaints as answered and the pin is released.
    pinned.querySelector<HTMLButtonElement>("button.chip")!.click();
    expect(sent).toEqual([[52, "Yes, go ahead"]]);
    expect(history.events.at(-1)).toMatchObject({ kind: "send", value: { text: "Yes, go ahead", replyTo: 52 } });
    expect(view.pinned.hidden).toBe(true); expect(view.pinned.children).toHaveLength(0);
    const answered = view.element.querySelector<HTMLElement>('[data-kind="ask"]')!;
    expect(answered.dataset.answered).toBe("true");
    expect(answered.querySelector(".chip.sent")?.textContent).toBe("Yes, go ahead");
    // The operator's send after the blocker acknowledges it too.
    expect(history.waiting()).toEqual([]);
  });
  it("unregisters cleanly so the fallback bubbles return", () => {
    installAttentionObjects()();
    const history = new ConversationHistory(); const view = new ConversationView(document, history, "sup");
    history.reply(reply(1, "ask", "Fix?"), at(9, 0)); view.update();
    expect(view.element.querySelector('[data-kind="ask"]')?.classList.contains("bub")).toBe(true);
    expect(view.pinned.hidden).toBe(true);
  });
});
