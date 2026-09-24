import { describe, expect, it } from "vitest";
import { ConversationHistory } from "./conversation-history";
import { blockerEvidence, cellTone, coalesceText, dayLabel, foldsAsStatus, messageBlocks, STATUS_FOLD_LIMIT, statusAsks, threadModel, type ThreadGroup } from "./thread-model";
import ROW_20812 from "./fixtures/hub-row-20812.txt?raw";
import type { OperatorReply, OperatorTurnKind } from "./types";

const NOW = new Date(2026, 8, 21, 10, 0).getTime();
const at = (hh: number, mm: number, dayOffset = 0) => new Date(2026, 8, 21 + dayOffset, hh, mm).getTime();
function reply(id: number, kind: OperatorTurnKind, message = `m${id}`, reply_to: number | null = null): OperatorReply {
  return { notification_id: id, reply_to, message, summary: "", device_id: "d", kind };
}

describe("threadModel", () => {
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
    expect(history.events.at(-1)).toMatchObject({ kind: "send", at: now + 1_000 });
    const groups = threadModel(history.events, { now }).filter((item) => item.type === "group");
    expect(groups.at(-1)).toMatchObject({ side: "you", time: "12:00" });
  });
  it("a send stamped before a live turn clamped ahead of it still sorts after and shows its own time", () => {
    // A browser clock that stepped back: the latest turn's key is ahead of the new send.
    const now = new Date(2026, 8, 24, 12, 0).getTime();
    const history = new ConversationHistory();
    history.receive({ notification_id: 1, reply_to: null, message: "Ack.", summary: "", device_id: "d", kind: "answer" }, now + 300_000);
    history.submit("s", "sup", "On it", now);
    expect(history.events.at(-1)).toMatchObject({ kind: "send", at: now + 300_000, shownAt: now });
    const groups = threadModel(history.events, { now }).filter((item) => item.type === "group");
    expect(groups.at(-1)).toMatchObject({ side: "you", time: "12:00" });
  });
});

describe("thread order is stable under a machine clock ahead and a reconnect (cas-1f13)", () => {
  const now = new Date(2026, 8, 24, 13, 36).getTime();
  const durable = (id: number, at: number, extra: Partial<OperatorReply> & { session?: string } = {}) => ({ notification_id: id, reply_to: null, message: `m${id}`, summary: "", device_id: "d", kind: "answer" as const, attachments: [], at: new Date(at).toISOString(), ...extra });
  const days = (history: ConversationHistory, clock: number) => threadModel(history.events, { now: clock }).filter((item) => item.type === "day").map((item) => item.type === "day" ? item.label : "");
  it("a day-ahead machine never puts a future day header above Today", () => {
    const history = new ConversationHistory();
    history.hydrateReply(durable(1, now - 3_600_000), now);
    history.hydrateReply(durable(2, now + 86_400_000), now);
    history.submit("s", "sup", "On it", now + 1_000);
    history.receive(reply(3, "answer", "Ack."), now + 60_000);
    expect(days(history, now + 60_000)).toEqual(["Today"]);
    // The future turn sorts at its arrival, keeps the machine's stamp, and says the clock is ahead.
    expect(history.events.find((event) => event.kind === "reply" && event.value.notification_id === 2)).toMatchObject({ at: now, stampedAt: now + 86_400_000 });
    const groups = threadModel(history.events, { now: now + 60_000 }).filter((item): item is ThreadGroup => item.type === "group");
    expect(groups.map((group) => [group.side, group.time, group.clockAhead === true])).toEqual([
      ["supervisor", "13:36", true], ["you", "13:36", false], ["supervisor", "13:37", false],
    ]);
  });
  it("orders turns by the clamped time: a 13:41 blocker from a clock 5 minutes ahead does not sit above a 13:36 session start", () => {
    const history = new ConversationHistory();
    history.hydrateReply(durable(900, now + 300_000, { kind: "blocker", message: "The release gate went red." }), now);
    history.receive(reply(901, "ask", "Fix or ship?"), now + 5_000, "patient-pelican-9");
    const items = threadModel(history.events, { now: now + 5_000 });
    const shown = items.map((item) => item.type === "group" ? `${item.side} ${item.time}${item.clockAhead ? " ahead" : ""}` : item.type === "session" ? item.label : item.type === "day" ? item.label : item.type);
    expect(shown).toEqual(["Today", "supervisor 13:36 ahead", "session patient-pelican-9 started 13:36", "supervisor 13:36"]);
  });
  it("keeps the machine's own order among turns clamped to the same arrival", () => {
    const history = new ConversationHistory();
    // Replies hydrate after messages, and in any order within a page.
    history.hydrateReply(durable(3, now + 240_000), now);
    history.hydrateSend({ notification_id: 2, target: "sup", text: "q", state: "acknowledged", stamped: true, device_id: "d", at: new Date(now + 120_000).toISOString() }, now);
    history.hydrateReply(durable(1, now + 60_000), now);
    history.hydrateReply(durable(0, now - 60_000), now);
    expect(history.events.map((event) => event.kind === "send" ? `send:${event.value.notificationId}` : `reply:${event.value.notification_id}`)).toEqual(["reply:0", "reply:1", "send:2", "reply:3"]);
    // A few seconds of skew changes nothing on screen, so it earns no hint.
    const slight = new ConversationHistory();
    slight.hydrateReply(durable(5, now + 2_000), now);
    expect(slight.events[0]).toMatchObject({ at: now, stampedAt: now + 2_000 });
    expect(threadModel(slight.events, { now }).find((item) => item.type === "group")).toMatchObject({ clockAhead: false });
  });
  it("a message keeps its place across the session line when a reconnect re-hydrates it", () => {
    const session = "patient-pelican-9";
    const history = new ConversationHistory();
    history.submit("c", "sup", "Are we back?", now, undefined, session);
    history.acknowledge({ client_ref: "c", notification_id: 40, target: "sup", stamped: true });
    history.receive(reply(41, "answer", "Back."), now + 1_000, session);
    const order = () => threadModel(history.events, { now: now + 2_000 }).map((item) => item.type === "group" ? `${item.side}` : item.type);
    const before = order();
    expect(before).toEqual(["day", "session", "you", "supervisor"]);
    // The reconnect's history page carries the same turns; a row without a session must not move the message.
    history.hydrateSend({ notification_id: 40, target: "sup", text: "Are we back?", state: "acknowledged", stamped: true, device_id: "d", at: new Date(now).toISOString() }, now + 2_000);
    history.hydrateReply(durable(41, now + 1_000, { message: "Back." }), now + 2_000);
    expect(order()).toEqual(before);
    expect(history.events.map((event) => event.session)).toEqual([session, session]);
  });
  it("an answered ask stays answered when a reconnect's history row omits in_reply_to", () => {
    const history = new ConversationHistory();
    history.receive(reply(50, "ask", "Fix or ship?"), now);
    history.submit("a", "sup", "Fix", now + 1_000, 50);
    history.acknowledge({ client_ref: "a", notification_id: 51, target: "sup", stamped: true });
    expect(history.pinnedAsk()).toBeUndefined();
    history.hydrateSend({ notification_id: 51, target: "sup", text: "Fix", state: "acknowledged", stamped: true, device_id: "d", at: new Date(now + 1_000).toISOString() }, now + 2_000);
    expect(history.pinnedAsk()).toBeUndefined();
    expect(history.answered(50)?.text).toBe("Fix");
  });
});
