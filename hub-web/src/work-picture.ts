/**
 * The whole picture (cas-ff22): which work-event pages Commander reads and
 * when. A thread with Show work off reads nothing. With it on, the open
 * thread reads its newest page once, an older page only when the loaded
 * history reaches further back than the loaded work, and a detail only when
 * a row is opened. A daemon that pushes `WorkEventsAppended` says when to
 * read the newest page again; an older one is re-read on a slow timer, and
 * only while a view that shows work is open.
 */
import type { WorkRowsContext } from "./work-rows-view";
import { WorkEventStream, type WorkEvent, type WorkEventDetail, type WorkEventsPage, type WorkEventSupport } from "./work-events";

/** What the controller needs of a machine connection. */
export interface WorkEventTransport {
  workEvents(session: string): WorkEventSupport;
  requestWorkEvents(session: string, options?: { before?: string; agent?: string; limit?: number }): string | undefined;
  requestWorkEventDetail(session: string, eventId: string): string | undefined;
}

export interface WorkPictureDeps {
  transport(machineId: string): WorkEventTransport | undefined;
  /** Repaint the thread(s) of one session; never a shell render. */
  repaintThread(machineId: string, session: string): void;
  /** Repaint the open worker timeline, if any. */
  repaintTimeline(): void;
  storage?: Pick<Storage, "getItem" | "setItem">;
  setInterval?: (run: () => void, ms: number) => number;
  clearInterval?: (handle: number) => void;
}

export const SHOW_WORK_KEY = "commander.showWork";
/** How often a daemon without change notices is re-read while work is on screen. */
export const PULL_REFRESH_MS = 10_000;

interface Watch { machineId: string; session: string; agent?: string }

function streamKey(machineId: string, session: string, agent?: string): string {
  return `${machineId}\u0000${session}\u0000${agent ?? ""}`;
}

export class WorkPicture {
  private readonly streams = new Map<string, WorkEventStream>();
  /** A live notice newer than the page currently being read. */
  private readonly invalidated = new Set<WorkEventStream>();
  private readonly open = new Set<string>();
  private show: boolean;
  private thread: Watch | undefined;
  private timeline: Watch | undefined;
  private poll: number | undefined;

  constructor(private readonly deps: WorkPictureDeps) {
    this.show = deps.storage?.getItem(SHOW_WORK_KEY) === "1";
  }

  get showWork(): boolean { return this.show; }

  /** Turn Show work on or off on this device. */
  setShowWork(on: boolean): void {
    if (on === this.show) return;
    this.show = on;
    try { this.deps.storage?.setItem(SHOW_WORK_KEY, on ? "1" : "0"); } catch { /* private mode: this visit only */ }
    if (on && this.thread) this.readNewest(this.thread);
    this.syncPoll();
    if (this.thread) this.deps.repaintThread(this.thread.machineId, this.thread.session);
  }

  /** Whether the session's daemon serves work events at all. */
  supported(machineId: string, session: string): boolean {
    return (this.deps.transport(machineId)?.workEvents(session) ?? "none") !== "none";
  }

  /** Whether one agent's own timeline can be read (a daemon with change notices). */
  timelineSupported(machineId: string, session: string): boolean {
    return this.deps.transport(machineId)?.workEvents(session) === "live";
  }

  stream(machineId: string, session: string, agent?: string): WorkEventStream {
    const key = streamKey(machineId, session, agent);
    let stream = this.streams.get(key);
    if (!stream) { stream = new WorkEventStream(agent); this.streams.set(key, stream); }
    return stream;
  }

  /** The thread's work, or undefined while Show work is off. */
  threadWork(machineId: string, session: string): readonly WorkEvent[] | undefined {
    if (!this.show || !this.supported(machineId, session)) return undefined;
    return this.stream(machineId, session).events;
  }

  /**
   * The open thread, on every render. Coming to a thread (first time or
   * back from another) reads its newest work: notices that arrived while it
   * was not on screen were not acted on, and an idle supervisor sends no more.
   */
  watchThread(machineId: string | undefined, session: string | undefined): void {
    const previous = this.thread;
    this.thread = machineId && session ? { machineId, session } : undefined;
    const arrived = this.thread !== undefined && (previous?.machineId !== machineId || previous?.session !== session);
    if (this.thread && this.show && (arrived || !this.stream(machineId!, session!).loaded)) this.readNewest(this.thread);
    this.syncPoll();
  }

  /** A worker timeline opened (or closed, with undefined). */
  watchTimeline(watch: Watch | undefined): void {
    this.timeline = watch;
    if (watch) this.readNewest(watch);
    this.syncPoll();
  }

  /**
   * The thread's loaded history reaches back to `oldestTurnAt`: read older
   * work while the loaded work starts later, one page at a time.
   */
  coverHistory(machineId: string, session: string, oldestTurnAt: number | undefined): void {
    if (!this.show || oldestTurnAt === undefined) return;
    const stream = this.stream(machineId, session);
    if (!stream.loaded || !stream.hasEarlier || stream.pending("older") || stream.cursor === undefined) return;
    const oldest = stream.oldestAt;
    if (oldest !== undefined && oldest <= oldestTurnAt) return;
    this.readOlder({ machineId, session });
  }

  /** "Load older" on a timeline. */
  readOlder(watch: Watch): void {
    const stream = this.stream(watch.machineId, watch.session, watch.agent);
    if (stream.pending("older") || stream.cursor === undefined) return;
    stream.requested(this.deps.transport(watch.machineId)?.requestWorkEvents(watch.session, { before: stream.cursor, ...(watch.agent ? { agent: watch.agent } : {}) }), "older");
  }

  rows(machineId: string, session: string, agent?: string): WorkRowsContext {
    const stream = this.stream(machineId, session, agent);
    const prefix = streamKey(machineId, session, agent);
    return {
      detail: (eventId) => stream.detail(eventId),
      requestDetail: (eventId) => {
        if (stream.detail(eventId)?.state === "loading") return;
        stream.requested(this.deps.transport(machineId)?.requestWorkEventDetail(session, eventId), { detail: eventId });
      },
      isOpen: (key) => this.open.has(`${prefix}\u0000${key}`),
      setOpen: (key, open) => { if (open) this.open.add(`${prefix}\u0000${key}`); else this.open.delete(`${prefix}\u0000${key}`); },
    };
  }

  /* ---- connection callbacks ------------------------------------------ */

  page(machineId: string, session: string, page: WorkEventsPage): void {
    for (const stream of this.sessionStreams(machineId, session)) {
      if (!stream.owns(page.request_id)) continue;
      const gap = stream.accept(page);
      if (gap !== undefined && this.visibleWatch(machineId, session, stream)) stream.requested(this.deps.transport(machineId)?.requestWorkEvents(session, { before: gap, ...(stream.agent ? { agent: stream.agent } : {}) }), "gap");
      this.refreshInvalidated(machineId, session, stream);
      this.repaint(machineId, session, stream);
      return;
    }
  }

  detail(machineId: string, session: string, detail: WorkEventDetail): void {
    for (const stream of this.sessionStreams(machineId, session)) {
      if (stream.acceptDetail(detail)) { this.repaint(machineId, session, stream); return; }
    }
  }

  failed(machineId: string, session: string, requestId: string, reason: string): void {
    for (const stream of this.sessionStreams(machineId, session)) {
      if (stream.failed(requestId, reason)) {
        this.refreshInvalidated(machineId, session, stream);
        this.repaint(machineId, session, stream);
        return;
      }
    }
  }

  /** The daemon's stream grew: re-read the newest page of what is on screen. */
  appended(machineId: string, session: string): void {
    for (const watch of this.watching()) {
      if (watch.machineId === machineId && watch.session === session) {
        this.invalidated.add(this.stream(machineId, session, watch.agent));
        this.readNewest(watch);
      }
    }
  }

  /** The session's socket closed: answers in flight will never come. */
  disconnected(machineId: string, session: string): void {
    for (const stream of this.sessionStreams(machineId, session)) {
      stream.abandon();
      this.invalidated.delete(stream);
    }
  }

  /** A reattach: re-read the newest page of what is on screen. */
  reattached(machineId: string, session: string): void {
    this.appended(machineId, session);
    this.syncPoll();
  }

  /* ---- internals ----------------------------------------------------- */

  private watching(): Watch[] {
    const watches: Watch[] = [];
    if (this.thread && this.show) watches.push(this.thread);
    if (this.timeline) watches.push(this.timeline);
    return watches;
  }

  private sessionStreams(machineId: string, session: string): WorkEventStream[] {
    const prefix = `${machineId}\u0000${session}\u0000`;
    return [...this.streams].filter(([key]) => key.startsWith(prefix)).map(([, stream]) => stream);
  }

  private readNewest(watch: Watch): void {
    const stream = this.stream(watch.machineId, watch.session, watch.agent);
    if (stream.pending("newest")) return;
    if (watch.agent && !this.timelineSupported(watch.machineId, watch.session)) return;
    const request = this.deps.transport(watch.machineId)?.requestWorkEvents(watch.session, watch.agent ? { agent: watch.agent } : {});
    stream.requested(request, "newest");
    if (request !== undefined) this.invalidated.delete(stream);
  }

  /** Coalesce notices during a newest/gap read into one follow-up, only while visible. */
  private refreshInvalidated(machineId: string, session: string, stream: WorkEventStream): void {
    if (!this.invalidated.has(stream) || stream.pending("newest")) return;
    const watch = this.visibleWatch(machineId, session, stream);
    if (watch) this.readNewest(watch);
    else this.invalidated.delete(stream);
  }

  private visibleWatch(machineId: string, session: string, stream: WorkEventStream): Watch | undefined {
    return this.watching().find(watch => watch.machineId === machineId && watch.session === session && watch.agent === stream.agent);
  }

  private repaint(machineId: string, session: string, stream: WorkEventStream): void {
    if (stream.agent === undefined) this.deps.repaintThread(machineId, session);
    else if (this.timeline && this.timeline.machineId === machineId && this.timeline.session === session && this.timeline.agent === stream.agent) this.deps.repaintTimeline();
  }

  /** Poll only a pull-only daemon, and only while work is on screen. */
  private syncPoll(): void {
    const pulled = this.watching().filter((watch) => this.deps.transport(watch.machineId)?.workEvents(watch.session) === "pull");
    if (pulled.length === 0) {
      if (this.poll !== undefined) (this.deps.clearInterval ?? clearInterval)(this.poll);
      this.poll = undefined;
      return;
    }
    if (this.poll !== undefined) return;
    this.poll = (this.deps.setInterval ?? ((run, ms) => window.setInterval(run, ms)))(() => {
      for (const watch of this.watching()) {
        if (this.deps.transport(watch.machineId)?.workEvents(watch.session) === "pull") this.readNewest(watch);
      }
    }, PULL_REFRESH_MS);
  }
}
