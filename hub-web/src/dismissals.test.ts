// @vitest-environment jsdom
// cas-16eed: failed sends and stale questions can be dismissed and never crowd
// the conversation.
import { afterEach, describe, expect, it, vi } from "vitest";
import { installAttentionObjects, RETIRED_LINES } from "./attention-objects";
import { ConversationHistory } from "./conversation-history";
import { askLine, ConversationView } from "./conversation-view";
import { DISMISSED_ASKS_PER_THREAD, DISMISSED_ASKS_STORAGE_KEY, DISMISSED_ASKS_THREADS, loadDismissedAsks, saveDismissedAsks } from "./dismissed-asks";
import { bindSwipeDismiss, swipeThreshold, SWIPE_MIN_PX } from "./swipe-dismiss";
import type { OperatorReply, OperatorTurnKind } from "./types";

const at = (hh: number, mm: number) => new Date(2026, 8, 27, hh, mm).getTime();
function reply(id: number, kind: OperatorTurnKind, message = `m${id}`, reply_to: number | null = null): OperatorReply {
  return { notification_id: id, reply_to, message, summary: "", device_id: "d", kind };
}
function refused(history: ConversationHistory, id: string, text: string, when = at(9, 0)): void {
  history.submit(id, "sup", text, when);
  history.reject(id, "forbidden");
}

let uninstall: (() => void) | undefined;
afterEach(() => { uninstall?.(); uninstall = undefined; vi.useRealTimers(); });

describe("failed sends in the history", () => {
  it("dismisses a refused or unconfirmed send out of the visible thread and restores it", () => {
    const history = new ConversationHistory();
    refused(history, "x", "Do the burn down");
    history.submit("y", "sup", "Delivered one", at(9, 1));
    expect(history.dismissSend("y")).toBe(false); // a send still in flight is not a failed send
    expect(history.dismissSend("x")).toBe(true);
    expect(history.dismissSend("x")).toBe(false);
    expect(history.visibleEvents().map((event) => event.kind === "send" && event.value.id)).toEqual(["y"]);
    expect(history.dismissedSends().map((send) => send.id)).toEqual(["x"]);
    expect(history.restoreDismissed().map((send) => send.id)).toEqual(["x"]);
    expect(history.dismissedSends()).toEqual([]);
    expect(history.visibleEvents()).toHaveLength(2);
  });
  it("never dismisses a replaced record or a settled Not confirmed", () => {
    const history = new ConversationHistory();
    refused(history, "x", "Ship it");
    history.retireRefused("x");
    expect(history.dismissSend("x")).toBe(false);
    history.submit("u", "sup", "Is it green?", at(9, 2));
    history.unconfirmSilent(at(9, 2) + 20_000);
    history.receive(reply(9, "answer", "Yes."), at(9, 3) + 30_000);
    expect(history.dismissSend("u")).toBe(false);
  });
  it("leaves the list preview once dismissed, and comes back delivered when a late receipt lands", () => {
    const history = new ConversationHistory();
    history.reply(reply(1, "answer", "Morning."), at(8, 0));
    history.submit("u", "sup", "Is the gate green?", at(9, 0));
    history.unconfirmSilent(at(9, 0) + 20_000);
    expect(history.preview()).toBe("You: Is the gate green?");
    history.dismissSend("u");
    expect(history.preview()).toBe("Morning.");
    history.acknowledge({ client_ref: "u", notification_id: 70, target: "sup", stamped: true });
    expect(history.dismissedSends()).toEqual([]);
    expect(history.visibleEvents().some((event) => event.kind === "send" && event.value.id === "u")).toBe(true);
    refused(history, "r", "Do the burn down", at(9, 5));
    expect(history.preview()).toBe("Not sent: Do the burn down");
    history.dismissSend("r");
    expect(history.preview()).toBe("You: Is the gate green?");
  });
});

describe("questions that stop waiting", () => {
  it("retires a question from a session that has ended", () => {
    const history = new ConversationHistory();
    history.reply(reply(10, "ask", "Open the PR to main and cut a release?"), at(9, 0), "old-session");
    history.reply(reply(11, "blocker", "Gate red."), at(9, 1), "old-session");
    expect(history.pinnedAsk()?.notification_id).toBe(10);
    history.currentSession = "new-session";
    expect(history.retirement(10)).toBe("session-ended");
    expect(history.retirement(11)).toBe("session-ended");
    expect(history.pinnedAsk()).toBeUndefined();
    expect(history.waiting()).toEqual([]);
  });
  it("retires a question once a later turn comes from another session, even before the thread knows its own", () => {
    const history = new ConversationHistory();
    history.reply(reply(10, "ask", "Ship?"), at(9, 0), "old-session");
    expect(history.retirement(10)).toBeUndefined();
    history.reply(reply(12, "status", "Starting up."), at(9, 5), "new-session");
    expect(history.retirement(10)).toBe("session-ended");
  });
  it("keeps a question from the current session, and one with no session stamp", () => {
    const history = new ConversationHistory();
    history.currentSession = "live";
    history.reply(reply(10, "ask", "Ship?"), at(9, 0), "live");
    history.reply(reply(11, "ask", "Old stamp-less ask?"), at(9, 1));
    expect(history.waiting().map((item) => item.notification_id)).toEqual([10, 11]);
  });
  it("retires an ask the supervisor moved past with an unprompted answer, but not for status or a reply to a later message", () => {
    const history = new ConversationHistory();
    history.reply(reply(10, "ask", "Ship?"), at(9, 0));
    history.reply(reply(11, "status", "Gate 2 of 3."), at(9, 1));
    expect(history.retirement(10)).toBeUndefined();
    history.submit("q", "sup", "How long?", at(9, 2));
    history.acknowledge({ client_ref: "q", notification_id: 40, target: "sup", stamped: true });
    history.reply(reply(12, "answer", "Ten minutes.", 40), at(9, 3));
    expect(history.retirement(10)).toBeUndefined();
    expect(history.pinnedAsk()?.notification_id).toBe(10);
    history.reply(reply(13, "receipt", "Released 3.32.0."), at(9, 4));
    expect(history.retirement(10)).toBe("moved-on");
    expect(history.pinnedAsk()).toBeUndefined();
  });
  it("does not call an answered question retired", () => {
    const history = new ConversationHistory();
    history.currentSession = "new";
    history.reply(reply(10, "ask", "Ship?"), at(9, 0), "old");
    history.submit("a", "sup", "Yes", at(9, 1), 10, "old");
    expect(history.retirement(10)).toBeUndefined();
    expect(history.answered(10)?.text).toBe("Yes");
  });
  it("unpins a dismissed question and stops it waiting", () => {
    const history = new ConversationHistory();
    history.reply(reply(10, "ask", "First?"), at(9, 0));
    history.reply(reply(11, "ask", "Second?"), at(9, 1));
    expect(history.dismissAsk(11)).toBe(true);
    expect(history.dismissAsk(11)).toBe(false);
    expect(history.retirement(11)).toBe("dismissed");
    expect(history.pinnedAsk()?.notification_id).toBe(10);
    expect(history.dismissedAskIds()).toEqual([11]);
  });
});

describe("dismissed questions across a reload", () => {
  function memory(): Storage {
    const items = new Map<string, string>();
    return { getItem: (key) => items.get(key) ?? null, setItem: (key, value) => { items.set(key, value); }, removeItem: (key) => { items.delete(key); }, clear: () => items.clear(), key: () => null, length: 0 } as Storage;
  }
  it("keeps ids per thread, capped, dropping the least recently written threads", () => {
    const storage = memory();
    saveDismissedAsks(storage, "atlas:pelican", [10, 11, 11]);
    expect(loadDismissedAsks(storage, "atlas:pelican")).toEqual([10, 11]);
    expect(loadDismissedAsks(storage, "studio:otter")).toEqual([]);
    saveDismissedAsks(storage, "atlas:pelican", Array.from({ length: DISMISSED_ASKS_PER_THREAD + 5 }, (_, index) => index));
    expect(loadDismissedAsks(storage, "atlas:pelican")).toHaveLength(DISMISSED_ASKS_PER_THREAD);
    for (let index = 0; index < DISMISSED_ASKS_THREADS; index += 1) saveDismissedAsks(storage, `m:${index}`, [index]);
    expect(loadDismissedAsks(storage, "atlas:pelican")).toEqual([]);
    expect(loadDismissedAsks(storage, `m:${DISMISSED_ASKS_THREADS - 1}`)).toEqual([DISMISSED_ASKS_THREADS - 1]);
    saveDismissedAsks(storage, "m:0", []);
    expect(loadDismissedAsks(storage, "m:0")).toEqual([]);
  });
  it("reads a corrupt or missing store as empty", () => {
    const storage = memory();
    storage.setItem(DISMISSED_ASKS_STORAGE_KEY, "{not json");
    expect(loadDismissedAsks(storage, "t")).toEqual([]);
    storage.setItem(DISMISSED_ASKS_STORAGE_KEY, JSON.stringify({ t: [1, "2", 3.5, 4] }));
    expect(loadDismissedAsks(storage, "t")).toEqual([1, 4]);
    expect(loadDismissedAsks(undefined, "t")).toEqual([]);
  });
});

describe("the collapsed question bar's line", () => {
  it("names the question, not the preamble", () => {
    expect(askLine("All skills fix waves (A–D) are done; the full test suite passes.\n\n- Epic at 51a09: pass.\n- **Ask:** open the PR to main and cut a release?")).toBe("open the PR to main and cut a release?");
    expect(askLine("Fix it in the train, or ship with it allowlisted?")).toBe("Fix it in the train, or ship with it allowlisted?");
    expect(askLine("**Gate red.** Decide how to proceed.")).toBe("Gate red. Decide how to proceed.");
    expect(askLine("keep snake_case names?")).toBe("keep snake_case names?");
  });
});

/** jsdom has no PointerEvent: a MouseEvent carrying the pointer fields the gesture reads. */
function pointer(type: string, x: number, y = 0, pointerType = "touch"): Event {
  const event = new MouseEvent(type, { bubbles: true, cancelable: true, clientX: x, clientY: y });
  Object.defineProperties(event, { pointerType: { value: pointerType }, pointerId: { value: 1 }, isPrimary: { value: true } });
  return event;
}
function swipe(element: HTMLElement, dx: number, dy = 0, pointerType = "touch"): void {
  element.dispatchEvent(pointer("pointerdown", 100, 100, pointerType));
  element.dispatchEvent(pointer("pointermove", 100 + dx / 2, 100 + dy / 2, pointerType));
  element.dispatchEvent(pointer("pointermove", 100 + dx, 100 + dy, pointerType));
  element.dispatchEvent(pointer("pointerup", 100 + dx, 100 + dy, pointerType));
}

describe("swipe to dismiss", () => {
  function card(width = 300): HTMLElement {
    const element = document.createElement("div");
    element.getBoundingClientRect = () => ({ width, height: 80, top: 0, left: 0, right: width, bottom: 80, x: 0, y: 0, toJSON: () => ({}) });
    document.body.replaceChildren(element);
    return element;
  }
  it("dismisses past a third of the card, at least 72px; settles back short of it", () => {
    expect(swipeThreshold(390)).toBe(130);
    expect(swipeThreshold(120)).toBe(SWIPE_MIN_PX);
    vi.useFakeTimers();
    const element = card();
    const onDismiss = vi.fn();
    bindSwipeDismiss(element, { onDismiss, reducedMotion: () => false });
    swipe(element, -80);
    vi.advanceTimersByTime(500);
    expect(onDismiss).not.toHaveBeenCalled();
    expect(element.style.transform).toBe("");
    swipe(element, -140);
    expect(element.style.transform).toBe("translateX(-324px)");
    vi.advanceTimersByTime(200);
    expect(onDismiss).toHaveBeenCalledTimes(1);
  });
  it("dismisses at once under reduced motion, without following the finger", () => {
    const element = card();
    const onDismiss = vi.fn();
    bindSwipeDismiss(element, { onDismiss, reducedMotion: () => true });
    element.dispatchEvent(pointer("pointerdown", 100));
    element.dispatchEvent(pointer("pointermove", 300));
    expect(element.style.transform).toBe("");
    element.dispatchEvent(pointer("pointerup", 300));
    expect(onDismiss).toHaveBeenCalledTimes(1);
  });
  it("leaves a vertical scroll and a mouse drag alone, and swallows the click after a swipe", () => {
    const element = card();
    const onDismiss = vi.fn();
    bindSwipeDismiss(element, { onDismiss, reducedMotion: () => true });
    swipe(element, 20, 200);
    swipe(element, 200, 0, "mouse");
    expect(onDismiss).not.toHaveBeenCalled();
    const chip = document.createElement("button"); element.append(chip);
    const clicked = vi.fn(); chip.addEventListener("click", clicked);
    chip.dispatchEvent(pointer("pointerdown", 100));
    chip.dispatchEvent(pointer("pointermove", 110));
    chip.dispatchEvent(pointer("pointermove", 150));
    chip.dispatchEvent(pointer("pointerup", 150));
    chip.click();
    expect(clicked).not.toHaveBeenCalled();
  });
});

describe("in the thread", () => {
  it("offers Dismiss on a failed send, keeps a restorable unsent chip, and brings Edit and Retry back", () => {
    const history = new ConversationHistory();
    const changed = vi.fn(); const retry = vi.fn();
    const view = new ConversationView(document, history, { supervisor: "sup", editMessage: vi.fn(), retryMessage: retry, dismissalsChanged: changed });
    document.body.replaceChildren(view.element, view.unsent, view.pinned);
    refused(history, "x", "Do the burn down");
    view.update();
    const bubble = () => view.element.querySelector<HTMLElement>('.bub[data-state="error"]');
    const dismiss = bubble()!.querySelector<HTMLButtonElement>(".conversation-dismiss")!;
    expect(dismiss.getAttribute("aria-label")).toBe("Dismiss unsent message");
    expect(bubble()!.dataset.swipe).toBe("dismiss");
    // Last in the card's tab order: Edit and Retry keep their row.
    expect([...bubble()!.querySelectorAll("button")].map((button) => button.className)).toEqual(["conversation-edit", "conversation-retry", "conversation-dismiss"]);
    expect(view.unsent.hidden).toBe(true);
    dismiss.focus();
    dismiss.click();
    expect(bubble()).toBeNull();
    expect(changed).toHaveBeenCalledTimes(1);
    expect(view.unsent.hidden).toBe(false);
    expect(view.unsent.textContent).toBe("1 unsent messageShow");
    expect(view.unsent.getAttribute("aria-label")).toBe("Show 1 unsent message");
    // Keyboard focus follows to the way back, never the page body.
    expect(document.activeElement).toBe(view.unsent);
    refused(history, "z", "Second try", at(9, 5)); view.update();
    swipe(bubble()!, 0); // a tap is not a swipe
    expect(bubble()).not.toBeNull();
    bubble()!.querySelector<HTMLButtonElement>(".conversation-dismiss")!.click();
    expect(view.unsent.getAttribute("aria-label")).toBe("Show 2 unsent messages");
    view.unsent.click();
    expect(view.unsent.hidden).toBe(true);
    expect(view.element.querySelectorAll('.bub[data-state="error"]')).toHaveLength(2);
    expect(document.activeElement).toBe(view.element.querySelector('.bub[data-state="error"] .conversation-retry'));
    view.element.querySelector<HTMLButtonElement>('.bub[data-state="error"] .conversation-retry')!.click();
    expect(retry).toHaveBeenCalledWith(expect.objectContaining({ id: "x" }));
  });
  it("offers Dismiss on a Not confirmed send until it settles", () => {
    const history = new ConversationHistory();
    const view = new ConversationView(document, history, { supervisor: "sup", retryMessage: vi.fn() });
    document.body.replaceChildren(view.element);
    history.submit("u", "sup", "Is it green?", at(9, 0));
    history.unconfirmSilent(at(9, 0) + 20_000); view.update();
    expect(view.element.querySelector('.bub[data-state="unconfirmed"] .conversation-dismiss')).not.toBeNull();
    history.receive(reply(5, "answer", "Yes."), at(9, 0) + 30_000); view.update();
    expect(view.element.querySelector('.bub[data-state="unconfirmed"] .conversation-dismiss')).toBeNull();
  });
  it("folds the pinned question to a one-line bar while composing, opens it on tap, and lets the operator collapse it", () => {
    uninstall = installAttentionObjects();
    const history = new ConversationHistory();
    const view = new ConversationView(document, history, { supervisor: "sup", respond: vi.fn() });
    document.body.replaceChildren(view.element, view.pinned);
    history.reply(reply(10, "ask", "Waves are done.\n\n- **Ask:** open the PR to main and cut a release?"), at(9, 0));
    view.update();
    expect(view.pinned.dataset.collapsed).toBe("false");
    expect(view.pinned.querySelector(".pinned-label")?.textContent).toBe("Waiting on you");
    expect(view.pinned.querySelectorAll("button.chip")).toHaveLength(2);
    view.setComposing(true);
    expect(view.pinned.dataset.collapsed).toBe("true");
    expect(view.pinned.querySelectorAll("button.chip")).toHaveLength(0);
    const expand = view.pinned.querySelector<HTMLButtonElement>(".pinned-expand")!;
    expect(expand.textContent).toBe("Waiting on you: open the PR to main and cut a release?");
    expect(expand.getAttribute("aria-expanded")).toBe("false");
    expand.click();
    expect(view.pinned.dataset.collapsed).toBe("false");
    expect(document.activeElement).toBe(view.pinned.querySelector(".pinned-collapse"));
    // Composing again folds it; the operator's own collapse outlasts composing.
    view.setComposing(false); view.setComposing(true);
    expect(view.pinned.dataset.collapsed).toBe("true");
    view.setComposing(false);
    expect(view.pinned.dataset.collapsed).toBe("false");
    view.pinned.querySelector<HTMLButtonElement>(".pinned-collapse")!.click();
    expect(view.pinned.dataset.collapsed).toBe("true");
    expect(document.activeElement).toBe(view.pinned.querySelector(".pinned-expand"));
    view.setComposing(true); view.setComposing(false);
    expect(view.pinned.dataset.collapsed).toBe("true");
  });
  it("dismisses the pinned question: it unpins, and the thread copy keeps its choices", () => {
    uninstall = installAttentionObjects();
    const history = new ConversationHistory();
    const changed = vi.fn();
    const view = new ConversationView(document, history, { supervisor: "sup", respond: vi.fn(), dismissalsChanged: changed });
    const composer = document.createElement("textarea"); composer.id = "message-text";
    document.body.replaceChildren(view.element, view.pinned, composer);
    history.reply(reply(10, "ask", "Ship?"), at(9, 0)); view.update();
    view.pinned.querySelector<HTMLButtonElement>(".pinned-dismiss")!.click();
    expect(view.pinned.hidden).toBe(true);
    expect(changed).toHaveBeenCalledTimes(1);
    expect(document.activeElement).toBe(composer);
    const copy = view.element.querySelector<HTMLElement>('[data-kind="ask"]')!;
    expect(copy.dataset.retired).toBe("dismissed");
    expect(copy.querySelector(".ask-retired")?.textContent).toBe(RETIRED_LINES.dismissed);
    expect(copy.querySelectorAll("button.chip")).toHaveLength(2);
  });
  it("swipes the pinned question away", () => {
    uninstall = installAttentionObjects();
    const history = new ConversationHistory();
    const view = new ConversationView(document, history, { supervisor: "sup", respond: vi.fn() });
    document.body.replaceChildren(view.element, view.pinned);
    history.reply(reply(10, "ask", "Ship?"), at(9, 0)); view.update();
    const media = window.matchMedia;
    window.matchMedia = ((query: string) => ({ matches: query.includes("reduce"), media: query })) as typeof window.matchMedia;
    try { swipe(view.pinned, 200); } finally { window.matchMedia = media; }
    expect(view.pinned.hidden).toBe(true);
    expect(history.retirement(10)).toBe("dismissed");
  });
  it("shows a stale question quiet, saying why, with no choices", () => {
    uninstall = installAttentionObjects();
    const history = new ConversationHistory();
    history.currentSession = "new";
    const view = new ConversationView(document, history, { supervisor: "sup", respond: vi.fn() });
    document.body.replaceChildren(view.element, view.pinned);
    history.reply(reply(10, "ask", "Open the PR to main and cut a release?"), at(9, 0), "old");
    history.reply(reply(11, "blocker", "Gate red."), at(9, 1), "old");
    view.update();
    expect(view.pinned.hidden).toBe(true);
    const ask = view.element.querySelector<HTMLElement>('[data-kind="ask"]')!;
    expect(ask.dataset.retired).toBe("session-ended");
    expect(ask.querySelector(".ask-retired")?.textContent).toBe("No longer waiting: the session that asked has ended.");
    expect(ask.querySelectorAll("button")).toHaveLength(0);
    const blocker = view.element.querySelector<HTMLElement>('[data-kind="blocker"]')!;
    expect(blocker.dataset.waiting).toBe("false");
    expect(blocker.querySelector(".blk-hint")).toBeNull();
    expect(blocker.querySelector(".blk-retired")?.textContent).toBe(RETIRED_LINES["session-ended"]);
  });
});
