/**
 * Commander usage counter (cas-0b3d): how often each journey's defining act
 * happens, so the most-used flows are tested and fixed first.
 *
 * Counts stay in this browser, per UTC day, as cumulative totals of named
 * actions: no session, project, machine, message text or timing. Each paired
 * machine has its own sent cursor and receives every changed day, so any one
 * hub's ledger covers every browser that reached it; the hub keeps the highest
 * count it has seen, so a repeated post never double counts. Nothing is sent
 * anywhere but the operator's own paired machines.
 */

/** Each counted action and the registry journey (docs/qa/journeys.yaml) it measures. */
export const USAGE_ACTIONS = {
  "pair.code": "HUB-J1",
  "pair.link": "HUB-J2",
  "conversation.open": "HUB-J3",
  "history.older": "HUB-J4",
  "reply.typed": "HUB-J5",
  "reply.voice": "HUB-J6",
  "ask.answer": "HUB-J7",
  "machine.switch": "HUB-J8",
  "phone.conversation.open": "HUB-J9",
  "scheme.change": "HUB-J10",
  "connection.recovered": "HUB-J11",
  "network.switch": "HUB-J12",
  "session.launch": "HUB-J13",
  "session.compare": "HUB-J14",
  "attention.delivery": "HUB-J15",
  "session.end": "HUB-J16",
  "fleet.operation": "HUB-J17",
  "supervisor.control": "HUB-J18",
  "inbox.open": "HUB-J19",
  "write-grant": "HUB-J20",
  "app-switch.explorer": "HUB-J21",
  "reply.attachment": "HUB-J24",
} as const;

export type UsageAction = keyof typeof USAGE_ACTIONS;

export interface UsageEntry {
  day: string;
  action: string;
  journey: string;
  count: number;
}

interface UsageState {
  /** day -> action -> cumulative count */
  days: Record<string, Record<string, number>>;
  /** machine id -> day -> that day's total as last accepted by the machine */
  sent: Record<string, Record<string, number>>;
}

export const USAGE_STORAGE_KEY = "commander.usage.v1";
/** Days kept and reported; the hub refuses older days. */
export const USAGE_RETENTION_DAYS = 35;

type Store = Pick<Storage, "getItem" | "setItem">;

function day(now: Date): string {
  return now.toISOString().slice(0, 10);
}

/** A day's total over the actions this build counts, so a retired action name never forces a resend. */
function total(actions: Record<string, number> | undefined): number {
  return Object.entries(actions ?? {}).reduce((sum, [action, count]) => sum + (action in USAGE_ACTIONS && count > 0 ? count : 0), 0);
}

export class UsageCounter {
  constructor(private readonly store: Store | null = safeLocalStorage(), private readonly clock: () => Date = () => new Date()) {}

  private read(): UsageState {
    try {
      const parsed = JSON.parse(this.store?.getItem(USAGE_STORAGE_KEY) ?? "null") as Partial<UsageState> | null;
      if (parsed && typeof parsed.days === "object" && typeof parsed.sent === "object") return { days: parsed.days ?? {}, sent: parsed.sent ?? {} };
    } catch { /* A corrupt record starts over. */ }
    return { days: {}, sent: {} };
  }

  private write(state: UsageState): void {
    const oldest = day(new Date(this.clock().getTime() - (USAGE_RETENTION_DAYS - 1) * 86_400_000));
    for (const key of Object.keys(state.days)) if (key < oldest) delete state.days[key];
    for (const days of Object.values(state.sent)) for (const key of Object.keys(days)) if (key < oldest) delete days[key];
    try { this.store?.setItem(USAGE_STORAGE_KEY, JSON.stringify(state)); } catch { /* Counting is best effort. */ }
  }

  count(action: UsageAction): void {
    const state = this.read();
    const today = day(this.clock());
    const counts = (state.days[today] ??= {});
    counts[action] = (counts[action] ?? 0) + 1;
    this.write(state);
  }

  /** Every action of every day whose total this machine has not yet accepted. */
  pending(machineId: string): UsageEntry[] {
    const state = this.read();
    const sent = state.sent[machineId] ?? {};
    const entries: UsageEntry[] = [];
    for (const [key, actions] of Object.entries(state.days).sort(([a], [b]) => a.localeCompare(b))) {
      if (total(actions) === sent[key]) continue;
      for (const [action, count] of Object.entries(actions)) {
        const journey = USAGE_ACTIONS[action as UsageAction];
        if (journey && count > 0) entries.push({ day: key, action, journey, count });
      }
    }
    return entries;
  }

  /** Record what a machine accepted, so unchanged days are not sent again. */
  accepted(machineId: string, entries: readonly UsageEntry[]): void {
    const state = this.read();
    const sent = (state.sent[machineId] ??= {});
    const days = new Map<string, number>();
    for (const entry of entries) days.set(entry.day, (days.get(entry.day) ?? 0) + entry.count);
    for (const [key, sum] of days) sent[key] = sum;
    this.write(state);
  }

  private queue: Promise<void> = Promise.resolve();

  /**
   * Queue a flush to the machines `machines()` names when it runs. Requests
   * never merge: a machine that comes online while an earlier flush is still
   * posting gets its own turn, with the machines live at that moment.
   */
  flushQueued(machines: () => readonly { id: string; post: (entries: UsageEntry[]) => Promise<unknown> }[]): Promise<void> {
    this.queue = this.queue.then(() => this.flush(machines()));
    return this.queue;
  }

  /** Post each machine's pending days; a failed post is retried next time. */
  async flush(machines: readonly { id: string; post: (entries: UsageEntry[]) => Promise<unknown> }[]): Promise<void> {
    for (const machine of machines) {
      const entries = this.pending(machine.id);
      if (entries.length === 0) continue;
      try {
        await machine.post(entries);
        this.accepted(machine.id, entries);
      } catch { /* Kept for the next flush. */ }
    }
  }
}

function safeLocalStorage(): Store | null {
  try { return typeof localStorage === "undefined" ? null : localStorage; } catch { return null; }
}

/** The page's counter. */
export const usage = new UsageCounter();

export function countUsage(action: UsageAction): void {
  usage.count(action);
}
