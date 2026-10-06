/**
 * Per-conversation state that must survive a reload or a same-tab navigation
 * (cas-7752 drafts; cas-e7b1 messages that have not been confirmed). Each
 * namespace is one bounded localStorage entry, keyed by the conversation's
 * thread key (`<machine>:<session>`), so nothing leaks between machines or
 * sessions. Callers delete a conversation's entry the moment its content is
 * sent, confirmed or discarded.
 *
 * Storage that is missing, unreadable, foreign or full never breaks the page:
 * reads yield nothing and writes keep only the in-memory state.
 */
export type StorageLike = Pick<Storage, "getItem" | "setItem" | "removeItem">;

export type ConversationStoreOptions = {
  /** At most this many conversations are kept; the least recently written go first. */
  maxConversations?: number;
  /** A single conversation's serialized value above this many characters is not stored. */
  maxValueChars?: number;
};

type Record_<T> = { value: T; updatedAt: number };

export const CONVERSATION_STORE_PREFIX = "cas-commander-conversation";

export class ConversationStore<T> {
  readonly key: string;
  private readonly maxConversations: number;
  private readonly maxValueChars: number;
  private readCleanupAttempted = false;

  constructor(
    private readonly storage: StorageLike | undefined,
    namespace: string,
    /** Narrow one stored value; undefined drops it. */
    private readonly validate: (raw: unknown) => T | undefined,
    options: ConversationStoreOptions = {},
    private readonly now: () => number = Date.now,
  ) {
    this.key = `${CONVERSATION_STORE_PREFIX}:${namespace}:v1`;
    this.maxConversations = options.maxConversations ?? 50;
    this.maxValueChars = options.maxValueChars ?? 64_000;
  }

  /** Every stored conversation's value, most recently written last. */
  entries(): Map<string, T> {
    return new Map([...this.read()].sort(([, a], [, b]) => a.updatedAt - b.updatedAt).map(([key, record]) => [key, record.value]));
  }

  get(conversation: string): T | undefined {
    return this.read().get(conversation)?.value;
  }

  /**
   * Store one conversation's value, evicting the least recently written beyond
   * the bound. A value over the per-conversation bound is not stored (and any
   * older copy is removed): false says so, so the caller can tell the operator
   * it will not survive a reload (cas-adfc).
   */
  set(conversation: string, value: T): boolean {
    return this.put(conversation, value) !== "too-long";
  }

  /**
   * As `set`, saying what happened: `kept` is on disk, `too-long` is over the
   * per-conversation bound, `not-saved` fit but the browser refused the write
   * (a full or denied localStorage), so it lives only in this page (cas-f657).
   */
  put(conversation: string, value: T): "kept" | "too-long" | "not-saved" {
    const records = this.read();
    const fits = JSON.stringify(value).length <= this.maxValueChars;
    records.delete(conversation);
    if (fits) records.set(conversation, { value, updatedAt: this.now() });
    const ordered = [...records].sort(([, a], [, b]) => a.updatedAt - b.updatedAt);
    while (ordered.length > this.maxConversations) ordered.shift();
    const written = this.write(new Map(ordered));
    return !fits ? "too-long" : written ? "kept" : "not-saved";
  }

  delete(conversation: string): void {
    const records = this.read();
    if (!records.delete(conversation)) return;
    this.write(records);
  }

  private read(): Map<string, Record_<T>> {
    let records = new Map<string, Record_<T>>();
    let raw: string | null = null;
    try { raw = this.storage?.getItem(this.key) ?? null; } catch { this.cleanupRead(records); return records; }
    if (raw === null) return records;
    let parsed: unknown;
    try { parsed = JSON.parse(raw); } catch { this.cleanupRead(records); return records; }
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) { this.cleanupRead(records); return records; }
    let dropped = false;
    for (const [conversation, entry] of Object.entries(parsed as Record<string, unknown>)) {
      if (!entry || typeof entry !== "object") { dropped = true; continue; }
      const { value, updatedAt } = entry as { value?: unknown; updatedAt?: unknown };
      // The bounds kept on write also apply to storage planted by another
      // page. Unreadable or invalid values never act on this conversation.
      try {
        if (JSON.stringify(value ?? null).length > this.maxValueChars) { dropped = true; continue; }
        const narrowed = this.validate(value);
        if (narrowed === undefined) { dropped = true; continue; }
        records.set(conversation, { value: narrowed, updatedAt: typeof updatedAt === "number" && Number.isFinite(updatedAt) ? updatedAt : 0 });
      } catch { dropped = true; }
    }
    if (records.size > this.maxConversations) {
      records = new Map([...records].sort(([, a], [, b]) => a.updatedAt - b.updatedAt).slice(-this.maxConversations));
      dropped = true;
    }
    if (dropped) this.cleanupRead(records);
    return records;
  }

  /** Repair once per load, including a failed attempt, so denied storage does
   * not turn every get/entries call into another quota-consuming write. */
  private cleanupRead(records: Map<string, Record_<T>>): void {
    if (this.readCleanupAttempted) return;
    this.readCleanupAttempted = true;
    this.write(records);
  }

  /** False when the browser refused the write (full or denied): the in-memory state still holds for this page. */
  private write(records: Map<string, Record_<T>>): boolean {
    if (!this.storage) return false;
    try {
      if (records.size) this.storage.setItem(this.key, JSON.stringify(Object.fromEntries(records)));
      else this.storage.removeItem(this.key);
      return true;
    } catch { return false; }
  }
}

export type EnumerableStorage = StorageLike & Pick<Storage, "key" | "length">;

/**
 * Drafts and unconfirmed messages are the operator's words, kept on disk past
 * the page. When a machine's pairing is removed or revoked, every namespace's
 * conversations on that machine go with it (`machineId`); with no machine,
 * every conversation store is cleared. Namespaces added later (cas-e7b1) are
 * covered without registering anything: they share the key prefix.
 */
export function purgeConversations(storage: EnumerableStorage | undefined, machineId?: string): void {
  if (!storage) return;
  const prefix = `${CONVERSATION_STORE_PREFIX}:`;
  let keys: string[] = [];
  try {
    for (let index = 0; index < storage.length; index += 1) {
      const key = storage.key(index);
      if (key?.startsWith(prefix)) keys.push(key);
    }
  } catch { keys = []; }
  for (const key of keys) {
    try {
      if (machineId === undefined) { storage.removeItem(key); continue; }
      const raw = storage.getItem(key);
      const parsed: unknown = raw ? JSON.parse(raw) : undefined;
      if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) { storage.removeItem(key); continue; }
      const kept = Object.fromEntries(Object.entries(parsed as Record<string, unknown>).filter(([conversation]) => !conversation.startsWith(`${machineId}:`)));
      if (Object.keys(kept).length) storage.setItem(key, JSON.stringify(kept)); else storage.removeItem(key);
    } catch { try { storage.removeItem(key); } catch { /* denied: nothing more to do */ } }
  }
}

/** A conversation composer draft (cas-7752). */
export type Draft = { text: string; caret: number };

/** A stored draft, or undefined when blank or malformed; the caret is clamped to the text. */
export function validDraft(raw: unknown): Draft | undefined {
  if (!raw || typeof raw !== "object") return undefined;
  const { text, caret } = raw as { text?: unknown; caret?: unknown };
  if (typeof text !== "string" || !text.trim()) return undefined;
  const at = typeof caret === "number" && Number.isFinite(caret) ? Math.max(0, Math.min(Math.trunc(caret), text.length)) : text.length;
  return { text, caret: at };
}

/**
 * What saving a draft did: `kept` is on disk for the next load, `cleared` was
 * blank (or withheld) and is gone, `too-long` is over the store's bound, so it
 * lives only in this page and a reload loses it (cas-adfc), and `not-saved`
 * fit but the browser refused to store it (full or denied localStorage), so it
 * too lives only in this page (cas-f657).
 */
export type DraftSave = "kept" | "cleared" | "too-long" | "not-saved";

/** The drafts store: a blank draft is a deletion, never a stored entry. */
export function draftStore(storage: StorageLike | undefined, options: ConversationStoreOptions = {}): {
  load(): Map<string, Draft>;
  save(conversation: string, draft: Draft | undefined): DraftSave;
} {
  const store = new ConversationStore(storage, "drafts", validDraft, options);
  return {
    load: () => store.entries(),
    save: (conversation, draft) => {
      const valid = draft && validDraft(draft);
      if (!valid) { store.delete(conversation); return "cleared"; }
      return store.put(conversation, valid);
    },
  };
}

/**
 * One of the operator's messages that has not settled (cas-e7b1): `held`
 * waits in this browser and has never been on the wire; `sending` went out
 * and its receipt has not come; `unconfirmed` gave up on its receipt; `error`
 * was not sent. A confirmed, answered, replaced or dismissed message is never
 * stored.
 */
export type PendingSend = {
  id: string;
  target: string;
  text: string;
  state: "held" | "sending" | "unconfirmed" | "error";
  /** When it was written (ms epoch): its place in the thread. */
  at: number;
  /** When it was first held (ms epoch; `held` only): its wait runs out from here. */
  heldAt?: number;
  /** When it went on the wire (ms epoch). */
  sentAt?: number;
  replyTo?: number;
  error?: string;
  session?: string;
};

const PENDING_STATES = new Set<PendingSend["state"]>(["held", "sending", "unconfirmed", "error"]);
/** At most this many unsettled messages are kept per conversation, the newest. */
export const MAX_PENDING_SENDS = 20;
/**
 * Field bounds for a stored unsettled message (cas-8f19), checked on read as
 * well as write. The text bound is the store's per-conversation bound: a
 * longer message is not stored, so a longer stored one is not this page's.
 */
export const PENDING_SEND_BOUNDS = { id: 128, target: 256, text: 64_000, error: 2_000, session: 256 } as const;

const finite = (value: unknown): number | undefined => (typeof value === "number" && Number.isFinite(value) ? value : undefined);

/** One stored unsettled message, or undefined when malformed. */
export function validPendingSend(raw: unknown): PendingSend | undefined {
  if (!raw || typeof raw !== "object") return undefined;
  const { id, target, text, state, at, heldAt, sentAt, replyTo, error, session } = raw as Record<string, unknown>;
  if (typeof id !== "string" || !id || typeof target !== "string" || typeof text !== "string" || !text.trim()) return undefined;
  const B = PENDING_SEND_BOUNDS;
  if (id.length > B.id || target.length > B.target || text.length > B.text) return undefined;
  if ((typeof error === "string" && error.length > B.error) || (typeof session === "string" && session.length > B.session)) return undefined;
  if (typeof state !== "string" || !PENDING_STATES.has(state as PendingSend["state"])) return undefined;
  const when = finite(at);
  if (when === undefined) return undefined;
  return {
    id, target, text, state: state as PendingSend["state"], at: when,
    ...(finite(heldAt) === undefined ? {} : { heldAt: finite(heldAt) }),
    ...(finite(sentAt) === undefined ? {} : { sentAt: finite(sentAt) }),
    ...(finite(replyTo) === undefined ? {} : { replyTo: finite(replyTo) }),
    ...(typeof error === "string" ? { error } : {}),
    ...(typeof session === "string" ? { session } : {}),
  };
}

/** A conversation's stored unsettled messages; malformed ones are dropped, the rest kept. */
export function validPendingSends(raw: unknown): PendingSend[] | undefined {
  if (!Array.isArray(raw)) return undefined;
  const sends = raw.flatMap((send) => validPendingSend(send) ?? []).slice(-MAX_PENDING_SENDS);
  return sends.length ? sends : undefined;
}

/** The unsettled-messages store (cas-e7b1): an empty list is a deletion. */
export function pendingSendStore(storage: StorageLike | undefined): {
  load(): Map<string, PendingSend[]>;
  save(conversation: string, sends: PendingSend[]): void;
} {
  const store = new ConversationStore(storage, "sends", validPendingSends);
  return {
    load: () => store.entries(),
    save: (conversation, sends) => {
      const kept = sends.flatMap((send) => validPendingSend(send) ?? []).slice(-MAX_PENDING_SENDS);
      if (kept.length) store.set(conversation, kept); else store.delete(conversation);
    },
  };
}

/**
 * When this browser first saw each of a conversation's turns (cas-8d52): a
 * supervisor turn's arrival (`r:<notification id>`) and the operator's own
 * message's send time (`s:<notification id>`), in ms epoch, plus the machine
 * clock's measured lead over this browser (`skew`, ms; positive when ahead).
 * A reload rebuilds the thread from the machine's stamps; these keep each turn
 * at the time the visit showed it instead of the moment of the reload.
 * `live` names the supervisor turns this browser saw arrive live, and whether
 * the visit marked each "machine clock ahead" (cas-9e33): a reload shows the
 * same mark, even when the visit could not yet know the machine's lead.
 */
export type Arrivals = { skew?: number; at: Record<string, number>; live?: Record<string, boolean> };

/** At most this many turns' times are kept per conversation, the newest. */
export const MAX_ARRIVALS = 400;

const ARRIVAL_KEY = /^[rs]:\d+$/;

export function validArrivals(raw: unknown): Arrivals | undefined {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return undefined;
  const { skew, at } = raw as Record<string, unknown>;
  const times: Record<string, number> = {};
  if (at && typeof at === "object" && !Array.isArray(at)) {
    const entries = Object.entries(at as Record<string, unknown>)
      .flatMap(([key, value]) => ARRIVAL_KEY.test(key) && finite(value) !== undefined ? [[key, value as number] as const] : [])
      .sort(([, a], [, b]) => a - b)
      .slice(-MAX_ARRIVALS);
    for (const [key, value] of entries) times[key] = value;
  }
  const lead = finite(skew);
  if (!Object.keys(times).length && lead === undefined) return undefined;
  const marks: Record<string, boolean> = {};
  const live = (raw as Record<string, unknown>).live;
  if (live && typeof live === "object" && !Array.isArray(live)) {
    for (const [key, value] of Object.entries(live as Record<string, unknown>)) if (key in times && key.startsWith("r:") && typeof value === "boolean") marks[key] = value;
  }
  return { ...(lead === undefined ? {} : { skew: lead }), at: times, ...(Object.keys(marks).length ? { live: marks } : {}) };
}

/** The turn-times store (cas-8d52): an empty record is a deletion. */
export function arrivalStore(storage: StorageLike | undefined): {
  load(): Map<string, Arrivals>;
  save(conversation: string, arrivals: Arrivals | undefined): void;
} {
  const store = new ConversationStore(storage, "arrivals", validArrivals);
  return {
    load: () => store.entries(),
    save: (conversation, arrivals) => {
      const valid = arrivals && validArrivals(arrivals);
      if (valid) store.set(conversation, valid); else store.delete(conversation);
    },
  };
}

/**
 * cas-97d58 F14: the newest supervisor reply the operator has seen in each
 * conversation, so a reload that replays history does not count replies
 * already read as unread again. Purged with the machine like every
 * conversation namespace.
 */
export function readMarkStore(storage: StorageLike | undefined): {
  load(): Map<string, number>;
  save(conversation: string, mark: number): void;
} {
  const store = new ConversationStore(storage, "read", (raw) => typeof raw === "number" && Number.isSafeInteger(raw) && raw >= 0 ? raw : undefined);
  return {
    load: () => store.entries(),
    save: (conversation, mark) => { store.set(conversation, mark); },
  };
}
