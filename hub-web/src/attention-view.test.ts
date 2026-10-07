// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { renderAttentionPanel } from "./attention-view";
import { createAttentionItem } from "./attention";
import { NOTICE_KIND, planNotice } from "./operator-notices";
import type { AttentionItem } from "./types";

const now = Date.parse("2026-09-07T12:00:00Z");
const event = (id: string, kind = "daemon_disconnected"): AttentionItem => ({
  id, kind, machineId: "machine", machineLabel: "Workstation",
  session: `a-long-session-codename-${id}`, message: "Inspect the transport and reconnect when the machine is reachable.",
  createdAt: "2026-09-07T11:59:00Z",
});
const callbacks = () => ({ dismiss: vi.fn(), act: vi.fn(), copy: vi.fn() });

describe("attention timeline", () => {
  it("keeps the empty timeline quiet without count badges", () => {
    const root = document.createElement("div");
    renderAttentionPanel(root, [], callbacks(), { now });
    expect(root.querySelector(".attention-empty")?.textContent).toContain("All clear");
    expect(root.querySelector(".attention-counts")).toBeNull();
    expect(root.querySelector(".attention-item")).toBeNull();
  });

  it("names an outage instead of saying All clear beside a reconnect banner (cas-edcd)", () => {
    const root = document.createElement("div");
    renderAttentionPanel(root, [], callbacks(), { now, outage: "Not all clear. Atlas · Linux is reconnecting." });
    const empty = root.querySelector(".attention-empty")!;
    expect(empty.classList.contains("outage")).toBe(true);
    expect(empty.querySelector("p")?.textContent).toBe("Not all clear. Atlas · Linux is reconnecting.");
    expect(empty.textContent).not.toContain("All clear.");
    // Events still win over the empty state: the outage line is for an empty rail only.
    const busy = document.createElement("div");
    renderAttentionPanel(busy, [event("1")], callbacks(), { now, outage: "Not all clear. Atlas · Linux is reconnecting." });
    expect(busy.querySelector(".attention-empty")).toBeNull();
  });

  it("tells the last event's time in the app's 24-hour clock, not the browser's locale (cas-0cd1)", () => {
    const at = new Date(now);
    at.setHours(9, 7, 32, 0);
    const earlier = new Date(now);
    earlier.setHours(8, 0, 0, 0);
    const acknowledged = (id: string, createdAt: string): AttentionItem => ({ ...event(id), createdAt, acknowledgedAt: createdAt });
    const items = [acknowledged("old", earlier.toISOString()), acknowledged("latest", at.toISOString())];
    const root = document.createElement("div");
    renderAttentionPanel(root, items, callbacks(), { now: at.getTime() + 60_000 });
    const stamp = root.querySelector<HTMLTimeElement>(".attention-empty time.attention-last-event")!;
    expect(stamp.dateTime).toBe(at.toISOString());
    expect(stamp.textContent).toBe("Last event 09:07");
    expect(stamp.textContent).not.toMatch(/AM|PM|\d+\/\d+\/\d+|:32/);

    // On a later day it names the day, as thread times do, and a redraw with
    // nothing new still moves the label over midnight.
    const tomorrow = at.getTime() + 86_400_000;
    const later = document.createElement("div");
    renderAttentionPanel(later, items, callbacks(), { now: tomorrow });
    const day = `${["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"][at.getMonth()]} ${at.getDate()}`;
    expect(later.querySelector(".attention-last-event")?.textContent).toBe(`Last event ${day}, 09:07`);
    renderAttentionPanel(root, items, callbacks(), { now: tomorrow });
    expect(root.querySelector(".attention-last-event")).toBe(stamp);
    expect(stamp.textContent).toBe(`Last event ${day}, 09:07`);
  });

  it("gives all twelve events one primary text action and a timestamp", () => {
    const root = document.createElement("div");
    renderAttentionPanel(root, Array.from({ length: 12 }, (_, i) => event(String(i))), callbacks(), { now });
    const items = root.querySelectorAll(".attention-item");
    expect(items).toHaveLength(12);
    for (const item of items) {
      expect(item.querySelectorAll(".attention-action")).toHaveLength(1);
      expect(item.querySelector(".attention-action")?.textContent).toBe("Retry");
      expect(item.querySelector("time")?.getAttribute("datetime")).toBe("2026-09-07T11:59:00Z");
      expect(item.querySelector(".attention-explicit-dismiss")?.textContent).toBe("Dismiss");
    }
  });

  it("shows Dismiss as the primary action when no recovery action exists", () => {
    const root = document.createElement("div");
    const cb = callbacks();
    const item = event("one", "checkpoint");
    renderAttentionPanel(root, [item], cb, { now });
    const dismiss = root.querySelector<HTMLButtonElement>(".attention-action")!;
    expect(dismiss.textContent).toBe("Dismiss");
    dismiss.click();
    expect(cb.dismiss).toHaveBeenCalledWith([item]);
  });

  it("states pending enrichment without an animated decoration", () => {
    const root = document.createElement("div");
    renderAttentionPanel(root, [{ ...event("pending", "unfamiliar_event"), enrichmentPending: true }], callbacks(), { now });
    expect(root.querySelector(".attention-enriching")?.textContent).toBe("Enriching…");
  });

  it("retains action-before-dismiss ordering and expands/collapses groups", async () => {
    const root = document.createElement("div");
    const cb = callbacks();
    const item = event("one");
    renderAttentionPanel(root, [item], cb, { now });
    root.querySelector<HTMLButtonElement>(".attention-action")!.click();
    expect(cb.act).toHaveBeenCalledWith(item, "retry");
    await Promise.resolve();
    expect(cb.dismiss).toHaveBeenCalledWith([item]);
    const toggle = root.querySelector<HTMLButtonElement>(".attention-group-toggle")!;
    const body = root.querySelector<HTMLElement>(".attention-group-body")!;
    toggle.click();
    expect(body.hidden).toBe(true);
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    toggle.click();
    expect(body.hidden).toBe(false);
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
  });
});

describe("heartbeat redraws (cas-a5c6 QA F03)", () => {
  it("leaves an unchanged panel, its focus and an opened Details alone, and redraws when something changes", () => {
    const root = document.createElement("div"); document.body.replaceChildren(root);
    renderAttentionPanel(root, [event("1")], callbacks(), { now });
    const dismiss = root.querySelector<HTMLButtonElement>("[data-role='dismiss']")!;
    const details = root.querySelector<HTMLDetailsElement>("details")!;
    details.open = true;
    dismiss.focus();
    // The 5 s heartbeat: same items, same minute.
    renderAttentionPanel(root, [event("1")], callbacks(), { now: now + 5_000 });
    expect(root.querySelector("[data-role='dismiss']")).toBe(dismiss);
    expect(document.activeElement).toBe(dismiss);
    expect(details.open).toBe(true);
    // A new item is a real change.
    renderAttentionPanel(root, [event("1"), event("2")], callbacks(), { now: now + 10_000 });
    expect(root.querySelector("[data-role='dismiss']")).not.toBe(dismiss);
    // The next minute is not a redraw: the ages move on in place (round 3).
    const before = root.querySelector("[data-role='dismiss']");
    renderAttentionPanel(root, [event("1"), event("2")], callbacks(), { now: now + 70_000 });
    expect(root.querySelector("[data-role='dismiss']")).toBe(before);
  });
});

describe("redraws keep each notice's state by its key (cas-a5c6 QA round 3)", () => {
  const warn = (id: string, at: string): AttentionItem => ({ ...event(id, "delivery_stall"), createdAt: at, message: `notice ${id}` });
  const card = (root: HTMLElement, id: string) => [...root.querySelectorAll<HTMLElement>("[data-attention-id]")].find((node) => node.textContent?.includes(`notice ${id}`))!;

  it("keeps focus on the same notice's Dismiss when a new notice arrives above it (F01)", () => {
    const root = document.createElement("div"); document.body.replaceChildren(root);
    const a = warn("a", "2026-09-07T11:50:00Z");
    renderAttentionPanel(root, [a], callbacks(), { now });
    card(root, "a").querySelector<HTMLElement>("[data-role='dismiss']")!.focus();
    renderAttentionPanel(root, [a, warn("b", "2026-09-07T11:59:00Z")], callbacks(), { now });
    const focused = document.activeElement as HTMLElement;
    expect(focused.dataset.role).toBe("dismiss");
    expect(focused.closest("[data-attention-id]")).toBe(card(root, "a"));
  });

  it("keeps an opened Details open and Copy focused across a minute, and its age moves on in place (F02, F03)", () => {
    const root = document.createElement("div"); document.body.replaceChildren(root);
    const a = warn("a", "2026-09-07T11:59:00Z");
    renderAttentionPanel(root, [a], callbacks(), { now });
    const details = card(root, "a").querySelector("details")!;
    details.open = true;
    const copy = card(root, "a").querySelector<HTMLElement>("[data-role='copy']")!;
    copy.focus();
    const time = card(root, "a").querySelector("time.attention-time")!;
    const before = time.textContent;
    renderAttentionPanel(root, [a], callbacks(), { now: now + 61_000 });
    expect(card(root, "a").querySelector("[data-role='copy']")).toBe(copy);
    expect(document.activeElement).toBe(copy);
    expect(details.open).toBe(true);
    expect(time.textContent).not.toBe(before);
    // A measured outage changes the panel header, not this notice's Details.
    const refreshedCallbacks = callbacks();
    renderAttentionPanel(root, [a], refreshedCallbacks, { now: now + 61_000, outage: "Connection unsteady — checking…" });
    copy.click();
    expect(refreshedCallbacks.copy).toHaveBeenCalledWith(card(root, "a").querySelector("pre")!.textContent);
    expect(card(root, "a").querySelector("[data-role='copy']")).toBe(copy);
    expect(document.activeElement).toBe(copy);
    // A real change redraws, and the opened Details stays open on its notice.
    renderAttentionPanel(root, [a, warn("b", "2026-09-07T11:59:30Z")], callbacks(), { now: now + 62_000 });
    expect(card(root, "a").querySelector("details")!.open).toBe(true);
    expect(card(root, "b").querySelector("details")!.open).toBe(false);
    expect((document.activeElement as HTMLElement).dataset.role).toBe("copy");
    expect((document.activeElement as HTMLElement).closest("[data-attention-id]")).toBe(card(root, "a"));
  });
});

describe("a rebuilt page keeps the Attention panel as the operator left it (cas-f486)", () => {
  const warn = (id: string): AttentionItem => ({ ...event(id, "delivery_stall"), createdAt: "2026-09-07T11:59:00Z", message: `notice ${id}` });
  const panel = () => { const root = document.createElement("section"); root.id = "attention-panel"; return root; };

  it("carries opened Details and focus into the panel that replaces it", () => {
    const first = panel(); document.body.replaceChildren(first);
    renderAttentionPanel(first, [warn("a")], callbacks(), { now });
    first.querySelector("details")!.open = true;
    first.querySelector<HTMLElement>("[data-role='copy']")!.focus();
    // A wake from sleep rebuilds the shell: a fresh, empty panel element.
    const second = panel(); document.body.replaceChildren(second);
    expect(document.activeElement).toBe(document.body);
    renderAttentionPanel(second, [warn("a")], callbacks(), { now: now + 600_000 });
    expect(second.querySelector("details")!.open).toBe(true);
    expect(document.activeElement).toBe(second.querySelector("[data-role='copy']"));
  });

  it("does not pull back focus the operator moved elsewhere", () => {
    const first = panel(); const elsewhere = document.createElement("button");
    document.body.replaceChildren(first, elsewhere);
    renderAttentionPanel(first, [warn("b")], callbacks(), { now });
    first.querySelector<HTMLElement>("[data-role='dismiss']")!.focus();
    elsewhere.focus();
    elsewhere.blur();
    const second = panel(); document.body.replaceChildren(second);
    renderAttentionPanel(second, [warn("b")], callbacks(), { now });
    expect(document.activeElement).toBe(document.body);
  });
});

describe("Attention notice labels (cas-7cb3)", () => {
  it("uses catalog project/supervisor names and refreshes them without a notice change", () => {
    const root = document.createElement("div"); const item = { ...event("label"), session: "Accounting-rapid-gazelle-52" };
    let label = "Accounting · happy-cheetah-1";
    const options = { now, sessionLabel: () => label };
    renderAttentionPanel(root, [item], callbacks(), options);
    expect(root.querySelector(".attention-group-label")?.textContent).toBe(label);
    expect(root.textContent).not.toContain(item.session);
    expect(root.querySelector(".attention-dismiss-group")).toBeNull();
    label = "Ledger · happy-cheetah-1";
    renderAttentionPanel(root, [item], callbacks(), options);
    expect(root.querySelector(".attention-group-label")?.textContent).toBe(label);
    renderAttentionPanel(root, [item, { ...item, id: "other", message: "Another failure" }], callbacks(), options);
    expect(root.querySelector(".attention-dismiss-group")).not.toBeNull();
  });
  it("does not show an opaque id when catalog metadata is unavailable", () => {
    const root = document.createElement("div");
    renderAttentionPanel(root, [{ ...event("fallback"), session: "f98e41d1-c544-443a-8256-22360ddf3701" }], callbacks(), { now });
    expect(root.querySelector(".attention-group-label")?.textContent).toBe("Workstation · Session unavailable");
  });
});

// cas-ed87: a delivery notice's Details reads as the message, never as the
// {summary, message} object cas-7cb3 keeps for it.
describe("delivery notice Details (cas-ed87)", () => {
  const SUMMARY = "Supervisor hasn't seen: worker died: daring-robin-43 (9m)";
  const MESSAGE = "The supervisor (happy-cheetah-1, Codex) was told 9 minutes ago that worker died: daring-robin-43, and the message never reached it.";
  const notice = (summary: string, message: string) => {
    const plan = planNotice("atlas", "Accounting-rapid-gazelle-52", { notification_id: 901, reply_to: null, message, summary, device_id: "*", notice: { source: "relay-watchdog", subject: 890 } }, () => false);
    if (plan.action !== "raise") throw new Error("expected a raised notice");
    return createAttentionItem({ id: "notice-901", machineId: "atlas", machineLabel: "Atlas", session: "Accounting-rapid-gazelle-52", kind: NOTICE_KIND, createdAt: "2026-09-06T17:49:00Z" }, plan.content);
  };
  const opened = (item: AttentionItem) => {
    const root = document.createElement("div");
    const calls = callbacks();
    renderAttentionPanel(root, [item], calls, { now });
    root.querySelector<HTMLButtonElement>("[data-role=copy]")!.click();
    return { text: root.querySelector(".attention-payload pre")?.textContent ?? "", copied: calls.copy.mock.calls[0]?.[0] as string };
  };

  it("shows the summary once and then the message as plain text, and Copy copies the same", () => {
    const { text, copied } = opened(notice(SUMMARY, MESSAGE));
    expect(text).toBe(`${SUMMARY}\n\n${MESSAGE}`);
    expect(text).not.toMatch(/^\s*[{[]|"summary"|"message"/);
    expect(copied).toBe(text);
  });

  it("shows only the message when the summary is empty or already in it", () => {
    expect(opened(notice("", MESSAGE)).text).toBe(MESSAGE);
    expect(opened(notice("  ", MESSAGE)).text).toBe(MESSAGE);
    expect(opened(notice("worker died: daring-robin-43", MESSAGE)).text).toBe(MESSAGE);
  });

  it("reads a notice persisted before cas-7cb3, with no payload, as its message", () => {
    const legacy = createAttentionItem({ id: "old", machineId: "atlas", machineLabel: "Atlas", session: "s", kind: NOTICE_KIND, createdAt: "2026-09-06T17:49:00Z" }, { headline: SUMMARY, detail: MESSAGE, severity: "warning", action: "view_pane" });
    const { text } = opened(legacy);
    expect(text).not.toMatch(/"summary"|"message"|^\s*\{/);
    expect(text).toContain(MESSAGE);
  });
});

describe("Open conversation on the conversation already open (cas-d043 G12)", () => {
  const item = (session: string) => createAttentionItem({ id: `n-${session}`, machineId: "atlas", machineLabel: "Atlas", session, kind: "session_transport", createdAt: "2026-10-06T12:00:00Z" }, { headline: "Lost connection to the session", severity: "warning", action: "view_pane" });
  const actions = (open?: { machineId: string; session?: string }) => {
    const container = document.createElement("div");
    renderAttentionPanel(container, [item("s1")], { act: vi.fn(), dismiss: vi.fn(), copy: vi.fn() }, { now: Date.parse("2026-10-06T12:01:00Z"), ...(open ? { openConversation: open } : {}) });
    return [...container.querySelectorAll<HTMLButtonElement>(".attention-actions > button")].map((button) => button.textContent);
  };
  it("offers only Dismiss for an item about the open conversation", () => {
    expect(actions({ machineId: "atlas", session: "s1" })).not.toContain("Open conversation");
    expect(actions({ machineId: "atlas", session: "s1" })).toContain("Dismiss");
  });
  it("still opens another conversation", () => {
    expect(actions({ machineId: "atlas", session: "s2" })).toContain("Open conversation");
    expect(actions()).toContain("Open conversation");
  });
});
