import { describe, expect, it } from "vitest";
import { ConversationHistory } from "./conversation-history";
import { blockerEvidence, cellTone, coalesceText, dayLabel, messageBlocks, threadModel, type ThreadGroup } from "./thread-model";
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
  it("renders a lone status as its own quiet text without a count", () => {
    const history = new ConversationHistory();
    history.reply(reply(1, "status", "Rebasing"), at(9, 49));
    const [, line] = threadModel(history.events, { now: NOW });
    if (line?.type !== "coalesce") throw new Error("expected coalesce");
    expect(coalesceText(line)).toBe("Rebasing");
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
