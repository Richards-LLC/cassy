// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { askOptions, installAttentionObjects, renderAskObject, renderBlockerObject } from "./attention-objects";
import { ConversationHistory, RECEIPT_TIMEOUT_MS } from "./conversation-history";
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
  it("is a fused object: body with no choice tray when the payload carries no options", () => {
    const history = new ConversationHistory();
    const ask = reply(50, "ask", "Fix in-train or ship?");
    const node = renderAskObject(ask, context(ask, history, () => {}));
    expect(node.className).toBe("obj t-a");
    expect(node.getAttribute("role")).toBe("group"); expect(node.getAttribute("aria-label")).toBe("Question from atlas-sup");
    expect(node.children[0]?.className).toBe("obj-body"); expect(node.querySelector(".obj-body p")?.textContent).toBe("Fix in-train or ship?");
    expect(node.querySelector(".obj-foot")).toBeNull();
    const chips = [...node.querySelectorAll<HTMLButtonElement>(".obj-foot button.chip")];
    expect(chips).toHaveLength(0);
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
    const ask = reply(50, "ask", "Fix or ship?", { options: ["Fix", "Ship"] });
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

describe("one complete question in the thread", () => {
  it("keeps the latest question's entire formatted body and declared choices in the flow", () => {
    const history = new ConversationHistory();
    const ask = reply(50, "ask", "Fix or ship?\nDetails follow.", { options: ["Fix", "Ship"] });
    history.reply(ask, at(9, 58));
    const node = renderAskObject(ask, context(ask, history, () => {}));
    expect(node.dataset.collapsed).toBeUndefined();
    expect(node.querySelector(".obj-body")?.textContent).toBe(ask.message);
    expect([...node.querySelectorAll("button.chip")].map(chip => chip.textContent)).toEqual(["Fix", "Ship"]);
  });
  it("keeps the chosen reply once the question is answered", () => {
    const history = new ConversationHistory(); const ask = reply(50, "ask", "Fix or ship?");
    history.reply(ask, at(9, 58)); history.submit("q", "atlas-sup", "Hold", at(10, 0), 50);
    const node = renderAskObject(ask, context(ask, history, () => {}));
    expect(node.dataset.answered).toBe("true"); expect(node.querySelector(".chip.sent")?.textContent).toBe("Hold");
    expect(node.querySelectorAll("button.chip")).toHaveLength(0);
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
  it("says how to clear it while it waits, and drops the hint once a send acknowledges it (journey F12)", () => {
    const history = new ConversationHistory();
    const blocker = reply(51, "blocker", "The release gate went red.\nattention.rs:212 · needless_borrow");
    history.reply(blocker, at(9, 58));
    const waiting = renderBlockerObject(blocker, context(blocker, history));
    expect(waiting.dataset.waiting).toBe("true");
    expect(waiting.querySelector(".obj-body .blk-hint")?.textContent).toBe("Reply to unblock");
    // The hint sits in the body, above the evidence window.
    expect(waiting.querySelector(".obj-body")?.lastElementChild?.className).toBe("blk-hint");
    history.submit("ack", "atlas-sup", "Looking now.", at(10, 0));
    const acknowledged = renderBlockerObject(blocker, context(blocker, history));
    expect(acknowledged.dataset.waiting).toBe("false");
    expect(acknowledged.querySelector(".blk-hint")).toBeNull();
    // cas-e829: a later send that is not bound to this blocker stops it
    // waiting and quiets it, but never claims a reply.
    expect(acknowledged.dataset.acknowledged).toBe("true");
    expect(acknowledged.querySelector(".blk-handled")?.textContent).toBe("You've written since this");
    expect(acknowledged.querySelector(".blk-handled svg.tick")).toBeNull();
    expect(acknowledged.getAttribute("aria-label")).toBe("Blocker from atlas-sup, you've written since");
    expect(acknowledged.textContent).not.toContain("you replied");
    // cas-71af (aac8 QA F01): a reply bound to it (in_reply_to) is handled,
    // says so with a tick and quiets like an answered ask.
    history.submit("bound", "atlas-sup", "Rolling it back.", at(10, 1), 51);
    const replied = renderBlockerObject(blocker, context(blocker, history));
    expect(replied.dataset.acknowledged).toBe("true");
    expect(replied.querySelector(".blk-handled")?.textContent).toBe("Acknowledged — you replied");
    expect(replied.querySelector(".blk-handled svg.tick")).not.toBeNull();
    expect(replied.getAttribute("aria-label")).toBe("Blocker from atlas-sup, acknowledged");
    history.reject("bound", "no access");
    // A refused send never reached the supervisor: the blocker still waits and still says how.
    history.reject("ack", "no access");
    expect(renderBlockerObject(blocker, context(blocker, history)).querySelector(".blk-hint")?.textContent).toBe("Reply to unblock");
  });
  it("never says you replied to a blocker nobody answered, even once it stops waiting (cas-e829)", () => {
    // The operator's 2026-10-01 screenshot: watchdog blockers from an ended
    // session, with no reply bound to any of them, each said "you replied".
    const history = new ConversationHistory();
    const blocker = reply(3196301, "blocker", "The supervisor was told 9 minutes ago that a worker died.");
    history.reply(blocker, at(9, 58), "Accounting-wise-lion-31");
    history.currentSession = "Accounting-rapid-gazelle-52";
    const node = renderBlockerObject(blocker, context(blocker, history));
    expect(node.dataset.waiting).toBe("false");
    expect(node.dataset.retired).toBe("session-ended");
    expect(node.querySelector(".blk-handled")).toBeNull();
    expect(node.textContent).not.toContain("you replied");
  });
  it("keeps waiting beside a send whose receipt never came (cas-71af, aac8 QA F02)", () => {
    const history = new ConversationHistory();
    const blocker = reply(51, "blocker", "The release gate went red.");
    history.reply(blocker, at(9, 58));
    history.submit("ack", "atlas-sup", "Rotating the key.", at(10, 0));
    history.unconfirmSilent(at(10, 0) + RECEIPT_TIMEOUT_MS);
    const node = renderBlockerObject(blocker, context(blocker, history));
    expect(node.dataset.waiting).toBe("true");
    expect(node.querySelector(".blk-hint")?.textContent).toBe("Reply to unblock");
    expect(node.dataset.acknowledged).toBeUndefined();
    // The receipt lands late: now it is acknowledged.
    expect(history.acknowledge({ client_ref: "ack", notification_id: 60, target: "atlas-sup", stamped: true })).toBe(true);
    expect(renderBlockerObject(blocker, context(blocker, history)).dataset.acknowledged).toBe("true");
  });
  it("quiets an acknowledged blocker in the stylesheet (cas-71af)", async () => {
    const [{ readFileSync }, { dirname, join }, { fileURLToPath }] = await Promise.all([import("node:fs"), import("node:path"), import("node:url")]);
    const css = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "styles.css"), "utf8");
    expect(css).toContain('.obj.t-a.blk[data-acknowledged="true"] .obj-body, .obj.t-a.blk[data-acknowledged="true"] .obj-foot { border-left-color: var(--color-transparent); background: var(--sup-bg); color: var(--sup-fg); }');
  });
  it("quiets an answered question in the stylesheet: supervisor colour, the tick kept (journey F12)", async () => {
    const [{ readFileSync }, { dirname, join }, { fileURLToPath }] = await Promise.all([import("node:fs"), import("node:path"), import("node:url")]);
    const css = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "styles.css"), "utf8");
    expect(css).toContain('.obj.t-a[data-answered="true"] .obj-body, .obj.t-a[data-answered="true"] .obj-foot { border-left-color: var(--color-transparent); background: var(--sup-bg); color: var(--sup-fg); }');
    expect(css).toContain('.obj.t-a[data-answered="true"] { box-shadow: var(--lift-sup); }');
  });
  it("has no tray without evidence", () => {
    const blocker = reply(51, "blocker", "The release gate went red.");
    const node = renderBlockerObject(blocker, context(blocker, new ConversationHistory()));
    expect(node.querySelector(".obj-body p")?.textContent).toBe("The release gate went red.");
    expect(node.querySelector(".obj-foot")).toBeNull();
  });
});

describe("installed on the Pebble 2 seam", () => {
  it("renders the full ask once and a jump bookmark; quick reply releases the bookmark", () => {
    uninstall = installAttentionObjects(); const history = new ConversationHistory();
    const sent: Array<[number, string]> = [];
    const view = new ConversationView(document, history, { supervisor: "atlas-sup", respond: (ask, text) => {
      sent.push([ask.notification_id, text]); history.submit("quick", "atlas-sup", text, at(10, 0), ask.notification_id); view.update();
    } });
    document.body.replaceChildren(view.element, view.pinned);
    history.reply(reply(52, "ask", "Fix or ship?", { options: ["Fix", "Ship"] }), at(9, 58)); view.update();
    const inFlow = view.element.querySelector<HTMLElement>('[data-kind="ask"]')!;
    expect(inFlow.dataset.collapsed).toBeUndefined();
    expect(document.querySelectorAll('.obj[data-kind="ask"]')).toHaveLength(1);
    expect(view.pinned.querySelector(".obj")).toBeNull();
    expect(view.pinned.querySelector(".pinned-bar-label")?.textContent).toBe("Waiting on you:");
    inFlow.querySelector<HTMLButtonElement>("button.chip")!.click();
    expect(sent).toEqual([[52, "Fix"]]); expect(history.answered(52)?.replyTo).toBe(52);
    expect(view.pinned.hidden).toBe(true); expect(view.pinned.children).toHaveLength(0);
    expect(view.element.querySelector('.chip.sent')?.textContent).toBe("Fix");
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
    history.reply(reply(52, "ask", "Fix or ship?", { options: ["Yes, go ahead", "Hold"] }), at(9, 58)); view.update();
    const flowAsk = () => view.element.querySelector<HTMLElement>('[data-kind="ask"]')!;
    const flowChips = () => [...view.element.querySelectorAll<HTMLButtonElement>("button.chip")];
    // Tap a thread choice: sending answers optimistically, so the pin is released.
    flowChips()[0]!.click();
    expect(sent).toEqual(["Yes, go ahead"]);
    expect(view.pinned.hidden).toBe(true); expect(flowAsk().dataset.answered).toBe("true");
    // The hub refuses it: the ask is pinned again with usable chips, the complete question stays in the flow, and the refused bubble offers Retry.
    history.reject("quick-1", "no access"); view.update();
    expect(view.pinned.hidden).toBe(false);
    expect(view.pinned.querySelector(".obj")).toBeNull();
    expect(view.pinned.textContent).toContain("Fix or ship?");
    expect(flowChips()).toHaveLength(2); expect(flowChips().every((chip) => !chip.disabled)).toBe(true);
    expect(flowAsk().dataset.answered).toBe("false"); expect(flowAsk().dataset.collapsed).toBeUndefined();
    expect(flowAsk().querySelector(".chip.sent")).toBeNull();
    const refused = view.element.querySelector<HTMLElement>('.bub[data-state="error"]')!;
    expect(refused.querySelector(".conversation-refused b")?.textContent).toBe("Not sent");
    // Answering again from the thread question works.
    flowChips()[1]!.click();
    expect(sent).toEqual(["Yes, go ahead", "Hold"]); expect(view.pinned.hidden).toBe(true);
    history.reject("quick-2", "no access"); view.update();
    expect(view.pinned.hidden).toBe(false);
    // Retry of a refused reply: the new send answers the ask and the pin is released.
    view.element.querySelector<HTMLButtonElement>('.bub[data-state="error"] .conversation-retry')!.click();
    expect(view.pinned.hidden).toBe(true); expect(view.pinned.children).toHaveLength(0);
    expect(flowAsk().dataset.answered).toBe("true"); expect(flowAsk().querySelector(".chip.sent")?.textContent).toBe("Yes, go ahead");
    expect(history.answered(52)?.id).toBe("retry-quick-1");
  });
  it("moves only the bookmark when a newer question arrives, keeping both complete", () => {
    uninstall = installAttentionObjects(); const history = new ConversationHistory();
    const view = new ConversationView(document, history, { supervisor: "atlas-sup", respond: () => {} });
    document.body.replaceChildren(view.element, view.pinned);
    history.reply(reply(50, "ask", "First?", { options: ["First choice"] }), at(9, 50)); view.update();
    history.reply(reply(52, "ask", "Second?", { options: ["Second choice"] }), at(9, 58)); view.update();
    expect(view.element.querySelectorAll('button.chip')).toHaveLength(2);
    expect(view.element.querySelectorAll('.ask-collapsed')).toHaveLength(0);
    expect(view.pinned.textContent).toContain("Second?"); expect(view.pinned.textContent).not.toContain("First?");
  });
  it("unregisters cleanly so the fallback bubbles return", () => {
    installAttentionObjects()();
    const history = new ConversationHistory(); const view = new ConversationView(document, history, "sup");
    history.reply(reply(1, "ask", "Fix?"), at(9, 0)); view.update();
    expect(view.element.querySelector('[data-kind="ask"]')?.classList.contains("bub")).toBe(true);
    expect(view.pinned.hidden).toBe(true);
  });
});
