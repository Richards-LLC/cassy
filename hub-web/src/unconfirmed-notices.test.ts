// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { ConversationHistory } from "./conversation-history";
import { ConversationView } from "./conversation-view";

const at = (hh: number, mm: number) => new Date(2026, 9, 2, hh, mm).getTime();

function unconfirmed(history: ConversationHistory, id: string, text: string, when: number): void {
  history.submit(id, "sup", text, when);
  history.unconfirmSilent(when + 20_000);
}

function mount(history: ConversationHistory, options: Partial<ConstructorParameters<typeof ConversationView>[2] & object> = {}) {
  const retry = vi.fn();
  const view = new ConversationView(document, history, { supervisor: "sup", retryMessage: retry, editMessage: vi.fn(), ...options });
  document.body.replaceChildren(view.element, view.unsent);
  view.update();
  const bubbles = () => [...view.element.querySelectorAll<HTMLElement>('.bub[data-state="unconfirmed"]')];
  return { view, retry, bubbles };
}

describe("several unconfirmed messages (cas-b00c, journey F19)", () => {
  it("read as one notice with Review, not a stack of cards each with Retry", () => {
    const history = new ConversationHistory();
    unconfirmed(history, "a", "First", at(9, 0));
    unconfirmed(history, "b", "Second", at(9, 1));
    unconfirmed(history, "c", "Third", at(9, 2));
    const { view, retry, bubbles } = mount(history);
    expect(bubbles().map((bubble) => bubble.dataset.grouped)).toEqual(["member", "member", "last"]);
    const notices = view.element.querySelectorAll<HTMLElement>('.conversation-unconfirmed[role="status"]');
    expect(notices).toHaveLength(1);
    expect(notices[0]!.textContent).toBe("3 messages not confirmed · Cassy couldn't confirm delivery to sup. Review them to retry.");
    expect(view.element.querySelectorAll(".conversation-retry, .conversation-dismiss")).toHaveLength(0);
    // Each message is still there to read.
    expect(bubbles().map((bubble) => bubble.querySelector("p")?.textContent)).toEqual(["First", "Second", "Third"]);

    const review = view.element.querySelector<HTMLButtonElement>(".conversation-review")!;
    expect(review.getAttribute("aria-label")).toBe("Review 3 messages not confirmed");
    expect(review.getAttribute("aria-expanded")).toBe("false");
    review.click();
    expect(bubbles().map((bubble) => bubble.dataset.grouped)).toEqual([undefined, undefined, undefined]);
    const retries = view.element.querySelectorAll<HTMLButtonElement>(".conversation-retry");
    expect(retries).toHaveLength(3);
    expect(document.activeElement, "the keyboard user lands on the first Retry").toBe(retries[0]);
    // The × on a message that may have arrived dismisses the notice, and says so.
    expect([...view.element.querySelectorAll(".conversation-dismiss")].map((button) => button.getAttribute("aria-label"))).toEqual(["Dismiss this notice", "Dismiss this notice", "Dismiss this notice"]);
    retries[1]!.click();
    expect(retry).toHaveBeenCalledWith(expect.objectContaining({ id: "b" }));

    const less = view.element.querySelector<HTMLButtonElement>(".conversation-review-less")!;
    expect(less.getAttribute("aria-expanded")).toBe("true");
    less.click();
    expect(view.element.querySelectorAll(".conversation-retry")).toHaveLength(0);
    expect(document.activeElement).toBe(view.element.querySelector(".conversation-review"));
  });

  it("groups only consecutive ones: a single unconfirmed message keeps its own card", () => {
    const history = new ConversationHistory();
    unconfirmed(history, "a", "Alone", at(9, 0));
    history.submit("d", "sup", "Delivered", at(9, 1));
    history.acknowledge({ client_ref: "d", notification_id: 5, target: "sup", stamped: true });
    unconfirmed(history, "b", "Also alone", at(9, 2));
    const { view, bubbles } = mount(history);
    expect(bubbles().map((bubble) => bubble.dataset.grouped)).toEqual([undefined, undefined]);
    expect(view.element.querySelectorAll(".conversation-review")).toHaveLength(0);
    expect(view.element.querySelectorAll(".conversation-retry")).toHaveLength(2);
  });

  it("dismissed, they are counted as not confirmed, not as unsent", () => {
    const history = new ConversationHistory();
    unconfirmed(history, "a", "Maybe arrived", at(9, 0));
    const { view } = mount(history);
    view.element.querySelector<HTMLButtonElement>(".conversation-dismiss")!.click();
    expect(view.unsent.getAttribute("aria-label")).toBe("Show 1 message not confirmed");
    history.submit("r", "sup", "Refused", at(9, 1));
    history.reject("r", "forbidden");
    view.update();
    view.element.querySelector<HTMLButtonElement>('.bub[data-state="error"] .conversation-dismiss')!.click();
    expect(view.unsent.getAttribute("aria-label")).toBe("Show 2 messages not sent or not confirmed");
  });

  it("the list preview says a message is not confirmed until the supervisor replies after it", () => {
    const history = new ConversationHistory();
    unconfirmed(history, "a", "Is the gate green?", at(9, 0));
    expect(history.preview()).toBe("Not confirmed: Is the gate green?");
  });
});

describe("Retry while another device holds control (cas-b00c, journey F18)", () => {
  it("is not pressable while waiting, and comes back when control is released", () => {
    const history = new ConversationHistory();
    history.submit("x", "sup", "Ship it", at(9, 0));
    history.reject("x", "forbidden");
    let holder: string | undefined = "Studio iPad";
    const { view, retry } = mount(history, { takeControl: vi.fn(), controlHolder: () => holder });
    const button = () => view.element.querySelector<HTMLButtonElement>(".conversation-retry")!;
    expect(button().getAttribute("aria-disabled")).toBe("true");
    expect(button().getAttribute("aria-description")).toBe("Waiting for Studio iPad to release control");
    button().click();
    expect(retry, "a retry the hub would refuse again is not sent").not.toHaveBeenCalled();
    holder = undefined; view.update();
    expect(button().hasAttribute("aria-disabled")).toBe(false);
    button().click();
    expect(retry).toHaveBeenCalledTimes(1);
  });
});
