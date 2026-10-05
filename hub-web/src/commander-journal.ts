import { MAX_PENDING_SENDS, validPendingSend, type PendingSend } from "./conversation-store";
import type { OperatorReply, StoredMachine } from "./types";

/** Credentials rotate; this installation/conversation identity does not. */
export type DeliveryScope = { hub: string; baseUrl: string; device: string; session: string; accountState?: "unenrolled" | "unsupported" };
export type CredentialFence = { credentialId: string; generation: number };
export const DELIVERY_DB = "cas-commander-delivery-v1";
export const scopeKey = (scope: DeliveryScope): string => JSON.stringify([scope.hub, scope.baseUrl, scope.device, scope.session]);
export function deliveryScope(machine: StoredMachine, session: string): DeliveryScope {
  const enrollment = (machine as StoredMachine & { accountEnrollment?: { state?: unknown } }).accountEnrollment;
  // Legacy and explicitly unenrolled installations share this direct journal.
  // Enrolled account/feed identities need their own authenticated contract.
  const accountState = enrollment === undefined || enrollment.state === "unenrolled" ? "unenrolled" : "unsupported";
  return { hub: machine.id, baseUrl: machine.baseUrl, device: machine.deviceId, session, accountState };
}
export function credentialFence(machine: StoredMachine): CredentialFence {
  return { credentialId: machine.credentialId, generation: (machine as StoredMachine & { credentialGeneration?: number }).credentialGeneration ?? 0 };
}
export type JournalSend = { key: string; scope: DeliveryScope; send?: PendingSend; revision: number; updatedAt: number; flight?: CredentialFence; owner?: string };
export type StoredReply = { key: string; scope: DeliveryScope; reply: OperatorReply; persistedAt: number };
type Block = { key: string; credentialId: string };
const itemKey = (scope: DeliveryScope, id: string | number) => JSON.stringify([scopeKey(scope), id]);
const blockKey = (hub: string, fence: CredentialFence) => JSON.stringify(["revoked", hub, fence.credentialId]);
const sameFence = (a: CredentialFence, b: CredentialFence) => a.credentialId === b.credentialId && a.generation === b.generation;

const RETENTION_MS = 90 * 24 * 60 * 60 * 1_000;
const object = (value: unknown): value is Record<string, unknown> => value !== null && typeof value === "object" && !Array.isArray(value);
const finiteTime = (value: unknown): value is number => typeof value === "number" && Number.isFinite(value) && value >= 0;
const boundedText = (value: unknown, limit = 64_000): value is string => typeof value === "string" && value.length <= limit;
function validScope(value: unknown): value is DeliveryScope {
  return object(value) && ["hub", "baseUrl", "device", "session"].every(key => boundedText(value[key], 2_000) && Boolean(value[key]))
    && (value.accountState === undefined || value.accountState === "unenrolled");
}
function validFence(value: unknown): value is CredentialFence {
  return object(value) && boundedText(value.credentialId, 2_000) && Boolean(value.credentialId)
    && Number.isSafeInteger(value.generation) && (value.generation as number) >= 0;
}
function validSendRow(value: unknown): value is JournalSend {
  return object(value) && validScope(value.scope) && typeof value.key === "string"
    && Number.isSafeInteger(value.revision) && (value.revision as number) > 0 && finiteTime(value.updatedAt)
    && (value.flight === undefined || validFence(value.flight)) && (value.owner === undefined || boundedText(value.owner, 2_000))
    && (value.send === undefined || (Boolean(validPendingSend(value.send)) && value.key === itemKey(value.scope, (value.send as PendingSend).id)));
}
function normalizedReply(value: unknown): OperatorReply | undefined {
  if (!object(value) || !Number.isSafeInteger(value.notification_id) || (value.notification_id as number) <= 0
    || !(value.reply_to === null || (Number.isSafeInteger(value.reply_to) && (value.reply_to as number) > 0))
    || !boundedText(value.message) || !boundedText(value.summary) || !boundedText(value.device_id, 2_000)) return undefined;
  const kind = value.kind ?? "answer";
  if (!["answer", "status", "receipt", "ask", "blocker"].includes(kind as string)) return undefined;
  const attachments = value.attachments ?? [];
  if (!Array.isArray(attachments) || attachments.some(a => !object(a)
    || !["artifact_id", "name", "mime", "sha256"].every(key => boundedText(a[key], 2_000))
    || !Number.isSafeInteger(a.size_bytes) || (a.size_bytes as number) < 0)) return undefined;
  if (value.options !== undefined && (!Array.isArray(value.options) || value.options.some(option => !boundedText(option)))) return undefined;
  if (value.operator_label !== undefined && !boundedText(value.operator_label, 2_000)) return undefined;
  if (value.reply_to_session !== undefined && value.reply_to_session !== null && !boundedText(value.reply_to_session, 2_000)) return undefined;
  if (value.notice !== undefined && value.notice !== null && (!object(value.notice) || !boundedText(value.notice.source, 2_000)
    || (value.notice.subject !== undefined && !Number.isSafeInteger(value.notice.subject))
    || (value.notice.resolved !== undefined && typeof value.notice.resolved !== "boolean"))) return undefined;
  const reply: OperatorReply = {
    notification_id: value.notification_id as number, reply_to: value.reply_to as number | null,
    message: value.message, summary: value.summary, device_id: value.device_id, kind: kind as OperatorReply["kind"],
    attachments: attachments.map(a => ({ artifact_id: a.artifact_id, name: a.name, mime: a.mime, size_bytes: a.size_bytes, sha256: a.sha256 })),
    ...(value.operator_label === undefined ? {} : { operator_label: value.operator_label as string }),
    ...(value.options === undefined || !(value.options as string[]).length ? {} : { options: value.options as string[] }),
    ...(value.reply_to_session === undefined || value.reply_to_session === null ? {} : { reply_to_session: value.reply_to_session as string }),
    ...(value.notice === undefined || value.notice === null ? {} : { notice: value.notice as unknown as OperatorReply["notice"] }),
  };
  return JSON.stringify(reply).length <= 64_000 ? reply : undefined;
}
function validReplyRow(value: unknown): value is StoredReply {
  return object(value) && validScope(value.scope) && finiteTime(value.persistedAt)
    && Boolean(normalizedReply(value.reply)) && value.key === itemKey(value.scope, (value.reply as OperatorReply).notification_id);
}
function survivingRows<T extends { key: string }>(store: IDBObjectStore, rows: unknown[], valid: (row: unknown) => row is T, stamp: (row: T) => number, now: number): T[] {
  return rows.filter((row): row is T => {
    if (valid(row) && now - stamp(row) <= RETENTION_MS) return true;
    if (object(row) && row.key !== undefined) store.delete(row.key as IDBValidKey);
    return false;
  });
}

/**
 * Per-item transaction journal. A dispatch claim is durable BEFORE a socket
 * write. There is deliberately no lease expiry/reclaim: uncertain writes are
 * never automatically executed twice. Tombstones fence stale tab snapshots.
 * Broadcasts are invalidation hints, never permission to send.
 */
export class CommanderJournal {
  private readonly owner = crypto.randomUUID();
  private readonly observed = new Map<string, number>();
  private readonly channel: BroadcastChannel | undefined;
  onChange?: () => void;
  constructor(
    private readonly factory: IDBFactory | undefined,
    private readonly current: (scope: DeliveryScope) => Promise<CredentialFence | undefined>,
    private readonly now: () => number = Date.now,
    broadcast = true,
  ) {
    this.channel = broadcast && typeof BroadcastChannel !== "undefined" ? new BroadcastChannel(DELIVERY_DB) : undefined;
    if (this.channel) this.channel.onmessage = () => this.onChange?.();
  }
  close(): void { this.channel?.close(); }
  private changed(): void { this.channel?.postMessage("changed"); }
  private open(): Promise<IDBDatabase> {
    return new Promise((resolve, reject) => {
      if (!this.factory) { reject(new Error("Browser storage is unavailable.")); return; }
      const request = this.factory.open(DELIVERY_DB, 1);
      let settled = false;
      const timeout = setTimeout(() => { settled = true; reject(new Error("Browser storage did not answer.")); }, 5_000);
      request.onupgradeneeded = () => {
        for (const name of ["sends", "replies", "blocks"]) request.result.createObjectStore(name, { keyPath: "key" });
      };
      request.onsuccess = () => {
        clearTimeout(timeout);
        if (settled) { request.result.close(); return; }
        settled = true;
        resolve(request.result);
      };
      request.onerror = () => { clearTimeout(timeout); settled = true; reject(request.error); };
    });
  }
  private async transaction<T>(names: string[], mode: IDBTransactionMode, run: (tx: IDBTransaction, result: (value: T) => void) => void): Promise<T> {
    const db = await this.open();
    return new Promise((resolve, reject) => {
      let tx: IDBTransaction;
      try { tx = db.transaction(names, mode, mode === "readwrite" ? { durability: "strict" } : undefined); }
      catch (error) { db.close(); reject(error); return; }
      let value: T;
      const timer = setTimeout(() => { try { tx.abort(); } catch { /* completed */ } }, 5_000);
      const cleanup = () => { clearTimeout(timer); db.close(); };
      tx.oncomplete = () => { cleanup(); resolve(value!); };
      tx.onabort = () => { cleanup(); reject(tx.error ?? new Error("Browser storage transaction was cancelled.")); };
      tx.onerror = () => { /* onabort is the authoritative failed commit */ };
      try { run(tx, (next) => { value = next; }); } catch (error) { tx.abort(); cleanup(); reject(error); }
    });
  }
  async read(scope: DeliveryScope): Promise<{ sends: PendingSend[]; replies: StoredReply[] }> {
    if (!validScope(scope)) return { sends: [], replies: [] };
    return this.transaction<{ sends: PendingSend[]; replies: StoredReply[] }>(["sends", "replies"], "readwrite", (tx, done) => {
      const sendStore = tx.objectStore("sends"), replyStore = tx.objectStore("replies");
      const sends = sendStore.getAll(), replies = replyStore.getAll();
      let pending: PendingSend[] = [], stored: StoredReply[] = [];
      sends.onsuccess = () => {
        for (const row of survivingRows(sendStore, sends.result, validSendRow, row => row.updatedAt, this.now())) {
          if (scopeKey(row.scope) !== scopeKey(scope)) continue;
          this.observed.set(row.key, row.revision);
          if (row.send) pending.push(validPendingSend(row.send)!);
        }
        pending.sort((a, b) => a.at - b.at); done({ sends: pending, replies: stored });
      };
      replies.onsuccess = () => {
        stored = survivingRows(replyStore, replies.result, validReplyRow, row => row.persistedAt, this.now())
          .filter(row => scopeKey(row.scope) === scopeKey(scope)).map(row => ({ ...row, reply: normalizedReply(row.reply)! }));
        done({ sends: pending, replies: stored });
      };
    });
  }
  async scopes(machine: StoredMachine): Promise<DeliveryScope[]> {
    if (deliveryScope(machine, "scope-check").accountState === "unsupported") return [];
    return this.transaction<DeliveryScope[]>(["sends", "replies"], "readwrite", (tx, done) => {
      const scopes = new Map<string, DeliveryScope>();
      for (const name of ["sends", "replies"]) {
        const store = tx.objectStore(name), all = store.getAll();
        all.onsuccess = () => {
          const rows = name === "sends"
            ? survivingRows(store, all.result, validSendRow, row => row.updatedAt, this.now())
            : survivingRows(store, all.result, validReplyRow, row => row.persistedAt, this.now());
          for (const row of rows) if (row.scope.hub === machine.id && row.scope.baseUrl === machine.baseUrl && row.scope.device === machine.deviceId) scopes.set(scopeKey(row.scope), row.scope);
          done([...scopes.values()]);
        };
      }
    });
  }
  /** Merge only this tab's changes, never a whole-namespace replacement. */
  async reconcile(scope: DeliveryScope, before: PendingSend[], after: PendingSend[], fence: CredentialFence): Promise<"kept" | "too-long" | "not-saved"> {
    if (!validScope(scope)) return "not-saved";
    if (JSON.stringify(after).length > 64_000 || after.some((send) => !validPendingSend(send))) return "too-long";
    const prior = new Map(before.map((send) => [send.id, send]));
    const next = new Map(after.slice(-MAX_PENDING_SENDS).map((send) => [send.id, send]));
    const changes = [...new Set([...prior.keys(), ...next.keys()])].filter((id) => JSON.stringify(prior.get(id)) !== JSON.stringify(next.get(id)));
    const committedVersions = new Map<string, number>();
    let rejected = false;
    try {
      await this.transaction<void>(["sends", "blocks"], "readwrite", (tx, done) => {
        done(undefined);
        const blocked = tx.objectStore("blocks").get(blockKey(scope.hub, fence));
        blocked.onsuccess = () => {
          if (blocked.result) { rejected = true; return; }
          const store = tx.objectStore("sends");
          const all = store.getAll();
          all.onsuccess = () => {
            const records = new Map(survivingRows(store, all.result, validSendRow, row => row.updatedAt, this.now()).map(row => [row.key, row]));
            for (const id of changes) {
              const key = itemKey(scope, id);
              const row = records.get(key);
              if (row && row.revision !== this.observed.get(key)) continue;
              if (!row && prior.has(id)) continue;
              if (!prior.has(id) && row) continue;
              const send = next.get(id);
              if (send && row?.owner && row.owner !== this.owner) continue;
              if (send?.state === "held" && row?.send?.state !== "held" && row?.owner !== this.owner && row !== undefined) continue;
              const revision = (row?.revision ?? 0) + 1;
              const updated = { key, scope, ...(send ? { send } : {}), revision, updatedAt: this.now(), ...(row?.flight ? { flight: row.flight, owner: row.owner } : {}) } satisfies JournalSend;
              records.set(key, updated);
              store.put(updated);
              committedVersions.set(key, revision);
            }
            const rows = [...records.values()].sort((a, b) => b.updatedAt - a.updatedAt
              || Number(committedVersions.has(b.key)) - Number(committedVersions.has(a.key)));
            const scopes = new Set<string>();
            const counts = new Map<string, number>();
            const sizes = new Map<string, number>();
            for (const [index, row] of rows.entries()) {
              const identity = scopeKey(row.scope);
              if (row.send) scopes.add(identity);
              const count = (counts.get(identity) ?? 0) + (row.send ? 1 : 0);
              const chars = (sizes.get(identity) ?? 0) + (row.send ? JSON.stringify(row.send).length + 1 : 0);
              counts.set(identity, count); sizes.set(identity, chars);
              if (row.send && (scopes.size > 50 || count > MAX_PENDING_SENDS || chars > 64_000)) {
                store.put({ ...row, send: undefined, revision: row.revision + 1 });
                committedVersions.set(row.key, row.revision + 1);
              }
              if (index >= 2_000 || this.now() - row.updatedAt > 90 * 24 * 60 * 60 * 1_000) store.delete(row.key);
            }
          };
        };
      });
      for (const [key, revision] of committedVersions) this.observed.set(key, revision);
      if (committedVersions.size) this.changed();
      return rejected ? "not-saved" : "kept";
    } catch { return "not-saved"; }
  }
  /** Import-once by item identity, retaining cancellation tombstones. */
  async importLegacy(scope: DeliveryScope, sends: PendingSend[], fence: CredentialFence): Promise<void> {
    if (!validScope(scope)) return;
    await this.transaction<void>(["sends", "blocks"], "readwrite", (tx, done) => {
      done(undefined);
      const store = tx.objectStore("sends");
      const block = tx.objectStore("blocks").get(blockKey(scope.hub, fence));
      block.onsuccess = () => {
        if (block.result) return;
        const markerKey = JSON.stringify(["legacy-import", scopeKey(scope)]);
        const marker = tx.objectStore("blocks").get(markerKey);
        marker.onsuccess = () => {
        if (marker.result) return;
        for (const send of sends.slice(-MAX_PENDING_SENDS)) {
        if (!validPendingSend(send) || JSON.stringify(send).length > 64_000) continue;
        const key = itemKey(scope, send.id);
        const get = store.get(key);
        get.onsuccess = () => {
          if (get.result) return;
          store.put({ key, scope, send: { ...send, state: send.state === "sending" ? "unconfirmed" : send.state }, revision: 1, updatedAt: this.now() } satisfies JournalSend);
        };
        }
        tx.objectStore("blocks").put({ key: markerKey, credentialId: fence.credentialId } satisfies Block);
        };
      };
    });
  }
  async cancel(scope: DeliveryScope, id: string): Promise<boolean> {
    if (!validScope(scope)) return false;
    const key = itemKey(scope, id);
    let cancelled = false;
    await this.transaction<void>(["sends"], "readwrite", (tx, done) => {
      done(undefined);
      const store = tx.objectStore("sends");
      const get = store.get(key);
      get.onsuccess = () => {
        const row = validSendRow(get.result) ? get.result : undefined;
        if (row?.send?.state !== "held") return;
        store.put({ ...row, send: undefined, revision: row.revision + 1, updatedAt: this.now() });
        cancelled = true;
      };
    });
    if (cancelled) this.changed();
    return cancelled;
  }
  async dispatch(scope: DeliveryScope, id: string, fence: CredentialFence, write: () => boolean): Promise<"written" | "waiting" | "unconfirmed" | "expired" | "stale" | "not-saved"> {
    if (!validScope(scope)) return "stale";
    const accepted = await this.current(scope);
    if (!accepted || !sameFence(accepted, fence)) return "stale";
    const key = itemKey(scope, id);
    let claimed: JournalSend | undefined;
    let expired = false;
    try {
      await this.transaction<void>(["sends", "blocks"], "readwrite", (tx, done) => {
        done(undefined);
        const block = tx.objectStore("blocks").get(blockKey(scope.hub, fence));
        block.onsuccess = () => {
          if ((block.result as Block | undefined)?.credentialId === fence.credentialId) return;
          const store = tx.objectStore("sends");
          const get = store.get(key);
          get.onsuccess = () => {
            const row = validSendRow(get.result) ? get.result : undefined;
            if (row?.send?.state !== "held") return;
            if (this.now() - (row.send.heldAt ?? row.send.at) >= 120_000) {
              expired = true;
              store.put({ ...row, revision: row.revision + 1, send: { ...row.send, state: "error", error: "This message was not sent before its wait expired. Retry to send it." } });
              return;
            }
            claimed = { ...row, revision: row.revision + 1, updatedAt: this.now(), flight: fence, owner: this.owner, send: { ...row.send, state: "sending", sentAt: this.now() } };
            store.put(claimed);
          };
        };
      });
    } catch { return "not-saved"; }
    if (!claimed) { if (expired) this.changed(); return expired ? "expired" : "unconfirmed"; }
    this.observed.set(key, claimed.revision);
    // A catalog replacement during either awaited operation never borrows the
    // replacement credential. The old claim remains uncertain, not replayable.
    const latest = await this.current(scope);
    if (!latest || !sameFence(latest, fence)) { this.changed(); return "stale"; }
    let sent: boolean;
    try { sent = write(); } catch { this.changed(); return "unconfirmed"; }
    if (sent) { this.changed(); return "written"; }
    // Only a synchronous false proves no websocket write was made.
    await this.reconcile(scope, [claimed.send!], [{ ...claimed.send!, state: "held", sentAt: undefined }], fence);
    return "waiting";
  }
  async persistReply(scope: DeliveryScope, reply: OperatorReply, fence: CredentialFence): Promise<boolean> {
    if (!validScope(scope)) return false;
    const normalized = normalizedReply(reply);
    if (!normalized) return false;
    reply = normalized;
    const accepted = await this.current(scope);
    if (!accepted || !sameFence(accepted, fence)) return false;
    try {
      let saved = false;
      await this.transaction<void>(["replies", "blocks"], "readwrite", (tx, done) => {
        done(undefined);
        const blocked = tx.objectStore("blocks").get(blockKey(scope.hub, fence));
        blocked.onsuccess = () => {
          if ((blocked.result as Block | undefined)?.credentialId === fence.credentialId) return;
          const store = tx.objectStore("replies");
          const key = itemKey(scope, reply.notification_id);
          const get = store.get(key);
          get.onsuccess = () => {
            const existing = validReplyRow(get.result) ? get.result : undefined;
            if (get.result !== undefined && !existing) return;
            if (existing && JSON.stringify(normalizedReply(existing.reply)) !== JSON.stringify(reply)) return;
            if (!existing || this.now() - existing.persistedAt > RETENTION_MS) store.put({ key, scope, reply, persistedAt: this.now() } satisfies StoredReply);
            saved = true;
            const all = store.getAll();
            all.onsuccess = () => {
              const rows = survivingRows(store, all.result, validReplyRow, row => row.persistedAt, this.now())
                .sort((a, b) => b.persistedAt - a.persistedAt || Number(b.key === key) - Number(a.key === key));
              // Fixed count and age; never unbounded private reply storage.
              for (const row of rows.slice(400)) store.delete(row.key);
              for (const row of rows) if (this.now() - row.persistedAt > 90 * 24 * 60 * 60 * 1_000) store.delete(row.key);
            };
          };
        };
      });
      if (saved) this.changed();
      return saved;
    } catch { return false; }
  }
  async purge(hub: string, fence: CredentialFence): Promise<void> {
    await this.transaction<void>(["sends", "replies", "blocks"], "readwrite", (tx, done) => {
      done(undefined);
      tx.objectStore("blocks").put({ key: blockKey(hub, fence), credentialId: fence.credentialId } satisfies Block);
      for (const name of ["sends", "replies"]) {
        const store = tx.objectStore(name);
        const all = store.getAll();
        all.onsuccess = () => {
          for (const row of all.result as (JournalSend | StoredReply)[]) if (!validScope(row.scope) || row.scope.hub === hub) {
            store.delete(row.key);
          }
        };
      }
    });
    this.changed();
  }
}
