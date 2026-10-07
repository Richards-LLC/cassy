import { describe, expect, it } from "vitest";
import { ConversationHistory, type ConversationEvent } from "./conversation-history";
import { blockerEvidence, cellTone, clockLabel, coalesceText, dayLabel, foldsAsStatus, messageBlocks, shownTimes, stampLabel, STATUS_FOLD_LIMIT, statusAsks, threadModel, type ThreadGroup } from "./thread-model";
import ROW_20812 from "./fixtures/hub-row-20812.txt?raw";
import type { OperatorReply, OperatorTurnKind } from "./types";

const NOW = new Date(2026, 8, 21, 10, 0).getTime();
const at = (hh: number, mm: number, dayOffset = 0) => new Date(2026, 8, 21 + dayOffset, hh, mm).getTime();
function reply(id: number, kind: OperatorTurnKind, message = `m${id}`, reply_to: number | null = null): OperatorReply {
  return { notification_id: id, reply_to, message, summary: "", device_id: "d", kind };
}

describe("threadModel", () => {
  it("dates any turn time that is not today's (cas-e829)", () => {
    expect(stampLabel(at(9, 41), NOW)).toBe("09:41");
    expect(stampLabel(at(17, 20, -1), NOW)).toBe("Sep 20, 17:20");
    expect(stampLabel(new Date(2025, 11, 31, 23, 5).getTime(), NOW)).toBe("Dec 31 2025, 23:05");
    expect(stampLabel(undefined, NOW)).toBeUndefined();
    const history = new ConversationHistory();
    history.reply(reply(1, "blocker", "Supervisor missed an update"), at(17, 20, -1));
    history.reply(reply(2, "answer"), at(9, 0));
    const times = threadModel(history.events, { now: NOW }).flatMap((item) => item.type === "group" ? [item.time] : []);
    expect(times).toEqual(["Sep 20, 17:20", "09:00"]);
  });
  it("draws no session line when the thread's own session opens it, and one when another turn precedes it (cas-55a4)", () => {
    const own = new ConversationHistory();
    own.reply(reply(1, "answer"), at(9, 0), "live");
    own.reply(reply(2, "answer"), at(9, 1), "live");
    expect(threadModel(own.events, { now: NOW, session: "live" }).map((item) => item.type)).toEqual(["day", "group"]);
    expect(threadModel(own.events, { now: NOW }).map((item) => item.type)).toEqual(["day", "session", "group"]);
    // Another session's turn before the thread's own opens a line for it.
    const mixed = new ConversationHistory();
    mixed.reply(reply(1, "blocker"), at(9, 0), "ended");
    mixed.reply(reply(2, "answer"), at(9, 1), "live");
    expect(threadModel(mixed.events, { now: NOW, session: "live" }).map((item) => item.type)).toEqual(["day", "session", "group", "session", "group"]);
  });
  it("files a turn with no session recorded under the thread's own session, with no line below it (cas-8d52, journey F13)", () => {
    const history = new ConversationHistory();
    history.reply(reply(1, "blocker"), at(9, 0));
    history.reply(reply(2, "answer"), at(9, 1), "live");
    expect(threadModel(history.events, { now: NOW, session: "live" }).map((item) => item.type)).toEqual(["day", "group"]);
  });
  it("groups consecutive turns from one side and marks only the outer corners", () => {
    const history = new ConversationHistory();
    history.submit("a", "sup", "Merge the lanes.", at(9, 41));
    history.reply(reply(1, "answer"), at(9, 44));
    history.reply(reply(2, "receipt"), at(9, 47));
    history.submit("b", "sup", "Did it move?", at(9, 56));
    const items = threadModel(history.events, { now: NOW });
    expect(items.map((item) => item.type)).toEqual(["day", "group", "group", "group"]);
    const [, you, sup, again] = items as [unknown, ThreadGroup, ThreadGroup, ThreadGroup];
    expect(you.side).toBe("you"); expect(you.turns).toHaveLength(1); expect(you.turns[0]).toMatchObject({ first: true, last: true });
    expect(sup.side).toBe("supervisor"); expect(sup.turns.map((turn) => [turn.kind, turn.first, turn.last])).toEqual([["answer", true, false], ["receipt", false, true]]);
    expect(again.side).toBe("you");
  });
  it("stamps one timestamp per group, taken from the group's latest turn", () => {
    const history = new ConversationHistory();
    history.reply(reply(1, "answer"), at(9, 44));
    history.reply(reply(2, "receipt"), at(9, 47));
    const [, group] = threadModel(history.events, { now: NOW }) as [unknown, ThreadGroup];
    expect(group.time).toBe("09:47");
  });
  it("coalesces consecutive status turns into one line that counts them and shows the latest", () => {
    const history = new ConversationHistory();
    history.reply(reply(1, "answer"), at(9, 47));
    history.reply(reply(2, "status", "Gate started"), at(9, 49));
    history.reply(reply(3, "status", "4 of 14 green"), at(9, 51));
    history.reply(reply(4, "status", "gate 11 of 14 targets green"), at(9, 55));
    history.reply(reply(5, "answer", "No — unchanged."), at(9, 57));
    const items = threadModel(history.events, { now: NOW });
    expect(items.map((item) => item.type)).toEqual(["day", "group", "coalesce", "group"]);
    const line = items[2];
    if (line.type !== "coalesce") throw new Error("expected coalesce");
    expect(line.count).toBe(3); expect(line.latest).toBe("gate 11 of 14 targets green"); expect(line.time).toBe("09:55");
    expect(coalesceText(line)).toBe("2 more updates · gate 11 of 14 targets green");
    // A status run splits the supervisor turns around it into separate groups.
    const after = items[3] as ThreadGroup; expect(after.turns[0]).toMatchObject({ first: true, last: true });
  });
  it("orders by time, not arrival: a turn stamped inside a status run splits it, one stamped after does not (cas-7294)", () => {
    const run = (sheetAt: number) => {
      const history = new ConversationHistory();
      history.reply(reply(1, "status", "Gate started"), at(9, 49));
      history.reply(reply(2, "status", "Gate 4 of 14"), at(9, 51));
      history.reply(reply(3, "status", "Gate 8 of 14"), at(9, 53));
      history.reply(reply(4, "status", "gate 11 of 14 targets green"), at(9, 55));
      history.reply(reply(5, "answer", ""), sheetAt); // arrives last
      return threadModel(history.events, { now: NOW }).filter((item) => item.type === "coalesce").map((item) => item.type === "coalesce" ? item.count : 0);
    };
    expect(run(at(9, 52))).toEqual([2, 2]);
    expect(run(at(9, 55))).toEqual([4]);
  });
  it("renders a lone status as its own quiet text without a count", () => {
    const history = new ConversationHistory();
    history.reply(reply(1, "status", "Rebasing"), at(9, 49));
    const [, line] = threadModel(history.events, { now: NOW });
    if (line?.type !== "coalesce") throw new Error("expected coalesce");
    expect(coalesceText(line)).toBe("Rebasing");
  });
  it("does not fold the operator's real row 20812: a long waiting-on-you status reads as a supervisor turn", () => {
    const history = new ConversationHistory();
    history.submit("d", "sup", "Where are we on the QA pilot? What do you need from me?", at(13, 18));
    history.reply(reply(20811, "status", "Gate 11 of 14 targets green"), at(13, 20));
    history.reply(reply(20812, "status", ROW_20812.trim()), at(13, 24));
    const items = threadModel(history.events, { now: NOW });
    expect(items.map((item) => item.type)).toEqual(["day", "group", "coalesce", "group"]);
    const line = items[2]; if (line?.type !== "coalesce") throw new Error("expected coalesce");
    // The short status keeps its quiet line on its own; the long one no longer hides it behind "1 more update ·".
    expect(line.count).toBe(1); expect(coalesceText(line)).toBe("Gate 11 of 14 targets green");
    const turn = (items[3] as ThreadGroup).turns[0]!;
    expect((items[3] as ThreadGroup).side).toBe("supervisor");
    expect(turn.kind).toBe("status");
    expect(turn.event.kind === "reply" && turn.event.value.message).toBe(ROW_20812.trim());
    expect(ROW_20812.trim().length).toBeGreaterThan(STATUS_FOLD_LIMIT);
    expect(statusAsks(ROW_20812)).toBe(true);
  });
  it("routes by length at the fold limit and by ask regardless of length", () => {
    expect(foldsAsStatus("x".repeat(STATUS_FOLD_LIMIT))).toBe(true);
    expect(foldsAsStatus("x".repeat(STATUS_FOLD_LIMIT + 1))).toBe(false);
    // Surrounding whitespace does not count toward the limit.
    expect(foldsAsStatus(`  ${"x".repeat(STATUS_FOLD_LIMIT)}\n`)).toBe(true);
    for (const ask of ["WAITING ON YOU: say post", "Need your call on the lane split", "Ship it or hold?", "Gate green — over to you", "Needs your approval to merge"]) {
      expect(foldsAsStatus(ask), ask).toBe(false);
    }
    for (const tick of ["Gate 11 of 14 targets green", "Rebasing", "fetching https://example.test/run?id=4 · 3 of 5"]) {
      expect(foldsAsStatus(tick), tick).toBe(true);
    }
  });
  it("an ask-bearing status splits a status run and joins the supervisor turns beside it", () => {
    const history = new ConversationHistory();
    history.reply(reply(1, "status", "Gate started"), at(9, 49));
    history.reply(reply(2, "status", "Gate red on macOS — rerun or skip?"), at(9, 50));
    history.reply(reply(3, "answer", "Details in the log."), at(9, 51));
    history.reply(reply(4, "status", "Rerunning"), at(9, 52));
    const items = threadModel(history.events, { now: NOW });
    expect(items.map((item) => item.type)).toEqual(["day", "coalesce", "group", "coalesce"]);
    expect((items[2] as ThreadGroup).turns.map((turn) => turn.kind)).toEqual(["status", "answer"]);
  });
  it("leaves the working line out while the newest message still says Sending… (cas-71f4, journey F20)", () => {
    const history = new ConversationHistory();
    history.reply(reply(1, "answer", "Ready."), at(9, 0));
    history.submit("b", "sup", "Ship it", at(9, 1));
    // One sending signal: the bubble's own "Sending…".
    expect(threadModel(history.events, { now: NOW, working: true }).some((item) => item.type === "working")).toBe(false);
    // Once the supervisor has it, the working line says it is on it.
    history.acknowledge({ client_ref: "b", notification_id: 7, target: "sup", stamped: true });
    expect(threadModel(history.events, { now: NOW, working: true }).at(-1)?.type).toBe("working");
  });
  it("adds day separators, an undated Today, and the working line only when executing", () => {
    const history = new ConversationHistory();
    history.submit("a", "sup", "Yesterday's ask", at(18, 0, -1));
    history.reply(reply(1, "answer"), at(9, 0));
    const items = threadModel(history.events, { now: NOW, working: true });
    expect(items.map((item) => item.type)).toEqual(["day", "group", "day", "group", "working"]);
    expect((items[0] as { label: string }).label).toBe("Yesterday"); expect((items[2] as { label: string }).label).toBe("Today");
    expect(threadModel(history.events, { now: NOW }).some((item) => item.type === "working")).toBe(false);
    const undated = new ConversationHistory(); undated.events.push({ kind: "reply", value: reply(9, "answer") });
    expect(threadModel(undated.events, { now: NOW })[0]).toMatchObject({ type: "day", label: "Today" });
    expect(dayLabel(at(9, 0, -9), NOW)).not.toMatch(/Today|Yesterday/);
  });
  it("marks project history session boundaries and its explicit beginning", () => {
    const history = new ConversationHistory();
    history.submit("older", "sup", "Older question", at(9, 0, -1), undefined, "factory-older");
    history.reply(reply(1, "answer", "Older answer"), at(9, 2, -1), "factory-older");
    history.submit("newer", "sup", "Newer question", at(9, 0), undefined, "factory-newer");
    const items = threadModel(history.events, { now: NOW, historyEnd: true });
    expect(items.map((item) => item.type)).toEqual(["history-end", "day", "session", "group", "group", "day", "session", "group"]);
    expect(items.filter((item) => item.type === "session").map((item) => item.label)).toEqual([
      "session factory-older started 09:00",
      "session factory-newer started 09:00",
    ]);
  });
  it("keeps an empty thread in the empty state when the loaded page ends history", () => {
    expect(threadModel([], { historyEnd: true, working: true })).toEqual([]);
  });
  it("keeps ask and blocker as turns with their kind for the render hook", () => {
    const history = new ConversationHistory();
    history.reply(reply(1, "ask"), at(9, 58)); history.reply(reply(2, "blocker"), at(9, 59));
    const [, group] = threadModel(history.events, { now: NOW }) as [unknown, ThreadGroup];
    expect(group.turns.map((turn) => turn.kind)).toEqual(["ask", "blocker"]);
  });
});

describe("messageBlocks", () => {
  it("splits prose from a markdown table and reads the header rule", () => {
    const blocks = messageBlocks("Yes — pass two is green. Every pack:\n\n| pack | cases | result |\n| --- | ---: | --- |\n| core | 412 | pass |\n| cli | 318 | 1 flake |\n\nTagged and pushed.");
    expect(blocks).toEqual([
      { type: "text", text: "Yes — pass two is green. Every pack:" },
      { type: "table", table: { header: ["pack", "cases", "result"], rows: [["core", "412", "pass"], ["cli", "318", "1 flake"]] } },
      { type: "text", text: "Tagged and pushed." },
    ]);
  });
  it("treats a table without a rule as body rows and leaves plain prose alone", () => {
    expect(messageBlocks("| a | b |\n| c | d |")).toEqual([{ type: "table", table: { header: undefined, rows: [["a", "b"], ["c", "d"]] } }]);
    expect(messageBlocks("just | a pipe in prose")).toEqual([{ type: "text", text: "just | a pipe in prose" }]);
  });
  it("tones result cells", () => {
    expect(cellTone("pass")).toBe("pass"); expect(cellTone("1 flake")).toBe("flake"); expect(cellTone("failed")).toBe("fail"); expect(cellTone("412")).toBeUndefined();
  });
});

describe("blockerEvidence", () => {
  it("lifts a trailing file:line · label line into the evidence window", () => {
    expect(blockerEvidence("The release gate went red. The train is held.\nattention.rs:212 · needless_borrow"))
      .toEqual({ text: "The release gate went red. The train is held.", evidence: "attention.rs:212 · needless_borrow" });
    expect(blockerEvidence("Gate red.\n\ncas-cli/src/hub/server.rs:883:5")).toEqual({ text: "Gate red.", evidence: "cas-cli/src/hub/server.rs:883:5" });
    expect(blockerEvidence("Gate red.\n`cargo clippy -- -D warnings`")).toEqual({ text: "Gate red.", evidence: "cargo clippy -- -D warnings" });
  });
  it("leaves prose alone: no evidence without a reference line, and a lone line is the message", () => {
    expect(blockerEvidence("The release gate went red.\nNothing was tagged.")).toEqual({ text: "The release gate went red.\nNothing was tagged.", evidence: undefined });
    expect(blockerEvidence("attention.rs:212 · needless_borrow")).toEqual({ text: "attention.rs:212 · needless_borrow", evidence: undefined });
  });
});

describe("clock-skewed live turns show their own time (cas-ac1f)", () => {
  const blocker = (id: number, at: number) => ({ notification_id: id, reply_to: null, message: "Gate red.", summary: "", device_id: "d", kind: "blocker" as const, attachments: [], at: new Date(at).toISOString() });
  it("a send after a turn from a clock 5 minutes ahead sorts after it and shows the browser's time", () => {
    const now = new Date(2026, 8, 24, 12, 0).getTime();
    const history = new ConversationHistory();
    history.hydrateReply(blocker(1, now + 300_000), now);
    history.submit("s", "sup", "On it", now + 1_000);
    expect(history.events.map((event) => event.kind)).toEqual(["reply", "send"]);
    expect(history.events.at(-1)).toMatchObject({ kind: "send", at: now + 300_000, shownAt: now + 1_000 });
    const groups = threadModel(history.events, { now: now + 1_000 }).filter((item): item is ThreadGroup => item.type === "group");
    expect(groups.map((group) => [group.side, group.time, group.clockAhead])).toEqual([["supervisor", "12:00", true], ["you", "12:00", false]]);
  });
  it("a send stamped before a live turn keyed ahead of it still sorts after and shows its own time", () => {
    // A browser clock that stepped back: the latest turn's key is ahead of the new send.
    const now = new Date(2026, 8, 24, 12, 0).getTime();
    const history = new ConversationHistory();
    history.receive({ notification_id: 1, reply_to: null, message: "Ack.", summary: "", device_id: "d", kind: "answer" }, now + 300_000);
    history.submit("s", "sup", "On it", now);
    expect(history.events.at(-1)).toMatchObject({ kind: "send", at: now + 300_000, shownAt: now });
    const groups = threadModel(history.events, { now: now + 300_000 }).filter((item) => item.type === "group");
    expect(groups.at(-1)).toMatchObject({ side: "you", time: "12:00" });
  });
});

describe("thread order is stable under a machine clock ahead, a reconnect and a reload (cas-1f13)", () => {
  const now = new Date(2026, 8, 24, 13, 36).getTime();
  const durable = (id: number, at: number, extra: Partial<OperatorReply> & { session?: string } = {}) => ({ notification_id: id, reply_to: null, message: `m${id}`, summary: "", device_id: "d", kind: "answer" as const, attachments: [], at: new Date(at).toISOString(), ...extra });
  const message = (id: number, text: string, at: number, extra: { reply_to?: number; session?: string } = {}) => ({ notification_id: id, target: "sup", text, state: "acknowledged" as const, stamped: true, device_id: "d", at: new Date(at).toISOString(), ...extra });
  const order = (history: ConversationHistory) => history.events.map((event) => event.kind === "send" ? `send:${event.value.notificationId ?? event.value.id}` : `reply:${event.value.notification_id}`);
  const days = (history: ConversationHistory, clock: number) => threadModel(history.events, { now: clock }).filter((item) => item.type === "day").map((item) => item.type === "day" ? item.label : "");
  const groups = (history: ConversationHistory, clock: number) => threadModel(history.events, { now: clock }).filter((item): item is ThreadGroup => item.type === "group").map((group) => `${group.side} ${group.time}${group.clockAhead ? " ahead" : ""}`);
  it("a day-ahead machine never puts a future day header above Today", () => {
    const history = new ConversationHistory();
    history.hydrateReply(durable(1, now - 3_600_000), now);
    history.hydrateReply(durable(2, now + 86_400_000), now);
    history.submit("s", "sup", "On it", now + 1_000);
    history.receive(reply(3, "answer", "Ack."), now + 60_000);
    expect(days(history, now + 60_000)).toEqual(["Today"]);
    // The future stamp shows its arrival and says the clock is ahead; the
    // live answer from the same machine says so too (F02).
    // cas-d043 H14: the 12:36 answer keeps its own time, not merged under
    // the later arrival and its clock-ahead flag.
    expect(groups(history, now + 60_000)).toEqual(["supervisor 12:36", "supervisor 13:36 ahead", "you 13:36", "supervisor 13:37 ahead"]);
  });
  it("orders a 13:41 blocker from a clock 5 minutes ahead before the 13:36 session start and shows it at 13:36", () => {
    const history = new ConversationHistory();
    history.hydrateReply(durable(900, now + 300_000, { kind: "blocker", message: "The release gate went red." }), now);
    history.receive(reply(901, "ask", "Fix or ship?"), now + 5_000, "patient-pelican-9");
    const items = threadModel(history.events, { now: now + 5_000 });
    const shown = items.map((item) => item.type === "group" ? `${item.side} ${item.time}${item.clockAhead ? " ahead" : ""}` : item.type === "session" ? item.label : item.type === "day" ? item.label : item.type);
    expect(shown).toEqual(["Today", "supervisor 13:36 ahead", "session patient-pelican-9 started 13:36", "supervisor 13:36 ahead"]);
  });
  it("places durable turns in the machine's sequence whatever their stamps and the hydration order", () => {
    const history = new ConversationHistory();
    history.hydrateReply(durable(3, now + 240_000), now);
    history.hydrateSend(message(2, "q", now + 120_000), now);
    history.hydrateReply(durable(1, now + 60_000), now);
    history.hydrateReply(durable(0, now - 60_000), now);
    expect(order(history)).toEqual(["reply:0", "reply:1", "send:2", "reply:3"]);
    // A few seconds of skew changes nothing on screen, so it earns no hint.
    const slight = new ConversationHistory();
    slight.hydrateReply(durable(5, now + 2_000), now);
    expect(groups(slight, now)).toEqual(["supervisor 13:36"]);
  });
  it("a reload rebuilds the order the visit saw: a day-ahead turn that came first stays above my message and its answer (review F01)", () => {
    const session = "patient-pelican-9";
    // The visit: history, then my message and its answer arrive live.
    const visit = new ConversationHistory();
    visit.hydrateReply(durable(800, now - 600_000, { kind: "status", message: "Earlier: tests are green." }), now);
    visit.hydrateReply(durable(801, now + 86_400_000, { message: "Tomorrow-stamped: the deploy is queued." }), now);
    visit.submit("c", "sup", "Ship it after the gate.", now + 60_000, undefined, session);
    visit.acknowledge({ client_ref: "c", notification_id: 1000, target: "sup", stamped: true });
    visit.receive(reply(1001, "answer", "On it."), now + 90_000, session);
    // The reload: the same turns, all from history, with the machine's stamps, messages before replies.
    const later = now + 600_000;
    const reload = new ConversationHistory();
    reload.hydrateSend(message(1000, "Ship it after the gate.", now + 60_000, { session }), later);
    for (const row of [durable(800, now - 600_000, { kind: "status", message: "Earlier: tests are green." }), durable(801, now + 86_400_000, { message: "Tomorrow-stamped: the deploy is queued." }), durable(1001, now + 90_000, { message: "On it.", session })]) reload.hydrateReply(row, later);
    expect(order(reload)).toEqual(["reply:800", "reply:801", "send:1000", "reply:1001"]);
    expect(order(visit)).toEqual(["reply:800", "reply:801", "send:1000", "reply:1001"]);
    expect(reload.preview()).toBe("On it.");
    expect(days(reload, later)).toEqual(["Today"]);
    // Times read in order: the day-ahead turn shows no later than my message below it.
    const times = threadModel(reload.events, { now: later }).flatMap((item) => item.type === "group" || item.type === "coalesce" ? [item.time ?? ""] : []);
    expect(times).toEqual([...times].sort());
  });
  it("my own message keeps its sent time and no hint when the machine's clock runs behind", () => {
    const history = new ConversationHistory();
    history.submit("c", "sup", "Status?", now, undefined, "s");
    history.acknowledge({ client_ref: "c", notification_id: 10, target: "sup", stamped: true });
    // Missed during an outage: the answer, stamped by a clock 5 minutes behind.
    history.hydrateReply(durable(11, now - 300_000, { message: "All green." }), now + 60_000);
    expect(order(history)).toEqual(["send:10", "reply:11"]);
    expect(groups(history, now + 60_000)).toEqual(["you 13:36", "supervisor 13:31"]);
  });
  it("a turn's shown time does not drift with the render clock", () => {
    const history = new ConversationHistory();
    history.hydrateReply(durable(1, now + 86_400_000), now);
    expect(groups(history, now)).toEqual(["supervisor 13:36 ahead"]);
    expect(groups(history, now + 600_000)).toEqual(["supervisor 13:36 ahead"]);
  });
  it("marks a live turn only once the machine has been seen running ahead (review F02)", () => {
    const history = new ConversationHistory();
    history.receive(reply(1, "answer", "Hello."), now);
    expect(history.events[0]).not.toHaveProperty("clockAhead");
    history.hydrateReply(durable(0, now + 7_200_000), now + 1_000);
    history.receive(reply(2, "answer", "Still here."), now + 2_000);
    expect(history.events.at(-1)).toMatchObject({ clockAhead: true });
    // A stamp within the minute is not evidence of a clock ahead.
    const close = new ConversationHistory();
    close.hydrateReply(durable(0, now + 30_000), now);
    close.receive(reply(1, "answer", "Hi."), now + 1_000);
    expect(close.events.at(-1)).not.toHaveProperty("clockAhead");
  });
  it("a message keeps its place across the session line when a reconnect re-hydrates it", () => {
    const session = "patient-pelican-9";
    const history = new ConversationHistory();
    history.submit("c", "sup", "Are we back?", now, undefined, session);
    history.acknowledge({ client_ref: "c", notification_id: 40, target: "sup", stamped: true });
    history.receive(reply(41, "answer", "Back."), now + 1_000, session);
    const shape = () => threadModel(history.events, { now: now + 2_000 }).map((item) => item.type === "group" ? `${item.side}` : item.type);
    const before = shape();
    expect(before).toEqual(["day", "session", "you", "supervisor"]);
    // The reconnect's history page carries the same turns; a row without a session must not move the message.
    history.hydrateSend(message(40, "Are we back?", now), now + 2_000);
    history.hydrateReply(durable(41, now + 1_000, { message: "Back." }), now + 2_000);
    expect(shape()).toEqual(before);
    expect(history.events.map((event) => event.session)).toEqual([session, session]);
  });
  it("an answered ask stays answered when a reconnect's history row omits in_reply_to", () => {
    const history = new ConversationHistory();
    history.receive(reply(50, "ask", "Fix or ship?"), now);
    history.submit("a", "sup", "Fix", now + 1_000, 50);
    history.acknowledge({ client_ref: "a", notification_id: 51, target: "sup", stamped: true });
    expect(history.pinnedAsk()).toBeUndefined();
    history.hydrateSend(message(51, "Fix", now + 1_000), now + 2_000);
    expect(history.pinnedAsk()).toBeUndefined();
    expect(history.answered(50)?.text).toBe("Fix");
  });
});

describe("a reload keeps each turn's time (cas-8d52, journey F11)", () => {
  const AHEAD = 5 * 60_000;
  const iso = (ms: number) => new Date(ms).toISOString();
  const shownLabels = (history: ConversationHistory, now: number) => shownTimes(history.events, now).map((time) => clockLabel(time.at));

  it("rebuilds a machine-ahead thread at the times the visit showed, not the reload's", () => {
    // The visit: a supervisor turn arrives at 09:00 and the operator answers at 09:01.
    const visit = new ConversationHistory();
    visit.currentSession = "live";
    visit.receive(reply(7, "answer"), at(9, 0), "live");
    visit.submit("c1", "sup", "On it.", at(9, 1), undefined, "live");
    visit.acknowledge({ client_ref: "c1", notification_id: 8, target: "sup", stamped: true } as never);
    const record = visit.arrivalsRecord();
    expect(record.at).toEqual({ "r:7": at(9, 0), "s:8": at(9, 1) });
    // The reload, an hour later: the machine's history carries its own stamps, five minutes ahead.
    const later = at(10, 3);
    const reloaded = new ConversationHistory();
    reloaded.currentSession = "live";
    reloaded.seedArrivals(record);
    reloaded.hydrateReply({ ...reply(7, "answer"), at: iso(at(9, 0) + AHEAD), session: "live" } as never, later);
    reloaded.hydrateSend({ notification_id: 8, target: "sup", text: "On it.", state: "acknowledged", stamped: true, at: iso(at(9, 1) + AHEAD), session: "live" } as never, later);
    expect(shownLabels(reloaded, later)).toEqual(["09:00", "09:01"]);
    // Without the record the rebuild shows the machine's own stamp, five
    // minutes ahead of what the visit showed (or the reload's time, when the
    // reload comes sooner than the lead): the defect.
    const forgetful = new ConversationHistory();
    forgetful.currentSession = "live";
    forgetful.hydrateReply({ ...reply(7, "answer"), at: iso(at(9, 0) + AHEAD), session: "live" } as never, later);
    expect(shownLabels(forgetful, later)).toEqual(["09:05"]);
    const soon = new ConversationHistory();
    soon.hydrateReply({ ...reply(7, "answer"), at: iso(at(9, 0) + AHEAD), session: "live" } as never, at(9, 2));
    expect(shownLabels(soon, at(9, 2))).toEqual(["09:02"]);
  });

  it("measures the machine's lead from a live turn, and places a turn never seen by it", () => {
    const visit = new ConversationHistory();
    visit.currentSession = "live";
    visit.receive(reply(7, "answer"), at(9, 0), "live");
    // The reconnect replays it with the machine's stamp: the lead is measured.
    visit.hydrateReply({ ...reply(7, "answer"), at: iso(at(9, 0) + AHEAD), session: "live" } as never, at(9, 1));
    expect(visit.arrivalsRecord().skew).toBe(AHEAD);
    const later = at(10, 3);
    const history = new ConversationHistory();
    history.currentSession = "live";
    history.seedArrivals(visit.arrivalsRecord());
    history.hydrateReply({ ...reply(7, "answer"), at: iso(at(9, 0) + AHEAD), session: "live" } as never, later);
    history.hydrateReply({ ...reply(9, "answer"), at: iso(at(9, 30) + AHEAD), session: "live" } as never, later);
    expect(shownLabels(history, later)).toEqual(["09:00", "09:30"]);
  });

  it("marks a live turn on a reload exactly as the visit did (cas-9e33)", () => {
    const marks = (history: ConversationHistory, now: number) => shownTimes(history.events, now).map((time) => `${clockLabel(time.at)}${time.clockAhead ? " ahead" : ""}`);
    // The visit: nothing yet says the machine's clock runs ahead, so the live answer shows 12:00 unmarked.
    const visit = new ConversationHistory();
    visit.currentSession = "live";
    visit.receive(reply(7, "answer"), at(12, 0), "live");
    expect(marks(visit, at(12, 0))).toEqual(["12:00"]);
    expect(visit.arrivalsRecord().live).toEqual({ "r:7": false });
    // The reload, three minutes later, rebuilds it from the machine's stamp, 12:05.
    const reloaded = new ConversationHistory();
    reloaded.currentSession = "live";
    reloaded.seedArrivals(visit.arrivalsRecord());
    reloaded.hydrateReply({ ...reply(7, "answer"), at: iso(at(12, 0) + AHEAD), session: "live" } as never, at(12, 3));
    expect(marks(reloaded, at(12, 3)), "the reload agrees with the visit").toEqual(["12:00"]);
    // It still measures the lead, so the next live turn is marked, and that mark survives the next reload too.
    expect(reloaded.arrivalsRecord().skew).toBe(AHEAD);
    reloaded.receive(reply(8, "answer"), at(12, 4), "live");
    expect(marks(reloaded, at(12, 4))).toEqual(["12:00", "12:04 ahead"]);
    expect(reloaded.arrivalsRecord().live).toEqual({ "r:7": false, "r:8": true });
    const again = new ConversationHistory();
    again.currentSession = "live";
    again.seedArrivals(reloaded.arrivalsRecord());
    again.hydrateReply({ ...reply(7, "answer"), at: iso(at(12, 0) + AHEAD), session: "live" } as never, at(12, 9));
    again.hydrateReply({ ...reply(8, "answer"), at: iso(at(12, 4) + AHEAD), session: "live" } as never, at(12, 9));
    expect(marks(again, at(12, 9))).toEqual(["12:00", "12:04 ahead"]);
    // A turn never seen live, or a record from before cas-9e33, is still marked from its stamp.
    const legacy = new ConversationHistory();
    legacy.currentSession = "live";
    legacy.seedArrivals({ at: visit.arrivalsRecord().at });
    legacy.hydrateReply({ ...reply(7, "answer"), at: iso(at(12, 0) + AHEAD), session: "live" } as never, at(12, 3));
    expect(marks(legacy, at(12, 3))).toEqual(["12:00 ahead"]);
  });

  it("dates the thread's activity by the time it shows, not the machine's stamp (cas-24fe)", () => {
    const visit = new ConversationHistory();
    visit.currentSession = "live";
    visit.receive(reply(7, "answer"), at(12, 0), "live");
    expect(visit.lastActivityAt()).toBe(at(12, 0));
    // After a reload the turn comes back stamped five minutes ahead; the row still dates it 12:00.
    const reloaded = new ConversationHistory();
    reloaded.currentSession = "live";
    reloaded.seedArrivals(visit.arrivalsRecord());
    reloaded.hydrateReply({ ...reply(7, "answer"), at: iso(at(12, 0) + AHEAD), session: "live" } as never, at(12, 3));
    expect(reloaded.lastActivityAt()).toBe(at(12, 0));
    // A live turn sorted after a future-stamped one keeps its own arrival.
    reloaded.receive(reply(8, "answer"), at(12, 4), "live");
    expect(reloaded.lastActivityAt()).toBe(at(12, 4));
    // A message that never reached the machine does not date the row (cas-b00c).
    reloaded.submit("c1", "sup", "Still there?", at(12, 6), undefined, "live");
    expect(reloaded.lastActivityAt()).toBe(at(12, 4));
    expect(new ConversationHistory().lastActivityAt()).toBeUndefined();
  });

  it("keeps a turn first shown from history at that time on the next reload", () => {
    // First visit at 09:02: an old blocker stamped 09:05 by a clock five minutes ahead shows 09:02.
    const first = new ConversationHistory();
    first.hydrateReply({ ...reply(3, "blocker"), at: iso(at(9, 5)) } as never, at(9, 2));
    expect(shownLabels(first, at(9, 2))).toEqual(["09:02"]);
    // Reload at 09:20: it still shows 09:02, not 09:05 or 09:20.
    const second = new ConversationHistory();
    second.seedArrivals(first.arrivalsRecord());
    second.hydrateReply({ ...reply(3, "blocker"), at: iso(at(9, 5)) } as never, at(9, 20));
    expect(shownLabels(second, at(9, 20))).toEqual(["09:02"]);
    expect(second.arrivalsRecord().skew, "a turn not seen live measures nothing").toBeUndefined();
  });
});

describe("turns minutes apart keep their own time (cas-d043 H14)", () => {
  it("splits a run of answers 8 minutes apart, and keeps a run a minute apart together", () => {
    const at = new Date(2026, 9, 6, 12, 0).getTime();
    const answer = (id: number, when: number): ConversationEvent => ({ kind: "reply", value: reply(id, "answer", `Answer ${id}`), at: when } as ConversationEvent);
    const shown = (events: ConversationEvent[]) => threadModel(events, { now: at + 600_000 }).filter((item) => item.type === "group").map((item) => item.type === "group" ? `${item.turns.length}@${item.time}` : "");
    expect(shown([answer(1, at), answer(2, at + 8 * 60_000)])).toEqual(["1@12:00", "1@12:08"]);
    expect(shown([answer(1, at), answer(2, at + 60_000)])).toEqual(["2@12:01"]);
  });
});

describe("a kept reply is marked the same before and after a reload (cas-d043 G03)", () => {
  it("does not take the time it was stored for the machine's stamp", () => {
    const now = Date.UTC(2026, 8, 30, 12, 0);
    const yesterday = Date.UTC(2026, 8, 29, 17, 20);
    const iso = (ms: number) => new Date(ms).toISOString();
    const marks = (history: ConversationHistory, at: number) => shownTimes(history.events, at).map((time) => time.clockAhead);
    const blocker = { ...reply(900, "blocker", "The ledger import is red."), at: iso(yesterday), session: "S" };
    const visit = new ConversationHistory();
    visit.currentSession = "S";
    visit.hydrateReply(blocker as never, now);
    expect(marks(visit, now)).toEqual([false]);
    // Reload: the journal restores the kept reply first, stored at 12:00 today.
    const reload = new ConversationHistory();
    reload.currentSession = "S";
    reload.seedArrivals(visit.arrivalsRecord());
    reload.hydrateKeptReply({ ...reply(900, "blocker", "The ledger import is red."), at: iso(now), session: "S" } as never, now + 60_000);
    expect(marks(reload, now + 60_000), "before the machine answers").toEqual([false]);
    // Then the machine's page brings its own stamp: still as the visit showed it.
    reload.hydrateReply(blocker as never, now + 60_000);
    expect(marks(reload, now + 60_000), "after the machine's page").toEqual([false]);
    expect(reload.events[0]!.at).toBe(yesterday);
  });
});
