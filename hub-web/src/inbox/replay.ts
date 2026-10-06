// Feed replay for one enrolled device (cloud contract §8.1–§8.3, §7.4).
//
// One round pages from this device's own cursor to the head:
//
// 1. Validate the page: generation, decimal bigints, ascending events, and
//    exact coverage of `after+1 … next_cursor` by events plus expired
//    intervals, each sequence exactly once. A page that fails is refused
//    whole; nothing is stored and no cursor moves.
// 2. Decrypt and verify every event. Observer notices pass all six §7.4
//    checks or are stored as unverified (kept for coverage, never rendered
//    or ACKed). A session event that does not open is stored undecryptable.
// 3. Commit the page events and its cursor in one IndexedDB transaction.
// 4. Only then post persisted ACKs for verified events (including any left
//    unacknowledged by a crash), then sync the cursor to the cloud.
//
// `410 history_expired` records an accepted gap and continues from the floor;
// `409 feed_generation_changed` is returned to the caller (warn, then reset);
// `409 cursor_ahead` stops the round (wrong account or a server anomaly).

import { b64urlDecode, digest, openEvent, openObserverNotice } from "./hpke";
import { ISSUER_TYP, IssuerTokenError, type IssuerKeys } from "./issuer";
import type { ExpiredInterval, InboxEvent, InboxIdentity, InboxStore } from "./store";
import { OperatorWireError, decimal, list, record, str, type OperatorClient } from "./wire";

const decoder = new TextDecoder("utf-8", { fatal: true });
const ACK_BATCH = 500;
const MAX_PAGES_PER_ROUND = 50;

export type ReplayOutcome =
  | { kind: "caught_up"; pollAfterMs: number; stored: InboxEvent[]; pages: number }
  | { kind: "more"; stored: InboxEvent[]; pages: number }
  | { kind: "generation_changed"; feedGeneration: string; startSequence: string }
  | { kind: "cursor_ahead"; head: string };

export class ReplayPageError extends Error {
  constructor(readonly reason: string) {
    super(`replay page refused: ${reason}`);
    this.name = "ReplayPageError";
  }
}

export interface MachineDirectory {
  /** Machine IDs of this account (`GET /principals`), for the §7.4 `mch` check. */
  has(machineId: string): boolean;
}

export interface ReplayContext {
  client: OperatorClient;
  store: InboxStore;
  issuer: IssuerKeys;
  identity: InboxIdentity;
  machines: () => Promise<MachineDirectory>;
  /** Re-fetch epoch keys when an event names an epoch this device lacks. */
  refreshKeys: () => Promise<void>;
}

interface WireEvent {
  sequence: string;
  eventId: string;
  scope: string;
  producerKind: string;
  hubId: string;
  projectId: string | null;
  sessionId: string | null;
  keyEpoch: string;
  ciphertext: string;
  digest: string;
  storedAt: string;
  expiresAt: string;
  observerAssertion: string | null;
}

function parseEvent(value: unknown): WireEvent {
  const row = record(value, "event");
  return {
    sequence: decimal(row.sequence, "sequence"),
    eventId: str(row.event_id, "event_id"),
    scope: typeof row.event_scope === "string" ? row.event_scope : "session",
    producerKind: typeof row.producer_kind === "string" ? row.producer_kind : "principal",
    hubId: str(row.hub_id, "hub_id"),
    projectId: typeof row.project_id === "string" ? row.project_id : null,
    sessionId: typeof row.session_id === "string" ? row.session_id : null,
    keyEpoch: decimal(row.key_epoch, "key_epoch"),
    ciphertext: str(row.ciphertext, "ciphertext"),
    digest: str(row.digest, "digest"),
    storedAt: str(row.stored_at, "stored_at"),
    expiresAt: str(row.expires_at, "expires_at"),
    observerAssertion: typeof row.observer_assertion === "string" ? row.observer_assertion : null,
  };
}

/** Exact coverage of `after+1 … next` by events and intervals, each once (§8.1). */
export function verifyCoverage(after: string, next: string, events: WireEvent[], intervals: ExpiredInterval[]): void {
  const start = BigInt(after);
  const end = BigInt(next);
  if (end < start) throw new ReplayPageError("next_cursor below after");
  const spans: [bigint, bigint][] = [
    ...events.map((event) => [BigInt(event.sequence), BigInt(event.sequence)] as [bigint, bigint]),
    ...intervals.map((interval) => [BigInt(interval.from), BigInt(interval.to)] as [bigint, bigint]),
  ].sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
  let expected = start + 1n;
  for (const [from, to] of spans) {
    if (to < from) throw new ReplayPageError("inverted interval");
    if (from !== expected) throw new ReplayPageError(from < expected ? "overlap" : "gap");
    expected = to + 1n;
  }
  if (expected !== end + 1n) throw new ReplayPageError(expected <= end ? "gap at end" : "beyond next_cursor");
  for (let index = 1; index < events.length; index += 1) {
    if (BigInt(events[index].sequence) <= BigInt(events[index - 1].sequence)) throw new ReplayPageError("events out of order");
  }
}

async function epochPair(context: ReplayContext, epoch: string): Promise<CryptoKeyPair | null> {
  const { store, identity } = context;
  let key = await store.epochKey(identity.accountId, identity.feedGeneration, epoch);
  if (!key) {
    await context.refreshKeys().catch(() => undefined);
    key = await store.epochKey(identity.accountId, identity.feedGeneration, epoch);
  }
  return key?.pair ?? null;
}

function base(identity: InboxIdentity, event: WireEvent, scope: InboxEvent["scope"], producerKind: InboxEvent["producerKind"]): Omit<InboxEvent, "plaintext" | "verification" | "failure"> {
  return {
    accountId: identity.accountId,
    feedGeneration: identity.feedGeneration,
    sequence: event.sequence,
    eventId: event.eventId,
    scope,
    producerKind,
    hubId: event.hubId,
    projectId: event.projectId,
    sessionId: event.sessionId,
    keyEpoch: event.keyEpoch,
    digest: event.digest,
    storedAt: event.storedAt,
    expiresAt: event.expiresAt,
    acked: false,
  };
}

function refused(row: Omit<InboxEvent, "plaintext" | "verification" | "failure">, verification: "unverified" | "undecryptable", failure: string): InboxEvent {
  return { ...row, plaintext: null, verification, failure };
}

async function decodeSessionEvent(context: ReplayContext, event: WireEvent, bytes: Uint8Array): Promise<InboxEvent> {
  const row = base(context.identity, event, "session", "principal");
  if (!event.projectId || !event.sessionId) return refused(row, "unverified", "missing_routing");
  const pair = await epochPair(context, event.keyEpoch);
  if (!pair) return refused(row, "undecryptable", "epoch_unavailable");
  try {
    const plain = await openEvent(pair, bytes, {
      accountId: context.identity.accountId,
      feedGeneration: context.identity.feedGeneration,
      keyEpoch: event.keyEpoch,
      eventId: event.eventId,
      hubId: event.hubId,
      projectId: event.projectId,
      sessionId: event.sessionId,
    });
    return { ...row, plaintext: JSON.parse(decoder.decode(plain)), verification: "verified", failure: null };
  } catch {
    return refused(row, "undecryptable", "open_failed");
  }
}

/** The six device checks of §7.4, in order, before an observer row may be persisted as verified. */
async function decodeObserverNotice(context: ReplayContext, event: WireEvent, bytes: Uint8Array): Promise<InboxEvent> {
  const row = base(context.identity, event, "machine", "cloud_observer");
  if (!event.observerAssertion) return refused(row, "unverified", "assertion_missing");
  let claims: Record<string, unknown>;
  try {
    ({ claims } = await context.issuer.verify(event.observerAssertion, ISSUER_TYP.machineObservation));
  } catch (error) {
    return refused(row, "unverified", error instanceof IssuerTokenError ? `assertion_${error.reason}` : "assertion_invalid");
  }
  if (claims.iss !== context.client.origin) return refused(row, "unverified", "issuer_mismatch");
  if (claims.acct !== context.identity.accountId) return refused(row, "unverified", "account_mismatch");
  if (
    claims.event_id !== event.eventId ||
    claims.digest !== event.digest ||
    claims.hub !== event.hubId ||
    claims.key_epoch !== event.keyEpoch ||
    claims.fgen !== context.identity.feedGeneration
  ) {
    return refused(row, "unverified", "claim_mismatch");
  }
  const machines = await context.machines().catch(() => null);
  if (!machines || typeof claims.mch !== "string" || !machines.has(claims.mch)) return refused(row, "unverified", "machine_unknown");
  const pair = await epochPair(context, event.keyEpoch);
  if (!pair) return refused(row, "undecryptable", "epoch_unavailable");
  let plaintext: Record<string, unknown>;
  try {
    const plain = await openObserverNotice(pair, bytes, {
      accountId: context.identity.accountId,
      feedGeneration: context.identity.feedGeneration,
      keyEpoch: event.keyEpoch,
      eventId: event.eventId,
      hubId: event.hubId,
    });
    plaintext = record(JSON.parse(decoder.decode(plain)), "notice");
  } catch {
    return refused(row, "undecryptable", "open_failed");
  }
  const refEventId = claims.ref_event_id ?? null;
  if (
    plaintext.kind !== claims.kind ||
    plaintext.machine_id !== claims.mch ||
    plaintext.hub_id !== claims.hub ||
    plaintext.outage_epoch !== claims.outage_epoch ||
    (plaintext.ref_event_id ?? null) !== refEventId
  ) {
    return refused(row, "unverified", "plaintext_mismatch");
  }
  return { ...row, plaintext, verification: "verified", failure: null };
}

async function decodeEvent(context: ReplayContext, event: WireEvent): Promise<InboxEvent> {
  let bytes: Uint8Array;
  try {
    bytes = b64urlDecode(event.ciphertext, "ciphertext");
  } catch {
    return refused(base(context.identity, event, "session", "principal"), "unverified", "ciphertext_malformed");
  }
  if ((await digest(bytes)) !== event.digest) {
    const scope = event.scope === "machine" ? "machine" : "session";
    return refused(base(context.identity, event, scope, scope === "machine" ? "cloud_observer" : "principal"), "unverified", "digest_mismatch");
  }
  if (event.scope === "session" && event.producerKind === "principal") return decodeSessionEvent(context, event, bytes);
  if (event.scope === "machine" && event.producerKind === "cloud_observer") return decodeObserverNotice(context, event, bytes);
  return refused(base(context.identity, event, "session", "principal"), "unverified", "scope_not_allowed");
}

/** Send persisted ACKs for every verified, committed, unacknowledged event. */
export async function flushAcks(context: ReplayContext): Promise<number> {
  const { client, store, identity } = context;
  const pending = await store.unacked(identity.accountId, identity.feedGeneration);
  let acked = 0;
  for (let index = 0; index < pending.length; index += ACK_BATCH) {
    const batch = pending.slice(index, index + ACK_BATCH);
    const response = await client.ackPersisted(
      identity.feedGeneration,
      batch.map((event) => ({ event_id: event.eventId, digest: event.digest })),
    );
    const settled = list(response.rows, "rows")
      .map((row) => record(row, "row"))
      .filter((row) => row.outcome === "acked" || row.outcome === "already_acked" || row.error === "event_expired")
      .map((row) => str(row.event_id, "event_id"));
    await store.markAcked(identity.accountId, settled);
    acked += settled.length;
  }
  return acked;
}

async function syncCursor(context: ReplayContext): Promise<void> {
  const { client, store, identity } = context;
  const cursor = await store.loadCursor(identity.accountId, identity.feedGeneration);
  if (!cursor || cursor.syncedCursor === cursor.cursor) return;
  try {
    await client.putCursor(identity.feedGeneration, cursor.cursor, cursor.acceptedExpiredThrough);
    await store.markSynced(identity.accountId, identity.feedGeneration, cursor.cursor);
  } catch (error) {
    // The cloud cursor is advisory for this device; local coverage is the
    // authority. A regression means the cloud already holds a higher value.
    if (!(error instanceof OperatorWireError) || (error.code !== "cursor_regression" && !error.retryable)) throw error;
  }
}

async function startAfter(context: ReplayContext): Promise<string> {
  const { client, identity } = context;
  if (identity.feedGeneration === "1") return "0";
  // A new device on a later generation learns its start from the one route
  // that reports it: a request naming a stale generation (§8.1).
  try {
    await client.replay("0", "0", 1);
  } catch (error) {
    if (error instanceof OperatorWireError && error.code === "feed_generation_changed") {
      const start = decimal(error.body.start_sequence, "start_sequence");
      return (BigInt(start) - 1n).toString();
    }
    throw error;
  }
  return "0";
}

export async function replayRound(context: ReplayContext, signal?: AbortSignal): Promise<ReplayOutcome> {
  const { client, store, identity } = context;
  const stored: InboxEvent[] = [];
  let pages = 0;
  while (pages < MAX_PAGES_PER_ROUND) {
    signal?.throwIfAborted();
    const local = await store.loadCursor(identity.accountId, identity.feedGeneration);
    const after = local?.cursor ?? (await startAfter(context));
    let page: Record<string, unknown>;
    try {
      page = await client.replay(identity.feedGeneration, after, 200, signal);
    } catch (error) {
      if (!(error instanceof OperatorWireError)) throw error;
      if (error.code === "history_expired") {
        const through = decimal(error.body.expired_through, "expired_through");
        if (BigInt(through) <= BigInt(after)) throw new ReplayPageError("history_expired below cursor");
        await store.commitPage({
          accountId: identity.accountId,
          feedGeneration: identity.feedGeneration,
          events: [],
          intervals: [{ from: (BigInt(after) + 1n).toString(), to: through, reason: "retention" }],
          cursor: through,
          acceptedExpiredThrough: through,
        });
        pages += 1;
        continue;
      }
      if (error.code === "feed_generation_changed") {
        return {
          kind: "generation_changed",
          feedGeneration: decimal(error.body.feed_generation, "feed_generation"),
          startSequence: decimal(error.body.start_sequence, "start_sequence"),
        };
      }
      if (error.code === "cursor_ahead") return { kind: "cursor_ahead", head: decimal(error.body.head, "head") };
      throw error;
    }
    pages += 1;
    if (page.feed_generation !== identity.feedGeneration) throw new ReplayPageError("feed_generation");
    decimal(page.head, "head");
    decimal(page.retained_floor, "retained_floor");
    const next = decimal(page.next_cursor, "next_cursor");
    const events = list(page.events, "events").map(parseEvent);
    const intervals = list(page.expired_intervals ?? [], "expired_intervals").map((entry) => {
      const interval = record(entry, "expired_interval");
      return { from: decimal(interval.from, "from"), to: decimal(interval.to, "to"), reason: str(interval.reason, "reason") };
    });
    verifyCoverage(after, next, events, intervals);
    const decoded: InboxEvent[] = [];
    for (const event of events) decoded.push(await decodeEvent(context, event));
    if (next !== after || decoded.length > 0) {
      const committed = await store.commitPage({
        accountId: identity.accountId,
        feedGeneration: identity.feedGeneration,
        events: decoded,
        intervals,
        cursor: next,
      });
      stored.push(...committed.stored);
    }
    await flushAcks(context);
    await syncCursor(context);
    if (page.has_more !== true) {
      const pollAfterMs = typeof page.poll_after_ms === "number" && page.poll_after_ms >= 0 ? page.poll_after_ms : 5000;
      return { kind: "caught_up", pollAfterMs, stored, pages };
    }
  }
  return { kind: "more", stored, pages };
}
