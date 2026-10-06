// Durable browser state of the operator inbox (cas-9b7d S1/S3).
//
// One IndexedDB database per profile, `cas-operator-inbox-v1`. Each profile
// is its own device: its own keys, grant, cursor and ACKs; nothing here is
// shared with or inherited from another installation (contract §8.3).
//
// The page commit is the one transaction that matters: the decrypted events
// of a replay page and the cursor that page proves are written together, and
// only after it commits does the caller send the persisted ACK (§8.2). A
// quota or abort error leaves both the cursor and the ACK state unchanged.
//
// `MemoryInboxStore` implements the same contract for unit tests; the
// IndexedDB implementation is exercised by real-Chromium journeys.

import type { SigningKey } from "./pop";
import type { Grant } from "./wire";

export interface InboxIdentity {
  cloudOrigin: string;
  accountId: string;
  deviceId: string;
  grant: Grant;
  signingKey: SigningKey;
  /** Device encryption key pair (private half non-extractable). */
  encryption: CryptoKeyPair;
  feedGeneration: string;
  enrolledAt: string;
  label: string;
  emailHint: string | null;
}

export interface PendingEnrollment {
  cloudOrigin: string;
  enrollmentId: string;
  userCode: string;
  pollSecret: string;
  approvalUrl: string;
  expiresAt: string;
  intervalS: number;
  check: { enc: string; ct: string };
  signingKey: SigningKey;
  encryption: CryptoKeyPair;
  label: string;
  createdAt: string;
}

export interface EpochKey {
  accountId: string;
  feedGeneration: string;
  epoch: string;
  status: string;
  /** Raw 65-byte epoch public key from the verified manifest. */
  publicRaw: Uint8Array;
  pair: CryptoKeyPair;
}

export type EventVerification = "verified" | "unverified" | "undecryptable";

export interface InboxEvent {
  accountId: string;
  feedGeneration: string;
  sequence: string;
  eventId: string;
  scope: "session" | "machine";
  producerKind: "principal" | "cloud_observer";
  hubId: string;
  projectId: string | null;
  sessionId: string | null;
  keyEpoch: string;
  digest: string;
  storedAt: string;
  expiresAt: string;
  /** Decrypted plaintext JSON; null unless `verification === "verified"`. */
  plaintext: unknown;
  verification: EventVerification;
  /** Closed local reason when not verified; never rendered as content. */
  failure: string | null;
  acked: boolean;
}

export interface ExpiredInterval {
  from: string;
  to: string;
  reason: string;
}

export interface FeedCursor {
  accountId: string;
  feedGeneration: string;
  /** Highest sequence covered locally (events stored or intervals recorded). */
  cursor: string;
  acceptedExpiredThrough: string | null;
  /** Gaps this device accepted; shown as "History expired" markers. */
  expired: ExpiredInterval[];
  /** Highest cursor the cloud has acknowledged for this device. */
  syncedCursor: string | null;
}

export interface ReadMark {
  accountId: string;
  hubId: string;
  projectId: string;
  sessionId: string;
  sequence: string;
  updatedAt: string;
}

export type CommandState =
  | "sealing"
  | "submitting"
  | "pending_machine"
  | "reserved"
  | "accepted"
  | "rejected_by_machine"
  | "cancelled"
  | "expired"
  | "refused";

export interface QueuedCommand {
  commandId: string;
  accountId: string;
  machineId: string;
  hubId: string;
  projectId: string;
  sessionId: string;
  /** Readable session name, local only (sealed inside the ciphertext). */
  sessionName: string;
  /** The exact request body, sealed once; every retry sends these bytes. */
  request: Record<string, unknown> | null;
  /** Plaintext shown locally until the history event replays. */
  body: string;
  historyEventId: string;
  state: CommandState;
  reason: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface PageCommit {
  accountId: string;
  feedGeneration: string;
  events: InboxEvent[];
  /** Intervals this page proved expired (already validated). */
  intervals: ExpiredInterval[];
  /** The page's `next_cursor`. */
  cursor: string;
  /** Set when a 410 gap was accepted before this page. */
  acceptedExpiredThrough?: string;
}

export interface InboxStore {
  loadIdentity(): Promise<InboxIdentity | null>;
  saveIdentity(identity: InboxIdentity): Promise<void>;
  loadPending(): Promise<PendingEnrollment | null>;
  savePending(pending: PendingEnrollment | null): Promise<void>;

  /** Replace this generation's epoch keys; refuses a lower policy version (§6.2). */
  saveEpochKeys(accountId: string, feedGeneration: string, policyVersion: string, keys: EpochKey[]): Promise<void>;
  epochKey(accountId: string, feedGeneration: string, epoch: string): Promise<EpochKey | null>;
  policyVersion(accountId: string): Promise<string | null>;

  loadCursor(accountId: string, feedGeneration: string): Promise<FeedCursor | null>;
  /** Atomically store page events (deduplicated by event ID) and advance the cursor. */
  commitPage(page: PageCommit): Promise<{ stored: InboxEvent[] }>;
  markSynced(accountId: string, feedGeneration: string, cursor: string): Promise<void>;
  markAcked(accountId: string, eventIds: string[]): Promise<void>;
  unacked(accountId: string, feedGeneration: string): Promise<InboxEvent[]>;
  events(accountId: string, feedGeneration: string): Promise<InboxEvent[]>;

  saveReadMark(mark: ReadMark): Promise<ReadMark>;
  readMarks(accountId: string): Promise<ReadMark[]>;

  saveCommand(command: QueuedCommand): Promise<void>;
  commands(accountId: string): Promise<QueuedCommand[]>;

  /** Remove everything owned by the account (sign-out, revoke, reset). */
  wipe(accountId: string): Promise<void>;
}

function compareDecimal(a: string, b: string): number {
  const left = BigInt(a);
  const right = BigInt(b);
  return left < right ? -1 : left > right ? 1 : 0;
}

export function maxDecimal(a: string, b: string): string {
  return compareDecimal(a, b) >= 0 ? a : b;
}

// --------------------------------------------------------------- in memory

export class MemoryInboxStore implements InboxStore {
  identity: InboxIdentity | null = null;
  pending: PendingEnrollment | null = null;
  private readonly epochs = new Map<string, EpochKey>();
  private readonly policies = new Map<string, string>();
  private readonly cursors = new Map<string, FeedCursor>();
  private readonly stored = new Map<string, InboxEvent>();
  private readonly marks = new Map<string, ReadMark>();
  private readonly queued = new Map<string, QueuedCommand>();
  /** Fail the next page commit (quota simulation). */
  failNextCommit: Error | null = null;

  async loadIdentity() {
    return this.identity;
  }
  async saveIdentity(identity: InboxIdentity) {
    this.identity = identity;
  }
  async loadPending() {
    return this.pending;
  }
  async savePending(pending: PendingEnrollment | null) {
    this.pending = pending;
  }

  async saveEpochKeys(accountId: string, feedGeneration: string, policyVersion: string, keys: EpochKey[]) {
    const previous = this.policies.get(accountId);
    if (previous && compareDecimal(policyVersion, previous) < 0) throw new Error("policy_version_regression");
    this.policies.set(accountId, policyVersion);
    for (const [key, value] of this.epochs) {
      if (value.accountId === accountId && value.feedGeneration !== feedGeneration) this.epochs.delete(key);
    }
    for (const key of keys) this.epochs.set(`${accountId}|${feedGeneration}|${key.epoch}`, key);
  }
  async epochKey(accountId: string, feedGeneration: string, epoch: string) {
    return this.epochs.get(`${accountId}|${feedGeneration}|${epoch}`) ?? null;
  }
  async policyVersion(accountId: string) {
    return this.policies.get(accountId) ?? null;
  }

  async loadCursor(accountId: string, feedGeneration: string) {
    const cursor = this.cursors.get(`${accountId}|${feedGeneration}`);
    return cursor ? structuredClone(cursor) : null;
  }

  async commitPage(page: PageCommit) {
    if (this.failNextCommit) {
      const failure = this.failNextCommit;
      this.failNextCommit = null;
      throw failure;
    }
    const key = `${page.accountId}|${page.feedGeneration}`;
    const current = this.cursors.get(key);
    if (current && compareDecimal(page.cursor, current.cursor) < 0) throw new Error("cursor_regression");
    const stored: InboxEvent[] = [];
    const staged = new Map(this.stored);
    for (const event of page.events) {
      const id = `${event.accountId}|${event.eventId}`;
      if (staged.has(id)) continue;
      staged.set(id, structuredClone(event));
      stored.push(event);
    }
    this.stored.clear();
    for (const [id, event] of staged) this.stored.set(id, event);
    this.cursors.set(key, {
      accountId: page.accountId,
      feedGeneration: page.feedGeneration,
      cursor: page.cursor,
      acceptedExpiredThrough: page.acceptedExpiredThrough ?? current?.acceptedExpiredThrough ?? null,
      expired: [...(current?.expired ?? []), ...page.intervals],
      syncedCursor: current?.syncedCursor ?? null,
    });
    return { stored };
  }

  async markSynced(accountId: string, feedGeneration: string, cursor: string) {
    const current = this.cursors.get(`${accountId}|${feedGeneration}`);
    if (current) current.syncedCursor = current.syncedCursor ? maxDecimal(current.syncedCursor, cursor) : cursor;
  }

  async markAcked(accountId: string, eventIds: string[]) {
    for (const eventId of eventIds) {
      const event = this.stored.get(`${accountId}|${eventId}`);
      if (event) event.acked = true;
    }
  }

  async unacked(accountId: string, feedGeneration: string) {
    return [...this.stored.values()].filter(
      (event) => event.accountId === accountId && event.feedGeneration === feedGeneration && event.verification === "verified" && !event.acked,
    );
  }

  async events(accountId: string, feedGeneration: string) {
    return [...this.stored.values()]
      .filter((event) => event.accountId === accountId && event.feedGeneration === feedGeneration)
      .sort((a, b) => compareDecimal(a.sequence, b.sequence));
  }

  async saveReadMark(mark: ReadMark) {
    const key = `${mark.accountId}|${mark.hubId}|${mark.projectId}|${mark.sessionId}`;
    const current = this.marks.get(key);
    const next = current && compareDecimal(current.sequence, mark.sequence) >= 0 ? current : mark;
    this.marks.set(key, next);
    return next;
  }
  async readMarks(accountId: string) {
    return [...this.marks.values()].filter((mark) => mark.accountId === accountId);
  }

  async saveCommand(command: QueuedCommand) {
    this.queued.set(command.commandId, structuredClone(command));
  }
  async commands(accountId: string) {
    return [...this.queued.values()].filter((command) => command.accountId === accountId).map((command) => structuredClone(command));
  }

  async wipe(accountId: string) {
    if (this.identity?.accountId === accountId) this.identity = null;
    this.pending = null;
    for (const map of [this.epochs, this.cursors, this.stored, this.marks, this.queued] as Map<string, { accountId: string }>[]) {
      for (const [key, value] of map) if (value.accountId === accountId) map.delete(key);
    }
    this.policies.delete(accountId);
  }
}

// --------------------------------------------------------------- IndexedDB

const DB_NAME = "cas-operator-inbox-v1";
const DB_VERSION = 1;
const STORES = ["identity", "epochs", "policies", "cursors", "events", "readMarks", "commands"] as const;
type StoreName = (typeof STORES)[number];

function promisify<T>(request: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

export class IndexedDbInboxStore implements InboxStore {
  private db: Promise<IDBDatabase> | null = null;

  constructor(private readonly factory: IDBFactory = indexedDB, private readonly name = DB_NAME) {}

  private open(): Promise<IDBDatabase> {
    this.db ??= new Promise((resolve, reject) => {
      const request = this.factory.open(this.name, DB_VERSION);
      request.onupgradeneeded = () => {
        const db = request.result;
        if (!db.objectStoreNames.contains("identity")) db.createObjectStore("identity");
        if (!db.objectStoreNames.contains("epochs")) {
          const epochs = db.createObjectStore("epochs", { keyPath: ["accountId", "feedGeneration", "epoch"] });
          epochs.createIndex("account", "accountId");
        }
        if (!db.objectStoreNames.contains("policies")) db.createObjectStore("policies");
        if (!db.objectStoreNames.contains("cursors")) {
          const cursors = db.createObjectStore("cursors", { keyPath: ["accountId", "feedGeneration"] });
          cursors.createIndex("account", "accountId");
        }
        if (!db.objectStoreNames.contains("events")) {
          const events = db.createObjectStore("events", { keyPath: ["accountId", "eventId"] });
          events.createIndex("account", "accountId");
          events.createIndex("generation", ["accountId", "feedGeneration"]);
        }
        if (!db.objectStoreNames.contains("readMarks")) {
          const marks = db.createObjectStore("readMarks", { keyPath: ["accountId", "hubId", "projectId", "sessionId"] });
          marks.createIndex("account", "accountId");
        }
        if (!db.objectStoreNames.contains("commands")) {
          const commands = db.createObjectStore("commands", { keyPath: "commandId" });
          commands.createIndex("account", "accountId");
        }
      };
      request.onsuccess = () => {
        const db = request.result;
        // Another tab upgraded or deleted the database: drop this handle.
        db.onversionchange = () => {
          db.close();
          this.db = null;
        };
        resolve(db);
      };
      request.onerror = () => {
        this.db = null;
        reject(request.error);
      };
    });
    return this.db;
  }

  /** Run `work` in one transaction and resolve only after it commits. */
  private async tx<T>(names: StoreName[], mode: IDBTransactionMode, work: (tx: IDBTransaction) => Promise<T> | T): Promise<T> {
    const db = await this.open();
    const tx = db.transaction(names, mode, { durability: "strict" } as IDBTransactionOptions);
    const done = new Promise<void>((resolve, reject) => {
      tx.oncomplete = () => resolve();
      tx.onabort = () => reject(tx.error ?? new DOMException("Inbox transaction aborted", "AbortError"));
      tx.onerror = () => reject(tx.error);
    });
    let result: T;
    try {
      result = await work(tx);
    } catch (error) {
      try {
        tx.abort();
      } catch {
        /* already finished */
      }
      await done.catch(() => undefined);
      throw error;
    }
    await done;
    return result;
  }

  async loadIdentity() {
    return this.tx(["identity"], "readonly", async (tx) => (await promisify(tx.objectStore("identity").get("current"))) ?? null);
  }
  async saveIdentity(identity: InboxIdentity) {
    await this.tx(["identity"], "readwrite", (tx) => promisify(tx.objectStore("identity").put(identity, "current")));
  }
  async loadPending() {
    return this.tx(["identity"], "readonly", async (tx) => (await promisify(tx.objectStore("identity").get("pending"))) ?? null);
  }
  async savePending(pending: PendingEnrollment | null) {
    await this.tx(["identity"], "readwrite", (tx) =>
      pending ? promisify(tx.objectStore("identity").put(pending, "pending")) : promisify(tx.objectStore("identity").delete("pending")),
    );
  }

  async saveEpochKeys(accountId: string, feedGeneration: string, policyVersion: string, keys: EpochKey[]) {
    await this.tx(["epochs", "policies"], "readwrite", async (tx) => {
      const policies = tx.objectStore("policies");
      const previous = (await promisify(policies.get(accountId))) as string | undefined;
      if (previous && compareDecimal(policyVersion, previous) < 0) throw new Error("policy_version_regression");
      policies.put(policyVersion, accountId);
      const epochs = tx.objectStore("epochs");
      const existing = (await promisify(epochs.index("account").getAll(accountId))) as EpochKey[];
      for (const key of existing) {
        if (key.feedGeneration !== feedGeneration) epochs.delete([key.accountId, key.feedGeneration, key.epoch]);
      }
      for (const key of keys) epochs.put(key);
    });
  }
  async epochKey(accountId: string, feedGeneration: string, epoch: string) {
    return this.tx(["epochs"], "readonly", async (tx) => (await promisify(tx.objectStore("epochs").get([accountId, feedGeneration, epoch]))) ?? null);
  }
  async policyVersion(accountId: string) {
    return this.tx(["policies"], "readonly", async (tx) => (await promisify(tx.objectStore("policies").get(accountId))) ?? null);
  }

  async loadCursor(accountId: string, feedGeneration: string) {
    return this.tx(["cursors"], "readonly", async (tx) => (await promisify(tx.objectStore("cursors").get([accountId, feedGeneration]))) ?? null);
  }

  async commitPage(page: PageCommit) {
    return this.tx(["events", "cursors"], "readwrite", async (tx) => {
      const cursors = tx.objectStore("cursors");
      const current = (await promisify(cursors.get([page.accountId, page.feedGeneration]))) as FeedCursor | undefined;
      if (current && compareDecimal(page.cursor, current.cursor) < 0) throw new Error("cursor_regression");
      const events = tx.objectStore("events");
      const stored: InboxEvent[] = [];
      for (const event of page.events) {
        const existing = await promisify(events.get([event.accountId, event.eventId]));
        if (existing) continue;
        events.put(event);
        stored.push(event);
      }
      const next: FeedCursor = {
        accountId: page.accountId,
        feedGeneration: page.feedGeneration,
        cursor: page.cursor,
        acceptedExpiredThrough: page.acceptedExpiredThrough ?? current?.acceptedExpiredThrough ?? null,
        expired: [...(current?.expired ?? []), ...page.intervals],
        syncedCursor: current?.syncedCursor ?? null,
      };
      cursors.put(next);
      return { stored };
    });
  }

  async markSynced(accountId: string, feedGeneration: string, cursor: string) {
    await this.tx(["cursors"], "readwrite", async (tx) => {
      const cursors = tx.objectStore("cursors");
      const current = (await promisify(cursors.get([accountId, feedGeneration]))) as FeedCursor | undefined;
      if (!current) return;
      current.syncedCursor = current.syncedCursor ? maxDecimal(current.syncedCursor, cursor) : cursor;
      cursors.put(current);
    });
  }

  async markAcked(accountId: string, eventIds: string[]) {
    await this.tx(["events"], "readwrite", async (tx) => {
      const events = tx.objectStore("events");
      for (const eventId of eventIds) {
        const event = (await promisify(events.get([accountId, eventId]))) as InboxEvent | undefined;
        if (event && !event.acked) events.put({ ...event, acked: true });
      }
    });
  }

  async unacked(accountId: string, feedGeneration: string) {
    return (await this.events(accountId, feedGeneration)).filter((event) => event.verification === "verified" && !event.acked);
  }

  async events(accountId: string, feedGeneration: string) {
    const rows = await this.tx(["events"], "readonly", (tx) =>
      promisify(tx.objectStore("events").index("generation").getAll([accountId, feedGeneration])),
    );
    return (rows as InboxEvent[]).sort((a, b) => compareDecimal(a.sequence, b.sequence));
  }

  async saveReadMark(mark: ReadMark) {
    return this.tx(["readMarks"], "readwrite", async (tx) => {
      const marks = tx.objectStore("readMarks");
      const current = (await promisify(marks.get([mark.accountId, mark.hubId, mark.projectId, mark.sessionId]))) as ReadMark | undefined;
      const next = current && compareDecimal(current.sequence, mark.sequence) >= 0 ? current : mark;
      if (next !== current) marks.put(next);
      return next;
    });
  }
  async readMarks(accountId: string) {
    return this.tx(["readMarks"], "readonly", (tx) => promisify(tx.objectStore("readMarks").index("account").getAll(accountId))) as Promise<ReadMark[]>;
  }

  async saveCommand(command: QueuedCommand) {
    await this.tx(["commands"], "readwrite", (tx) => promisify(tx.objectStore("commands").put(command)));
  }
  async commands(accountId: string) {
    return this.tx(["commands"], "readonly", (tx) => promisify(tx.objectStore("commands").index("account").getAll(accountId))) as Promise<QueuedCommand[]>;
  }

  async wipe(accountId: string) {
    await this.tx([...STORES], "readwrite", async (tx) => {
      const identity = tx.objectStore("identity");
      const current = (await promisify(identity.get("current"))) as InboxIdentity | undefined;
      if (current?.accountId === accountId) identity.delete("current");
      identity.delete("pending");
      tx.objectStore("policies").delete(accountId);
      for (const name of ["epochs", "cursors", "events", "readMarks", "commands"] as const) {
        const store = tx.objectStore(name);
        const keys = await promisify(store.index("account").getAllKeys(accountId));
        for (const key of keys) store.delete(key);
      }
    });
  }
}
