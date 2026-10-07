// @vitest-environment jsdom
// cas-55a4: a session's Commander thread is its own conversation. Other
// sessions' turns sit beside it, collapsed and labelled; a session that has
// not written yet says so and shows what it is doing; a project's live
// sessions are grouped with the most recent one marked.
import { describe, expect, it, vi } from "vitest";
import { ConversationHistory, sessionCodename } from "./conversation-history";
import { applyActionAvailability, conversationHeaderMarkup } from "./conversation-shell";
import { ConversationView, earlierSessionLabel, emptyActivityText, emptyCardActivityText, emptyThreadCopy } from "./conversation-view";
import { activityTime, ConversationList, conversationRowMarkup, ENDED_NOTICE_MS, groupConversationRows, machineActivityAt, plainActivity, type ConversationRow } from "./conversation-list";
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

  it("says a session has not written yet and shows its last activity", () => {
    const history = new ConversationHistory();
    history.currentSession = "gabber-studio-calm-puma-34";
    history.hydrateReply(said(2, "Old answer", "gabber-studio-noble-cheetah-84", at(29, 21, 41)));
    const now = Date.now();
    let activity: { at: number; label?: string; terminal?: boolean } = { at: now - 3 * 60_000, label: "supervisor → wild-shark-68" };
    const view = new ConversationView(document, history, { supervisor: "calm-puma-34", project: "gabber-studio", header: false, activity: () => activity });
    document.body.replaceChildren(view.element);
    view.update();
    const empty = view.element.querySelector<HTMLElement>(".empty")!;
    expect(empty.hidden).toBe(false);
    expect(empty.querySelector(".said")?.textContent).toBe("No messages from the gabber-studio supervisor in this session yet — nothing is waiting on you.");
    // cas-010f: no product codename and no queue jargon; one quiet line
    // holds the activity, and nothing offers a terminal (cas-0546).
    expect(empty.textContent).not.toContain("Commander");
    expect(empty.textContent).not.toContain("→");
    expect(empty.querySelector(".empty-activity")?.textContent).toBe("Last active 3m ago");
    expect(empty.querySelector(".empty-foot")?.textContent).toBe("Last active 3m ago");
    expect(empty.querySelector("button")).toBeNull();
    expect(empty.textContent).not.toMatch(/terminal view/i);
    // The supervisor's own terminal output, when that is newest.
    activity = { at: now - 60_000, label: "terminal output", terminal: true }; view.update();
    expect(empty.querySelector(".empty-activity")?.textContent).toBe("Terminal output 1m ago");
    // The other session's thread is not shown as this one's: it is the
    // collapsed section under the card.
    expect(view.element.querySelector('[role="log"]')!.textContent).not.toContain("Old answer");
    const section = view.element.querySelector<HTMLElement>("section.earlier-sessions")!;
    expect(section.hidden).toBe(false);
    expect(section.previousElementSibling).toBe(empty);
  });

  it("reads the header's connection state, and claims nothing before the first page resolves (cas-010f)", () => {
    const history = new ConversationHistory();
    let connection = "Live";
    let loading = true;
    const view = new ConversationView(document, history, { supervisor: "calm-puma-34", machine: "Atlas · Linux", project: "gabber-studio", header: false, connection: () => connection, loadingHistory: () => loading });
    document.body.replaceChildren(view.element);
    view.update();
    const empty = view.element.querySelector<HTMLElement>(".empty")!;
    // Live, page on its way: the loading line, never "No messages".
    expect(empty.dataset.state).toBe("loading");
    expect(empty.textContent).not.toContain("No messages");
    // Not live and the page cannot come: say why.
    connection = "Needs pairing"; view.update();
    expect(empty.dataset.state).toBe("waiting");
    expect(empty.querySelector(".said")?.textContent).toBe("Atlas · Linux needs pairing again before messages from the gabber-studio supervisor can load.");
    expect(empty.textContent).not.toContain("No messages");
    connection = "Live"; loading = false; view.update();
    expect(empty.dataset.state).toBe("empty");
    expect(empty.querySelector(".said")?.textContent).toBe("No messages from the gabber-studio supervisor in this session yet — nothing is waiting on you.");
    expect(empty.querySelector(".empty-terminal")).toBeNull();
    connection = "Reconnecting"; view.update();
    expect(empty.querySelector(".said")?.textContent).toBe("No messages from the gabber-studio supervisor in this session yet. Reconnecting to Atlas · Linux — anything new will show here once it's back.");
    connection = "Degraded"; view.update();
    expect(empty.querySelector(".said")?.textContent).toBe("No messages from the gabber-studio supervisor in this session yet. The connection is unsteady, so a new one may arrive late.");
  });

  it("words every connection state for the empty thread", () => {
    const base = { project: "cas-src", machine: "Atlas · Linux" };
    expect(emptyThreadCopy({ ...base, connection: "Live", resolved: true })).toEqual({ state: "empty", said: "No messages from the cas-src supervisor in this session yet — nothing is waiting on you." });
    expect(emptyThreadCopy({ ...base, connection: undefined, resolved: true }).state).toBe("empty");
    expect(emptyThreadCopy({ ...base, connection: "Needs pairing", resolved: true })).toEqual({ state: "empty", said: "No messages from the cas-src supervisor in this session yet. Atlas · Linux needs pairing again before new ones can arrive." });
    expect(emptyThreadCopy({ ...base, connection: "Unreachable · message pending", resolved: true }).said).toBe("No messages from the cas-src supervisor in this session yet. Atlas · Linux can't be reached — anything new will show here once it's back.");
    expect(emptyThreadCopy({ ...base, connection: "Can't reach · retrying", resolved: true }).said).toContain("Reconnecting to Atlas · Linux");
    for (const connection of ["Live", "Degraded", "Unsteady", "Connecting", "Idle", undefined]) expect(emptyThreadCopy({ ...base, connection, resolved: false }).state).toBe("loading");
    // cas-97d58 F10: the banner says "unsteady — checking…"; the thread agrees.
    expect(emptyThreadCopy({ ...base, connection: "Unsteady", resolved: true }).said).toBe("No messages from the cas-src supervisor in this session yet. The connection is unsteady, so a new one may arrive late.");
    expect(emptyThreadCopy({ ...base, connection: "Reconnecting", resolved: false })).toEqual({ state: "waiting", said: "Reconnecting to Atlas · Linux — messages from the cas-src supervisor will load once it's back." });
    expect(emptyThreadCopy({ ...base, connection: "Unreachable", resolved: false }).said).toBe("Atlas · Linux can't be reached — messages from the cas-src supervisor will load once it's back.");
    expect(emptyThreadCopy({ connection: "Needs pairing", resolved: false }).said).toBe("This machine needs pairing again before messages from this supervisor can load.");
    expect(emptyThreadCopy({ connection: "Reconnecting", resolved: true }).said).toContain("Reconnecting to this machine");
    // cas-d043 G04: a cause in this browser is never "can't be reached … once it's back".
    expect(emptyThreadCopy({ ...base, connection: "Browser can't connect", resolved: true }).said).toBe("No messages from the cas-src supervisor in this session yet. This browser can't connect to Atlas · Linux — update your browser, then reload this page to see anything new.");
    expect(emptyThreadCopy({ ...base, connection: "Browser can't connect", resolved: false })).toEqual({ state: "waiting", said: "This browser can't connect to Atlas · Linux, so messages from the cas-src supervisor can't load. Update your browser, then reload this page." });
    expect(emptyThreadCopy({ ...base, connection: "Blocked by browser", resolved: true }).said).toBe("No messages from the cas-src supervisor in this session yet. This browser is blocking its connection to Atlas · Linux — allow Local network access for this site to see anything new.");
    expect(emptyThreadCopy({ ...base, connection: "Blocked by browser", resolved: false }).said).toContain("This browser is blocking its connection to Atlas · Linux");
  });

  it("words the empty thread's activity plainly", () => {
    const now = new Date(2026, 8, 30, 12, 0).getTime();
    expect(emptyCardActivityText({ at: now - 2 * 60_000 }, now)).toBe("Last active 2m ago");
    expect(emptyCardActivityText({ at: now - 30_000, terminal: true }, now)).toBe("Terminal output just now");
    expect(emptyCardActivityText({}, now)).toBe("");
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
    expect(rows[0]!.group).toMatchObject({ first: true, size: 3, label: "gabber-studio · 3 conversations on Atlas" });
    expect(conversationRowMarkup(rows[0]!)).toContain('<span class="conversation-session-mark">Most recent</span>');
    expect(conversationRowMarkup(rows[1]!)).not.toContain("Most recent");
  });

  it("tells grouped rows apart before any is opened: what each session last did, in plain words (cas-5d2c)", () => {
    const rows = groupConversationRows([
      row("calm-puma-34", 300, { activityLine: "Messaged bright-robin-85" }),
      row("wild-shark-68", 200, { activityLine: "You wrote to it", preview: "Stem export is at 60%." }),
      row("noble-cheetah-84", 100),
    ]);
    const preview = (index: number) => new DOMParser().parseFromString(conversationRowMarkup(rows[index]!), "text/html").querySelector(".conversation-preview")?.textContent;
    // The catalog's activity until the thread has a turn; the turn once it has; the state when neither.
    expect([preview(0), preview(1), preview(2)]).toEqual(["Messaged bright-robin-85", "Stem export is at 60%.", "Live"]);
    // An ungrouped row keeps its connection words.
    expect(new DOMParser().parseFromString(conversationRowMarkup({ ...row("solo", 1), activityLine: "Wrote to you" }), "text/html").querySelector(".conversation-preview")?.textContent).toBe("Live");
  });

  it("words the catalog's activity label plainly", () => {
    expect(plainActivity("supervisor → bright-robin-85")).toBe("Messaged bright-robin-85");
    expect(plainActivity("supervisor → Commander")).toBe("Wrote to you");
    expect(plainActivity("Commander → supervisor")).toBe("You wrote to it");
    expect(plainActivity("daring-robin-43 → supervisor")).toBe("Heard from daring-robin-43");
    expect(plainActivity("lifecycle-wake → supervisor")).toBe("Woken up");
    expect(plainActivity("supervisor → supervisor")).toBe("Typed at its terminal");
    expect(plainActivity(undefined)).toBeUndefined();
    expect(plainActivity("something else")).toBe("something else");
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
    expect(container.querySelector(".conversation-group-head")?.textContent).toBe("gabber-studio · 2 conversations on Atlas");
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

  it("keeps a failed End session actionable and focused without dropping its row (cas-a549)", async () => {
    const container = document.createElement("nav"); document.body.replaceChildren(container);
    const list = new ConversationList();
    const rows = groupConversationRows([row("calm-puma-34", 300, { canEnd: true }), row("noble-cheetah-84", 100, { canEnd: true })]);
    const end = vi.fn(async () => { throw new Error("DELETE /v1/sessions/noble-cheetah-84 failed (500)"); });
    const open = vi.fn();
    list.render(container, rows, open, end);
    const keptRow = container.querySelectorAll(".conversation-row")[1];
    const control = container.querySelectorAll<HTMLElement>(".conversation-end")[1]!;
    control.querySelector<HTMLButtonElement>(".conversation-end-ask")!.click();
    const confirm = control.querySelector<HTMLButtonElement>(".conversation-end-confirm")!;
    confirm.focus(); confirm.click();
    await Promise.resolve();
    const retry = control.querySelector<HTMLButtonElement>(".conversation-end-ask")!;
    const error = control.querySelector<HTMLElement>(".conversation-end-error")!;
    expect(error.textContent).toBe("Could not end noble-cheetah-84 on Atlas. Try End session again. If it still fails, check the session on Atlas.");
    // cas-9ae6: focus is back on End session, which reads the failure as its
    // description; the line is not also an alert, so it is said once.
    expect(error.hasAttribute("role")).toBe(false);
    expect(document.activeElement).toBe(retry);
    expect(retry.getAttribute("aria-describedby")).toBe(error.id);
    expect(error.id).not.toBe("");
    expect(container.querySelectorAll(".conversation-row")).toHaveLength(2);
    expect(container.querySelectorAll(".conversation-row")[1]).toBe(keptRow);
    // A catalog heartbeat keeps the actionable error and keyboard position.
    list.render(container, rows, open, end);
    expect(document.activeElement).toBe(retry);
    expect(container.querySelector(".conversation-ended")).toBeNull();
    // Retrying still asks for confirmation; Cancel does not send another request.
    retry.click();
    expect(control.dataset.state).toBe("confirm");
    expect(document.activeElement).toBe(control.querySelector(".conversation-end-cancel"));
    control.querySelector<HTMLButtonElement>(".conversation-end-cancel")!.click();
    expect(end).toHaveBeenCalledOnce();
  });

  it("does not steal focus if the operator leaves while End session is pending (cas-a549)", async () => {
    const container = document.createElement("nav");
    const elsewhere = document.createElement("button"); elsewhere.textContent = "Another action";
    document.body.replaceChildren(container, elsewhere);
    let reject!: (error: Error) => void;
    const end = vi.fn(() => new Promise<void>((_resolve, fail) => { reject = fail; }));
    new ConversationList().render(container, [row("calm-puma-34", 300, { canEnd: true })], vi.fn(), end);
    container.querySelector<HTMLButtonElement>(".conversation-end-ask")!.click();
    const confirm = container.querySelector<HTMLButtonElement>(".conversation-end-confirm")!;
    confirm.focus(); confirm.click();
    elsewhere.focus();
    reject(new Error("request failed (500)"));
    await Promise.resolve();
    expect(document.activeElement).toBe(elsewhere);
    expect(container.querySelector(".conversation-end-error")?.textContent).toContain("Try End session again.");
    // Focus stayed elsewhere, so nothing reads the description: the failure is an alert (cas-9ae6).
    expect(container.querySelector(".conversation-end-error")?.getAttribute("role")).toBe("alert");
    expect(container.querySelectorAll(".conversation-row")).toHaveLength(1);
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

  it("dates a machine-stamped activity in this browser's time when the machine clock runs ahead (cas-24fe)", () => {
    const now = new Date(2026, 9, 1, 12, 0).getTime();
    const AHEAD = 5 * 60_000;
    // With the lead measured, the stamp less the lead: three minutes ago reads 3m, not "now".
    expect(machineActivityAt(now - 3 * 60_000 + AHEAD, AHEAD, undefined, now)).toEqual({ at: now - 3 * 60_000 });
    expect(activityTime(machineActivityAt(now - 3 * 60_000 + AHEAD, AHEAD, undefined, now).at, now).short).toBe("3m");
    // A lead never puts the activity after now.
    expect(machineActivityAt(now + AHEAD + 60_000, AHEAD, undefined, now)).toEqual({ at: now });
    // A stamp in the past with no lead known is its own time.
    expect(machineActivityAt(now - 60_000, undefined, undefined, now)).toEqual({ at: now - 60_000 });
    // With no lead known, a future stamp dates from when this page first saw it, so the row ages.
    const first = machineActivityAt(now + 2 * 60_000, undefined, undefined, now);
    expect(first).toEqual({ at: now, seen: { stamp: now + 2 * 60_000, seen: now } });
    const fiveLater = machineActivityAt(now + 2 * 60_000, undefined, first.seen, now + 5 * 60_000);
    expect(activityTime(fiveLater.at, now + 5 * 60_000).short).toBe("5m");
    // A newer stamp is new activity.
    expect(machineActivityAt(now + 9 * 60_000, undefined, first.seen, now + 5 * 60_000)).toEqual({ at: now + 5 * 60_000, seen: { stamp: now + 9 * 60_000, seen: now + 5 * 60_000 } });
  });

  it("never ends a session on the second click of a double-click: Cancel sits first, and neither button takes it (cas-f60a)", () => {
    const container = document.createElement("nav"); document.body.replaceChildren(container);
    const end = vi.fn(async (_row: ConversationRow) => {});
    new ConversationList().render(container, groupConversationRows([row("calm-puma-34", 300, { canEnd: true }), row("noble-cheetah-84", 100, { canEnd: true })]), vi.fn(), end);
    const control = container.querySelectorAll<HTMLElement>(".conversation-end")[1]!;
    const click = (node: Element, detail: number) => node.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, detail }));
    click(control.querySelector(".conversation-end-ask")!, 1);
    expect([...control.querySelectorAll("button")].map((node) => node.textContent)).toEqual(["Cancel", "End session"]);
    // The double-click's second press keeps focus on Cancel and selects nothing.
    const press = new MouseEvent("mousedown", { bubbles: true, cancelable: true, detail: 2 });
    control.querySelector(".conversation-end-question")!.dispatchEvent(press);
    expect(press.defaultPrevented).toBe(true);
    // The double-click's second click, wherever it lands, does nothing.
    click(control.querySelector(".conversation-end-confirm")!, 2);
    click(control.querySelector(".conversation-end-cancel")!, 2);
    click(control.querySelector(".conversation-end-confirm")!, 3);
    expect(end).not.toHaveBeenCalled();
    expect(control.dataset.state).toBe("confirm");
    // A deliberate click, or a key (detail 0), ends it.
    click(control.querySelector(".conversation-end-confirm")!, 0);
    expect(end).toHaveBeenCalledOnce();
  });

  it("says a session ended where its row was and in a polite live region, then lets it go (cas-f60a)", async () => {
    vi.useFakeTimers();
    try {
      const sidebar = document.createElement("aside");
      const container = document.createElement("nav"); container.id = "conversation-list";
      sidebar.append(container, Object.assign(document.createElement("footer"), { textContent: "2 paired machines" }));
      document.body.replaceChildren(sidebar);
      const list = new ConversationList();
      let live = [row("calm-puma-34", 300, { canEnd: true }), row("wild-shark-68", 200, { canEnd: true }), row("noble-cheetah-84", 100, { canEnd: true })];
      const open = vi.fn();
      const end = vi.fn(async (ended: ConversationRow) => {
        live = live.filter((item) => item.key !== ended.key);
        list.render(container, groupConversationRows(live), open, end);
      });
      list.render(container, groupConversationRows(live), open, end);
      // The live region is beside the list from the first render, empty, so its first sentence is announced.
      const status = container.nextElementSibling as HTMLElement;
      expect(status.getAttribute("role")).toBe("status");
      expect(status.classList.contains("sr-only")).toBe(true);
      expect(status.textContent).toBe("");
      const control = [...container.querySelectorAll<HTMLElement>(".conversation-end")].find((node) => node.previousElementSibling?.textContent?.includes("wild-shark-68"))!;
      control.querySelector<HTMLButtonElement>(".conversation-end-ask")!.click();
      control.querySelector<HTMLButtonElement>(".conversation-end-confirm")!.click();
      await vi.advanceTimersByTimeAsync(0);
      expect(status.textContent).toBe("wild-shark-68 on Atlas ended.");
      expect([...container.children].map((node) => node.className.split(" ")[0])).toEqual(["conversation-group-head", "conversation-row", "conversation-end", "conversation-ended", "conversation-row", "conversation-end"]);
      expect(container.querySelector(".conversation-ended")?.textContent).toBe("wild-shark-68 on Atlas ended.");
      // A catalog poll keeps it in place.
      list.render(container, groupConversationRows(live), open, end);
      expect(container.querySelector(".conversation-ended")?.previousElementSibling?.previousElementSibling?.textContent).toContain("calm-puma-34");
      await vi.advanceTimersByTimeAsync(ENDED_NOTICE_MS);
      expect(container.querySelector(".conversation-ended")).toBeNull();
      expect(status.textContent).toBe("");
      expect(container.nextElementSibling).toBe(status);
    } finally {
      vi.useRealTimers();
    }
  });

  it("times a row from its own activity only: now under a minute, then words for assistive tech (cas-6acf)", () => {
    const now = new Date(2026, 9, 1, 12, 0).getTime();
    expect(activityTime(now - 20_000, now)).toEqual({ short: "now", spoken: "just now" });
    expect(activityTime(now - 59_999, now)).toEqual({ short: "now", spoken: "just now" });
    expect(activityTime(now - 60_000, now)).toEqual({ short: "1m", spoken: "1 minute ago" });
    expect(activityTime(now - 20 * 60_000, now)).toEqual({ short: "20m", spoken: "20 minutes ago" });
    expect(activityTime(now - 3 * 3_600_000, now)).toEqual({ short: "3h", spoken: "3 hours ago" });
    expect(activityTime(now - 2 * 86_400_000, now)).toEqual({ short: "2d", spoken: "2 days ago" });
    // A machine clock ahead never reads as the future.
    expect(activityTime(now + 30_000, now).short).toBe("now");
    const markup = conversationRowMarkup(row("calm-puma-34", 300, { when: "20m", whenSpoken: "20 minutes ago", freshness: "Last activity 20m ago" }));
    expect(markup).toContain('<span class="conversation-when" title="Last activity 20m ago" aria-hidden="true">20m</span>');
    expect(markup).toContain('<span class="sr-only">, 20 minutes ago</span>');
    // No activity, no time: never a catalog-check "now".
    expect(conversationRowMarkup(row("idle-otter-1"))).not.toContain("conversation-when");
  });

  it("marks the newest-started session Most recent when none has activity (cas-6acf)", () => {
    const rows = groupConversationRows([row("old-owl-1", undefined, { startedAt: 100 }), row("new-newt-2", undefined, { startedAt: 300 }), row("mid-mole-3", undefined, { startedAt: 200 })]);
    expect(rows.map((item) => [item.session, item.group?.active])).toEqual([["new-newt-2", true], ["mid-mole-3", false], ["old-owl-1", false]]);
    // Any activity outranks start times.
    const active = groupConversationRows([row("old-owl-1", 50, { startedAt: 100 }), row("new-newt-2", undefined, { startedAt: 300 })]);
    expect(active[0]).toMatchObject({ session: "old-owl-1", group: { active: true } });
    // Neither activity nor start: nothing is claimed.
    expect(groupConversationRows([row("a-b-1"), row("c-d-2")]).some((item) => item.group?.active)).toBe(false);
  });

  it("offers no End session without the callback or the scope", () => {
    const container = document.createElement("nav"); document.body.replaceChildren(container);
    new ConversationList().render(container, groupConversationRows([row("a-b-1", 2), row("c-d-2", 1)]), vi.fn(), vi.fn());
    expect(container.querySelector(".conversation-end")).toBeNull();
  });
});

describe("cas-0546: the header's Interrupt and Raw output say why they can't run", () => {
  it("marks an action unavailable with a spoken reason, keeps it focusable, and restores it", () => {
    document.body.innerHTML = conversationHeaderMarkup({ supervisor: "patient-pelican-9", projectDir: "/projects/cas-src", host: "Atlas · Linux", selected: true, loaded: true, paired: true });
    const button = document.querySelector<HTMLButtonElement>("#conversation-interrupt")!;
    const note = document.querySelector<HTMLElement>("#conversation-interrupt-reason")!;
    const reason = "Lost connection to Atlas · Linux. Interrupt and raw output return when it reconnects.";
    applyActionAvailability(button, note, reason);
    applyActionAvailability(button, note, reason);
    expect(button.getAttribute("aria-disabled")).toBe("true");
    expect(button.disabled).toBe(false);
    expect(button.dataset.disabledReason).toBe(reason);
    expect(button.title).toBe(reason);
    expect(document.getElementById(button.getAttribute("aria-describedby")!)?.textContent).toBe(reason);
    // A hidden description node: named by aria-describedby, never read twice as page text.
    expect(note.hidden).toBe(true);
    expect(button.getAttribute("aria-label")).toBe("Interrupt the cas-src supervisor");
    expect(button.textContent).toBe("Interrupt");
    applyActionAvailability(button, note, undefined);
    expect(button.hasAttribute("aria-disabled")).toBe(false);
    expect(button.hasAttribute("aria-describedby")).toBe(false);
    expect(button.hasAttribute("title")).toBe(false);
    expect(button.dataset.disabledReason).toBeUndefined();
    expect(note.textContent).toBe("");
  });

  it("offers no Terminal view anywhere in the header", () => {
    document.body.innerHTML = conversationHeaderMarkup({ supervisor: "patient-pelican-9", projectDir: "/projects/cas-src", host: "Atlas · Linux", selected: true, loaded: true, paired: true });
    expect(document.querySelector("#conversation-terminal")).toBeNull();
    expect(document.body.textContent).not.toMatch(/terminal/i);
    const raw = document.querySelector<HTMLButtonElement>("#conversation-raw-output")!;
    expect(raw.getAttribute("aria-label")).toBe("Raw output");
    expect(raw.getAttribute("aria-haspopup")).toBe("dialog");
  });
});
