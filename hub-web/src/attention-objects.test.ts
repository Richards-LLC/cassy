// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_ASK_OPTIONS, WAITING_LINE, askOptions, installAttentionObjects, renderAskObject, renderBlockerObject } from "./attention-objects";
import { ConversationHistory } from "./conversation-history";
import { ConversationView, type TurnRenderContext } from "./conversation-view";
import type { ThreadTurn } from "./thread-model";
import type { OperatorReply } from "./types";

const at = (hh: number, mm: number) => new Date(2026, 8, 21, hh, mm).getTime();
function reply(id: number, kind: "ask" | "blocker", message: string, extra: Partial<OperatorReply> = {}): OperatorReply {
  return { notification_id: id, reply_to: null, message, summary: "", device_id: "d", kind, ...extra };
}
function context(reply: OperatorReply, history: ConversationHistory, respond?: TurnRenderContext["respond"], pinned?: boolean): TurnRenderContext {
  const turn: ThreadTurn = { key: `reply:${reply.notification_id}`, side: "supervisor", kind: reply.kind ?? "answer", event: { kind: "reply", value: reply }, first: true, last: true };
  const ctx: TurnRenderContext = { document, turn, reply, supervisor: "atlas-sup", body: () => { const p = document.createElement("p"); p.textContent = reply.message; return [p]; }, history, respond, pinned };
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

describe("ask shown once (P1, cas-b1ee)", () => {
  it("keeps the flow copy of a pinned ask to a one-line reference (F18)", () => {
    const history = new ConversationHistory();
    const long = "Gate run 33512 failed on that one warning. Fix it in-train — one worker, about ten minutes — or ship 3.26.0 with it allowlisted?\nDetails follow.";
    const ask = reply(70, "ask", long);
    history.reply(ask, at(9, 58));
    const flow = renderAskObject(ask, context(ask, history, () => {}));
    expect([...flow.querySelectorAll(".obj-body > p")].map((p) => p.className)).toEqual(["ask-excerpt", "ask-waiting"]);
    expect(flow.querySelector(".ask-excerpt")?.textContent).toBe("Gate run 33512 failed on that one warning. Fix it in-train — one worker, about ten…");
    // The pinned card still carries the whole question.
    expect(renderAskObject(ask, context(ask, history, () => {}, true)).textContent).toContain("or ship 3.26.0 with it allowlisted?");
  });

  it("collapses the pinned ask's flow copy to a waiting pebble with no chips, while the pinned copy keeps them", () => {
    const history = new ConversationHistory();
    const ask = reply(50, "ask", "Fix or ship?", { options: ["Fix in-train", "Ship with allowlist"] });
    history.reply(ask, at(9, 58));
    const flow = renderAskObject(ask, context(ask, history, () => {}));
    expect(flow.className).toBe("obj t-a ask-collapsed");
    expect(flow.dataset.collapsed).toBe("true"); expect(flow.dataset.answered).toBe("false");
    expect(flow.getAttribute("aria-label")).toBe("Question from atlas-sup");
    expect(flow.querySelector(".obj-body p")?.textContent).toBe("Fix or ship?");
    expect(flow.querySelector(".obj-body .ask-waiting")?.textContent).toBe(WAITING_LINE);
    expect(WAITING_LINE).toBe("Waiting on you — answer below");
    expect(flow.querySelector(".obj-foot")).toBeNull(); expect(flow.querySelectorAll(".chip")).toHaveLength(0);
    const pinned = renderAskObject(ask, context(ask, history, () => {}, true));
    expect(pinned.className).toBe("obj t-a"); expect(pinned.dataset.collapsed).toBeUndefined();
    expect(pinned.querySelector(".ask-waiting")).toBeNull();
    expect([...pinned.querySelectorAll("button.chip")].map((chip) => chip.textContent)).toEqual(["Fix in-train", "Ship with allowlist"]);
  });
  it("keeps the chips on an unpinned ask: an older unanswered ask, or one rendered without a history", () => {
    const history = new ConversationHistory();
    const older = reply(50, "ask", "Older?"); const newer = reply(52, "ask", "Newer?");
    history.reply(older, at(9, 50)); history.reply(newer, at(9, 58));
    const node = renderAskObject(older, context(older, history, () => {}));
    expect(node.dataset.collapsed).toBeUndefined(); expect(node.querySelector(".ask-waiting")).toBeNull();
    expect(node.querySelectorAll("button.chip")).toHaveLength(2);
    expect(renderAskObject(newer, context(newer, history, () => {})).dataset.collapsed).toBe("true");
    const bare = { ...context(older, history, () => {}), history: undefined };
    expect(renderAskObject(older, bare).querySelectorAll("button.chip")).toHaveLength(2);
  });
  it("shows the sent-answer chip, not the waiting line, once the ask is answered", () => {
    const history = new ConversationHistory();
    const ask = reply(50, "ask", "Fix or ship?");
    history.reply(ask, at(9, 58)); history.submit("q", "atlas-sup", "Hold", at(10, 0), 50);
    const node = renderAskObject(ask, context(ask, history, () => {}));
    expect(node.className).toBe("obj t-a"); expect(node.dataset.answered).toBe("true");
    expect(node.querySelector(".ask-waiting")).toBeNull();
    expect(node.querySelector(".chip.sent")?.textContent).toBe("Hold");
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
    // Shown once: the flow copy is the collapsed pebble pointing at the tray;
    // the only chips on screen are the pinned ones.
    expect(inFlow.dataset.collapsed).toBe("true");
    expect(inFlow.querySelector(".ask-waiting")?.textContent).toBe("Waiting on you — answer below");
    expect(view.element.querySelectorAll(".chip")).toHaveLength(0);
    expect(document.querySelectorAll("button.chip")).toHaveLength(2);
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
    expect(answered.dataset.answered).toBe("true"); expect(answered.dataset.collapsed).toBeUndefined();
    expect(answered.querySelector(".ask-waiting")).toBeNull();
    expect(answered.querySelector(".chip.sent")?.textContent).toBe("Yes, go ahead");
    // The operator's send after the blocker acknowledges it too.
    expect(history.waiting()).toEqual([]);
  });
  it("keeps a refused chip reply's ask pinned with live chips beside Not sent, and unpins it on a successful retry (cas-b438)", () => {
    uninstall = installAttentionObjects();
    const history = new ConversationHistory();
    const sent: string[] = [];
    const view = new ConversationView(document, history, {
      supervisor: "atlas-sup",
      respond: (ask, text) => { sent.push(text); history.submit(`quick-${sent.length}`, "atlas-sup", text, at(10, sent.length), ask.notification_id); view.update(); },
      retryMessage: (send) => { history.discardRefused(send.id); history.submit(`retry-${send.id}`, "atlas-sup", send.text, at(10, 30), send.replyTo); view.update(); },
    });
    document.body.replaceChildren(view.element, view.pinned);
    history.reply(reply(52, "ask", "Fix or ship?"), at(9, 58)); view.update();
    const flowAsk = () => view.element.querySelector<HTMLElement>('[data-kind="ask"]')!;
    const pinnedChips = () => [...view.pinned.querySelectorAll<HTMLButtonElement>("button.chip")];
    // Tap a pinned chip: sending answers optimistically, so the pin is released.
    pinnedChips()[0]!.click();
    expect(sent).toEqual(["Yes, go ahead"]);
    expect(view.pinned.hidden).toBe(true); expect(flowAsk().dataset.answered).toBe("true");
    // The hub refuses it: the ask is pinned again with usable chips, the flow copy collapses back, and the refused bubble offers Retry.
    history.reject("quick-1", "no access"); view.update();
    expect(view.pinned.hidden).toBe(false);
    expect(view.pinned.querySelector<HTMLElement>(".obj")?.dataset.notificationId).toBe("52");
    expect(pinnedChips()).toHaveLength(2); expect(pinnedChips().every((chip) => !chip.disabled)).toBe(true);
    expect(flowAsk().dataset.answered).toBe("false"); expect(flowAsk().dataset.collapsed).toBe("true");
    expect(flowAsk().querySelector(".chip.sent")).toBeNull();
    const refused = view.element.querySelector<HTMLElement>('.bub[data-state="error"]')!;
    expect(refused.querySelector(".conversation-refused b")?.textContent).toBe("Not sent");
    // Answering again from the still-pinned tray works.
    pinnedChips()[1]!.click();
    expect(sent).toEqual(["Yes, go ahead", "Hold"]); expect(view.pinned.hidden).toBe(true);
    history.reject("quick-2", "no access"); view.update();
    expect(view.pinned.hidden).toBe(false);
    // Retry of a refused reply: the new send answers the ask and the pin is released.
    view.element.querySelector<HTMLButtonElement>('.bub[data-state="error"] .conversation-retry')!.click();
    expect(view.pinned.hidden).toBe(true); expect(view.pinned.children).toHaveLength(0);
    expect(flowAsk().dataset.answered).toBe("true"); expect(flowAsk().querySelector(".chip.sent")?.textContent).toBe("Yes, go ahead");
    expect(history.answered(52)?.id).toBe("retry-quick-1");
  });
  it("expands the older ask's flow copy again when a newer ask takes the pin", () => {
    uninstall = installAttentionObjects();
    const history = new ConversationHistory();
    const view = new ConversationView(document, history, { supervisor: "atlas-sup", respond: () => {} });
    document.body.replaceChildren(view.element, view.pinned);
    history.reply(reply(50, "ask", "First?"), at(9, 50)); view.update();
    const first = () => view.element.querySelector<HTMLElement>('[data-kind="ask"][data-notification-id="50"]')!;
    expect(first().dataset.collapsed).toBe("true");
    history.reply(reply(52, "ask", "Second?"), at(9, 58)); view.update();
    expect(first().dataset.collapsed).toBeUndefined(); expect(first().querySelectorAll("button.chip")).toHaveLength(2);
    expect(view.element.querySelector<HTMLElement>('[data-notification-id="52"]')?.dataset.collapsed).toBe("true");
    expect(view.pinned.querySelector<HTMLElement>(".obj")?.dataset.notificationId).toBe("52");
  });
  it("unregisters cleanly so the fallback bubbles return", () => {
    installAttentionObjects()();
    const history = new ConversationHistory(); const view = new ConversationView(document, history, "sup");
    history.reply(reply(1, "ask", "Fix?"), at(9, 0)); view.update();
    expect(view.element.querySelector('[data-kind="ask"]')?.classList.contains("bub")).toBe(true);
    expect(view.pinned.hidden).toBe(true);
  });
});
