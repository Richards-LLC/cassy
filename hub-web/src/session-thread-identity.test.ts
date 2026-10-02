// @vitest-environment jsdom
// cas-55a4: a session's Commander thread is its own conversation. Other
// sessions' turns sit beside it, collapsed and labelled; a session that has
// not written yet says so and shows what it is doing; a project's live
// sessions are grouped with the most recent one marked.
import { describe, expect, it, vi } from "vitest";
import { ConversationHistory, sessionCodename } from "./conversation-history";
import { ConversationView, earlierSessionLabel, emptyActivityText } from "./conversation-view";
import { ConversationList, conversationRowMarkup, groupConversationRows, type ConversationRow } from "./conversation-list";
import type { ConversationHistoryMessage, ConversationHistoryReply } from "./types";

const at = (day: number, hh: number, mm: number) => new Date(2026, 8, day, hh, mm).toISOString();
const send = (id: number, text: string, session: string | undefined, when: string): ConversationHistoryMessage => ({
  notification_id: id, target: "supervisor", text, state: "acknowledged", stamped: true, device_id: "phone", operator_label: "Pixel 10", at: when, ...(session === undefined ? {} : { session }),
});
const said = (id: number, message: string, session: string | undefined, when: string, kind: ConversationHistoryReply["kind"] = "answer"): ConversationHistoryReply => ({
  notification_id: id, reply_to: null, message, summary: "", device_id: "phone", kind, attachments: [], at: when, ...(session === undefined ? {} : { session }),
});

describe("session-bound thread history (cas-55a4)", () => {
  it("keeps only the attached session's turns in the thread and files the rest by session", () => {
    const history = new ConversationHistory();
    history.currentSession = "gabber-studio-calm-puma-34";
    // A project-wide page, as a daemon before cas-55a4 still sends it.
    history.hydrateSend(send(1, "Old question", "gabber-studio-noble-cheetah-84", at(29, 21, 30)));
    history.hydrateReply(said(2, "Old answer", "gabber-studio-noble-cheetah-84", at(29, 21, 41)));
    history.hydrateReply(said(3, "Watchdog notice", "", at(30, 17, 20), "blocker"));
    history.hydrateReply(said(4, "Mine", "gabber-studio-calm-puma-34", at(30, 9, 0)));
    history.hydrateSend(send(5, "Stamp-less row from an old daemon", undefined, at(30, 9, 5)));
    expect(history.events.map((event) => event.kind === "send" ? event.value.text : event.value.message)).toEqual(["Mine", "Stamp-less row from an old daemon"]);
    // Another session's question never waits on the operator here.
    expect(history.waiting()).toEqual([]);
    expect(history.preview()).toBe("You: Stamp-less row from an old daemon");
    const earlier = history.earlierSessions();
    expect(earlier.map((entry) => entry.session)).toEqual(["", "gabber-studio-noble-cheetah-84"]);
    expect(earlier[1]!.events.map((event) => event.kind)).toEqual(["send", "reply"]);
    // Re-hydrating the same page (a reconnect) does not duplicate anything.
    history.hydrateReply(said(2, "Old answer", "gabber-studio-noble-cheetah-84", at(29, 21, 41)));
    expect(history.earlierSessions()[1]!.events).toHaveLength(2);
  });

  it("names a session by its codename and an earlier section by codename and day", () => {
    expect(sessionCodename("gabber-studio-calm-puma-34")).toBe("calm-puma-34");
    expect(sessionCodename("Accounting-wise-lion-31")).toBe("wise-lion-31");
    expect(sessionCodename("main")).toBe("main");
    const now = new Date(2026, 8, 30, 12, 0).getTime();
    expect(earlierSessionLabel({ session: "Accounting-wise-lion-31", lastAt: new Date(2026, 8, 29, 21, 20).getTime() }, now)).toBe("Earlier session wise-lion-31, Yesterday");
    expect(earlierSessionLabel({ session: "", lastAt: undefined }, now)).toBe("Earlier messages with no session recorded");
  });

  it("renders earlier sessions collapsed, outside the log, with a date on every turn", () => {
    const history = new ConversationHistory();
    history.currentSession = "Accounting-rapid-gazelle-52";
    history.hydrateReply(said(10, "worker died: daring-robin-43", "Accounting-wise-lion-31", at(29, 21, 20), "blocker"));
    history.hydrateReply(said(11, "Working on cas-1a88.", "Accounting-rapid-gazelle-52", at(30, 9, 0)));
    const view = new ConversationView(document, history, { supervisor: "happy-cheetah-1", header: false });
    document.body.replaceChildren(view.element);
    view.update();
    const log = view.element.querySelector('[role="log"]')!;
    expect(log.textContent).toContain("Working on cas-1a88.");
    expect(log.textContent).not.toContain("daring-robin-43");
    const section = view.element.querySelector<HTMLElement>('section.earlier-sessions[aria-label="Earlier sessions"]')!;
    expect(section.hidden).toBe(false);
    const details = section.querySelector<HTMLDetailsElement>("details.earlier-session")!;
    expect(details.open).toBe(false);
    expect(details.querySelector(".earlier-label")?.textContent).toMatch(/^Earlier session wise-lion-31, /);
    expect(details.querySelector(".earlier-count")?.textContent).toBe("1 message");
    const time = details.querySelector("time")!;
    expect(time.textContent).toMatch(/ 21:20$/);
    expect(time.textContent).not.toBe("21:20");
    // Nothing in an earlier section is an object to act on.
    expect(details.querySelectorAll("button")).toHaveLength(0);
    // The section sits above the thread, and an opened section stays open on repaint.
    expect(section.nextElementSibling?.classList.contains("conversation-load-earlier")).toBe(true);
    details.open = true;
    history.hydrateReply(said(12, "Still going.", "Accounting-rapid-gazelle-52", at(30, 9, 5)));
    view.update();
    expect(section.querySelector<HTMLDetailsElement>("details.earlier-session")!.open).toBe(true);
  });

  it("says a session has not written yet, shows its last activity, and offers its Terminal", () => {
    const history = new ConversationHistory();
    history.currentSession = "gabber-studio-calm-puma-34";
    history.hydrateReply(said(2, "Old answer", "gabber-studio-noble-cheetah-84", at(29, 21, 41)));
    const openTerminal = vi.fn();
    const now = Date.now();
    const view = new ConversationView(document, history, { supervisor: "calm-puma-34", project: "gabber-studio", header: false, activity: () => ({ at: now - 3 * 60_000, label: "supervisor → wild-shark-68" }), openTerminal });
    document.body.replaceChildren(view.element);
    view.update();
    const empty = view.element.querySelector<HTMLElement>(".empty")!;
    expect(empty.hidden).toBe(false);
    expect(empty.querySelector(".said")?.textContent).toBe("No Commander messages from this session yet. The supervisor (calm-puma-34) will write here when it needs a decision.");
    expect(empty.querySelector(".empty-activity")?.textContent).toBe("Last activity 3m ago · supervisor → wild-shark-68");
    empty.querySelector<HTMLButtonElement>(".empty-terminal")!.click();
    expect(openTerminal).toHaveBeenCalledOnce();
    // The other session's thread is not shown as this one's: it is the
    // collapsed section under the card.
    expect(view.element.querySelector('[role="log"]')!.textContent).not.toContain("Old answer");
    const section = view.element.querySelector<HTMLElement>("section.earlier-sessions")!;
    expect(section.hidden).toBe(false);
    expect(section.previousElementSibling).toBe(empty);
  });

  it("words last activity plainly", () => {
    const now = new Date(2026, 8, 30, 12, 0).getTime();
    expect(emptyActivityText({ at: now - 30_000 }, now)).toBe("Last activity just now");
    expect(emptyActivityText({ at: now - 2 * 3_600_000, label: "Commander → supervisor" }, now)).toBe("Last activity 2h ago · Commander → supervisor");
  });
});

describe("grouped project sessions (cas-55a4)", () => {
  const base = { machineId: "atlas", host: "Atlas · Linux", projectDir: "/projects/gabber-studio", freshness: "", connection: "Live", attention: 0, selected: false } satisfies Partial<ConversationRow>;
  const row = (session: string, activityAt?: number, extra: Partial<ConversationRow> = {}): ConversationRow => ({ ...base, key: `atlas:${session}`, session, supervisor: session, ...(activityAt === undefined ? {} : { activityAt }), ...extra });

  it("puts one project's sessions together, most recent first and marked", () => {
    const rows = groupConversationRows([
      row("noble-cheetah-84", 100),
      row("solo", 50, { projectDir: "/projects/cas-src" }),
      row("calm-puma-34", 300),
      row("wild-shark-68", 200),
    ]);
    expect(rows.map((item) => item.session)).toEqual(["calm-puma-34", "wild-shark-68", "noble-cheetah-84", "solo"]);
    expect(rows.map((item) => item.group?.active ?? null)).toEqual([true, false, false, null]);
    expect(rows[0]!.group).toMatchObject({ first: true, size: 3, label: "gabber-studio · 3 sessions on Atlas" });
    expect(conversationRowMarkup(rows[0]!)).toContain('<span class="conversation-session-mark">Most recent</span>');
    expect(conversationRowMarkup(rows[1]!)).not.toContain("Most recent");
  });

  it("keeps sessions of one project on different machines apart", () => {
    const rows = groupConversationRows([row("a-b-1", 1), row("c-d-2", 2, { machineId: "studio", key: "studio:c-d-2" })]);
    expect(rows.every((item) => item.group === undefined)).toBe(true);
  });

  it("asks before ending a session, and only the confirmation ends it", async () => {
    const container = document.createElement("nav"); document.body.replaceChildren(container);
    const list = new ConversationList();
    const rows = groupConversationRows([row("calm-puma-34", 300, { canEnd: true }), row("noble-cheetah-84", 100, { canEnd: true })]);
    let resolve!: () => void;
    const end = vi.fn((_row: ConversationRow) => new Promise<void>((ok) => { resolve = ok; }));
    list.render(container, rows, vi.fn(), end);
    expect(container.querySelector(".conversation-group-head")?.textContent).toBe("gabber-studio · 2 sessions on Atlas");
    expect([...container.children].map((node) => node.className.split(" ")[0])).toEqual(["conversation-group-head", "conversation-row", "conversation-end", "conversation-row", "conversation-end"]);
    const control = container.querySelectorAll<HTMLElement>(".conversation-end")[1]!;
    const ask = control.querySelector<HTMLButtonElement>(".conversation-end-ask")!;
    expect(ask.getAttribute("aria-label")).toBe("End session noble-cheetah-84 on Atlas");
    ask.click();
    expect(end).not.toHaveBeenCalled();
    expect(control.querySelector(".conversation-end-question")?.textContent).toBe("End noble-cheetah-84 on Atlas? Its supervisor and workers stop.");
    control.querySelector<HTMLButtonElement>(".conversation-end-cancel")!.click();
    expect(control.querySelector(".conversation-end-ask")).not.toBeNull();
    control.querySelector<HTMLButtonElement>(".conversation-end-ask")!.click();
    control.querySelector<HTMLButtonElement>(".conversation-end-confirm")!.click();
    expect(end).toHaveBeenCalledOnce();
    expect(end.mock.calls[0]![0].session).toBe("noble-cheetah-84");
    expect(control.querySelector('[role="status"]')?.textContent).toBe("Ending noble-cheetah-84…");
    resolve();
  });

  it("opens the confirmation at once in a list rebuilt since the control was made, focused on Cancel (cas-d6bf)", () => {
    // The shell rebuilds #conversation-list when a conversation opens; the
    // End control is kept across that render. Its repaint once went to the
    // detached list and emptied the live one until the next catalog poll.
    const list = new ConversationList();
    const rows = groupConversationRows([row("calm-puma-34", 300, { canEnd: true }), row("noble-cheetah-84", 100, { canEnd: true })]);
    const before = document.createElement("nav"); before.id = "conversation-list"; document.body.replaceChildren(before);
    list.render(before, rows, vi.fn(), vi.fn(async () => {}));
    const live = document.createElement("nav"); live.id = "conversation-list"; document.body.replaceChildren(live);
    list.render(live, rows, vi.fn(), vi.fn(async () => {}));
    const control = live.querySelectorAll<HTMLElement>(".conversation-end")[1]!;
    expect(control.dataset.state).toBe("idle");
    control.querySelector<HTMLButtonElement>(".conversation-end-ask")!.click();
    expect(live.querySelectorAll(".conversation-row")).toHaveLength(2);
    expect(before.children).toHaveLength(0);
    expect(control.parentElement).toBe(live);
    expect(control.dataset.state).toBe("confirm");
    expect(control.querySelector(".conversation-end-question")?.textContent).toBe("End noble-cheetah-84 on Atlas? Its supervisor and workers stop.");
    expect(document.activeElement).toBe(control.querySelector(".conversation-end-cancel"));
    // Cancel hands focus back to End session.
    control.querySelector<HTMLButtonElement>(".conversation-end-cancel")!.click();
    expect(live.querySelectorAll(".conversation-row")).toHaveLength(2);
    expect(document.activeElement).toBe(control.querySelector(".conversation-end-ask"));
  });

  it("gives End session its own column only on rows that can end (cas-339a)", () => {
    const container = document.createElement("nav"); document.body.replaceChildren(container);
    const rows = groupConversationRows([row("calm-puma-34", 300, { canEnd: true }), row("noble-cheetah-84", 100)]);
    new ConversationList().render(container, rows, vi.fn(), vi.fn(async () => {}));
    expect([...container.querySelectorAll(".conversation-row")].map((node) => node.classList.contains("endable"))).toEqual([true, false]);
    const plain = document.createElement("nav"); document.body.replaceChildren(plain);
    new ConversationList().render(plain, rows, vi.fn());
    expect(plain.querySelector(".conversation-row.endable")).toBeNull();
  });

  it("lands focus on the next row, the one before, then the list once a session ends from the keyboard (cas-e634)", async () => {
    const container = document.createElement("nav"); container.id = "conversation-list"; document.body.replaceChildren(container);
    const list = new ConversationList();
    let live = [row("calm-puma-34", 300, { canEnd: true }), row("wild-shark-68", 200, { canEnd: true }), row("noble-cheetah-84", 100, { canEnd: true })];
    const open = vi.fn();
    // As main.ts does: the hub ends it, the catalog drops it, the list redraws.
    const end = vi.fn(async (ended: ConversationRow) => {
      await new Promise((ok) => setTimeout(ok, 0));
      live = live.filter((item) => item.key !== ended.key);
      list.render(container, groupConversationRows(live), open, end);
    });
    const draw = () => list.render(container, groupConversationRows(live), open, end);
    const endFromKeyboard = async (session: string) => {
      const control = [...container.querySelectorAll<HTMLElement>(".conversation-end")].find((node) => node.previousElementSibling?.textContent?.includes(session))!;
      control.querySelector<HTMLButtonElement>(".conversation-end-ask")!.click();
      const confirm = control.querySelector<HTMLButtonElement>(".conversation-end-confirm")!;
      confirm.focus();
      confirm.click();
      // While it ends, focus waits on the status line, not the page.
      expect(document.activeElement?.textContent).toBe(`Ending ${session}…`);
      await new Promise((ok) => setTimeout(ok, 5));
    };
    draw();
    await endFromKeyboard("wild-shark-68");
    expect((document.activeElement as HTMLElement).dataset.threadKey).toBe("atlas:noble-cheetah-84");
    await endFromKeyboard("noble-cheetah-84");
    // The last row ended: the one before it. One session left is no longer a group.
    expect((document.activeElement as HTMLElement).dataset.threadKey).toBe("atlas:calm-puma-34");
    expect(container.querySelector(".conversation-group-head")).toBeNull();
    expect(document.activeElement).not.toBe(document.body);
  });

  it("offers no End session without the callback or the scope", () => {
    const container = document.createElement("nav"); document.body.replaceChildren(container);
    new ConversationList().render(container, groupConversationRows([row("a-b-1", 2), row("c-d-2", 1)]), vi.fn(), vi.fn());
    expect(container.querySelector(".conversation-end")).toBeNull();
  });
});
