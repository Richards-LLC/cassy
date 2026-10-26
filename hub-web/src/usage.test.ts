import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { USAGE_ACTIONS, UsageCounter, type UsageEntry } from "./usage";

function memoryStore() {
  const values = new Map<string, string>();
  return { getItem: (key: string) => values.get(key) ?? null, setItem: (key: string, value: string) => void values.set(key, value) };
}

describe("usage counter (cas-0b3d)", () => {
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
    await counter.flush(machines);
    await counter.flush(machines);
    expect(received.soundwave).toEqual([[{ day: "2026-10-26", action: "reply.typed", journey: "HUB-J5", count: 2 }]]);
    expect(received.prowl).toEqual([]);
    now = new Date("2026-10-27T08:00:00Z");
    counter.count("ask.answer");
    prowlUp = true;
    await counter.flush(machines);
    expect(received.soundwave[1]).toEqual([{ day: "2026-10-27", action: "ask.answer", journey: "HUB-J7", count: 1 }]);
    expect(received.prowl).toEqual([[
      { day: "2026-10-26", action: "reply.typed", journey: "HUB-J5", count: 2 },
      { day: "2026-10-27", action: "ask.answer", journey: "HUB-J7", count: 1 },
    ]]);
  });

  it("counts an action for every live Commander journey in the registry", () => {
    const registry = readFileSync(new URL("../../docs/qa/journeys.yaml", import.meta.url), "utf8");
    const live = [...registry.matchAll(/^- id: (\S+)\n(?:  .*\n)*?  status: live$/gm)].map((match) => match[1]);
    expect(live.length).toBeGreaterThan(0);
    expect([...new Set(Object.values(USAGE_ACTIONS))].sort()).toEqual([...live].sort());
  });
});
