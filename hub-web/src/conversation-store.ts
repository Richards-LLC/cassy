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

  /** Store one conversation's value, evicting the least recently written beyond the bound. */
  set(conversation: string, value: T): void {
    const records = this.read();
    if (JSON.stringify(value).length > this.maxValueChars) {
      records.delete(conversation);
    } else {
      records.delete(conversation);
      records.set(conversation, { value, updatedAt: this.now() });
    }
    const ordered = [...records].sort(([, a], [, b]) => a.updatedAt - b.updatedAt);
    while (ordered.length > this.maxConversations) ordered.shift();
    this.write(new Map(ordered));
  }

  delete(conversation: string): void {
    const records = this.read();
    if (!records.delete(conversation)) return;
    this.write(records);
  }

  private read(): Map<string, Record_<T>> {
    const records = new Map<string, Record_<T>>();
    let raw: string | null = null;
    try { raw = this.storage?.getItem(this.key) ?? null; } catch { return records; }
    if (!raw) return records;
    let parsed: unknown;
    try { parsed = JSON.parse(raw); } catch { return records; }
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return records;
    for (const [conversation, entry] of Object.entries(parsed as Record<string, unknown>)) {
      if (!entry || typeof entry !== "object") continue;
      const { value, updatedAt } = entry as { value?: unknown; updatedAt?: unknown };
      const narrowed = this.validate(value);
      if (narrowed === undefined) continue;
      records.set(conversation, { value: narrowed, updatedAt: typeof updatedAt === "number" && Number.isFinite(updatedAt) ? updatedAt : 0 });
    }
    return records;
  }

  private write(records: Map<string, Record_<T>>): void {
    if (!this.storage) return;
    try {
      if (records.size) this.storage.setItem(this.key, JSON.stringify(Object.fromEntries(records)));
      else this.storage.removeItem(this.key);
    } catch { /* full or denied: the in-memory state still holds for this page */ }
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

/** The drafts store: a blank draft is a deletion, never a stored entry. */
export function draftStore(storage: StorageLike | undefined): {
  load(): Map<string, Draft>;
  save(conversation: string, draft: Draft | undefined): void;
} {
  const store = new ConversationStore(storage, "drafts", validDraft);
  return {
    load: () => store.entries(),
    save: (conversation, draft) => {
      const valid = draft && validDraft(draft);
      if (valid) store.set(conversation, valid); else store.delete(conversation);
    },
  };
}
