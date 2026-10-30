// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it, onTestFinished } from "vitest";
import { USAGE_ACTIONS, UsageCounter, bindRichInspectionUsage, countAttentionDelivery, type UsageEntry } from "./usage";

function memoryStore() {
  const values = new Map<string, string>();
  return { getItem: (key: string) => values.get(key) ?? null, setItem: (key: string, value: string) => void values.set(key, value) };
}

describe("usage counter (cas-0b3d)", () => {
  it("counts opening diagram and diff inspections even when their toggle events coalesce", async () => {
    const counter = new UsageCounter(memoryStore(), () => new Date("2026-10-26T12:00:00Z"));
    const root = document.createElement("div");
    root.innerHTML = '<figure class="rich-diagram"><details><summary>Diagram source</summary>source</details></figure><div class="rich-diff"><details><summary>Show diff</summary>diff</details></div>';
    document.body.append(root); onTestFinished(() => root.remove());
    bindRichInspectionUsage(root, counter);
    const details = [...root.querySelectorAll("details")];
    const delivered = Promise.all(details.map(detail => new Promise(resolve => detail.addEventListener("toggle", resolve, { once: true }))));
    for (const detail of details) {
      const summary = detail.querySelector("summary")!;
      summary.click(); expect(detail.open).toBe(true);
      summary.click(); expect(detail.open).toBe(false);
    }
    await delivered;
    expect(counter.pending("m")).toEqual([{ day: "2026-10-26", action: "rich-content.inspect", journey: "HUB-J22", count: 2 }]);
  });

  it("sends each changed day to every machine once, cumulatively, and retries a failed post", async () => {
    let now = new Date("2026-10-26T12:00:00Z");
    const counter = new UsageCounter(memoryStore(), () => now);
    const received: Record<string, UsageEntry[][]> = { soundwave: [], prowl: [] };
    let prowlUp = false;
    const machines = [
      { id: "soundwave", post: async (entries: UsageEntry[]) => void received.soundwave.push(entries) },
      { id: "prowl", post: async (entries: UsageEntry[]) => { if (!prowlUp) throw new Error("offline"); received.prowl.push(entries); } },
    ];
    counter.count("reply.typed");
    counter.count("reply.typed");
    const resolved = { kind: "delivery_stall", acknowledgedAt: undefined as string | undefined };
    // Self-resolution counts once; its replay and a resolution with no item do not.
    countAttentionDelivery([{ kind: "session_transport" }, resolved], counter);
    resolved.acknowledgedAt = now.toISOString();
    countAttentionDelivery([resolved], counter);
    countAttentionDelivery([], counter);
    await counter.flush(machines);
    await counter.flush(machines);
    expect(received.soundwave).toEqual([[
      { day: "2026-10-26", action: "reply.typed", journey: "HUB-J5", count: 2 },
      { day: "2026-10-26", action: "attention.delivery", journey: "HUB-J15", count: 1 },
    ]]);
    expect(received.prowl).toEqual([]);
    now = new Date("2026-10-27T08:00:00Z");
    counter.count("ask.answer");
    prowlUp = true;
    await counter.flush(machines);
    expect(received.soundwave[1]).toEqual([{ day: "2026-10-27", action: "ask.answer", journey: "HUB-J7", count: 1 }]);
    expect(received.prowl).toEqual([[
      { day: "2026-10-26", action: "reply.typed", journey: "HUB-J5", count: 2 },
      { day: "2026-10-26", action: "attention.delivery", journey: "HUB-J15", count: 1 },
      { day: "2026-10-27", action: "ask.answer", journey: "HUB-J7", count: 1 },
    ]]);
  });

  it("a flush asked for while another is posting still reaches the machine that just came online", async () => {
    const counter = new UsageCounter(memoryStore(), () => new Date("2026-10-26T12:00:00Z"));
    counter.count("inbox.open");
    let release!: () => void;
    const slow = new Promise<void>((done) => { release = done; });
    const posted: string[] = [];
    const live = [{ id: "prowl", post: async () => { await slow; posted.push("prowl"); } }];
    const first = counter.flushQueued(() => [...live]);
    live.push({ id: "soundwave", post: async () => { posted.push("soundwave"); } });
    const second = counter.flushQueued(() => [...live]);
    release();
    await Promise.all([first, second]);
    expect(posted).toEqual(["prowl", "soundwave"]);
  });

  it("counts an action for every live Commander journey in the registry", () => {
    const registry = readFileSync(resolve(dirname(fileURLToPath(import.meta.url)), "../../docs/qa/journeys.yaml"), "utf8");
    const live = [...registry.matchAll(/^- id: (\S+)\n(?:  .*\n)*?  status: live$/gm)].map((match) => match[1]);
    expect(live.length).toBeGreaterThan(0);
    expect([...new Set(Object.values(USAGE_ACTIONS))].sort()).toEqual([...live].sort());
  });
});
