/** A presentation of daemon facts; never an elapsed estimate from history. */
export interface PhaseLine {
  readonly label: string;
  /** Idle only from a fresh, reported turn end; missing or old facts are unknown. */
  readonly state: "busy" | "idle" | "waiting" | "unknown";
  /** The open turn's running tool name. */
  readonly step?: string;
  readonly lastActivityAt?: string;
  readonly elapsed?: string;
  readonly active: boolean;
  readonly unavailable: boolean;
}
export interface PhaseContext {
  connected: boolean;
  stale?: boolean;
  waiting?: boolean;
  working?: boolean;
  /** The session's daemon: a stopped one leaves the transcript's last turn open. */
  daemonRunning?: boolean;
  /** Browser monotonic clock, paired with the moment status arrived. */
  now: number;
  receivedAt?: number;
}
/** Status follows each machine heartbeat (5 s); three missed reads may be a stuck daemon. */
export const PHASE_FRESH_MS = 15_000;
function record(value: unknown): Record<string, unknown> | undefined {
  return value !== null && typeof value === "object" ? value as Record<string, unknown> : undefined;
}
function instant(value: unknown): number | undefined {
  const time = typeof value === "string" ? Date.parse(value) : NaN;
  return Number.isFinite(time) ? time : undefined;
}
export function livePhase(value: unknown, context: PhaseContext): PhaseLine | undefined {
  const snapshot = record(value), lifecycle = record(snapshot?.lifecycle);
  const reported = typeof lifecycle?.id === "string" && typeof lifecycle.stage === "string";
  if (!reported && !context.waiting) return undefined;
  const phase = reported ? lifecycle.phase : undefined;
  const ended = lifecycle?.stage === "end" || typeof lifecycle?.outcome === "string" || phase === "compacted";
  const sourceRunning = reported && !ended && ["start", "phase"].includes(lifecycle.stage as string);
  // An old unanswered ask must not override a newer executing lifecycle.
  const waiting = (context.waiting && !sourceRunning) || ["waiting", "waiting_on_you", "waiting_for_input", "awaiting_approval"].includes(String(phase));
  let label: string | undefined;
  let running = false;
  if (waiting) label = "Waiting on you";
  else if (phase === "compacting" || phase === "compacted") { label = ended ? "Compacted" : "Compacting"; running = !ended; }
  else if (phase === "setting_up" || phase === "setup") { label = ended ? "Setup completed" : "Setting up"; running = !ended; }
  else if (reported && ["start", "phase", "end"].includes(lifecycle.stage as string) && (phase === undefined || phase === "running")) {
    label = snapshot?.kind === "tool" ? ended ? lifecycle.outcome === "error" ? "Tool failed" : "Tool finished" : "Running a tool"
      : ended ? lifecycle.outcome === "error" ? "Turn failed" : "Turn completed" : "Working";
    running = !ended && (lifecycle.stage === "start" || lifecycle.stage === "phase");
  }
  if (label === undefined) return undefined;
  const expired = reported && (context.daemonRunning === false
    || (context.receivedAt !== undefined && context.now - context.receivedAt > PHASE_FRESH_MS));
  const unavailable = !context.connected || context.stale === true || snapshot?.stale === true || expired;
  const active = running && !waiting && !unavailable;
  const start = reported ? instant(lifecycle.started_at) : undefined;
  const observed = instant(snapshot?.observed_at);
  const end = ended ? instant(lifecycle?.ended_at) : observed === undefined ? undefined
    : observed + (active ? Math.max(0, context.now - (context.receivedAt ?? context.now)) : 0);
  const seconds = start !== undefined && end !== undefined && end >= start && !waiting ? Math.floor((end - start) / 1000) : undefined;
  const elapsed = seconds === undefined ? undefined : `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
  const state = unavailable ? "unknown" : waiting ? "waiting" : active ? "busy"
    : ended && snapshot?.kind === "notice" && phase === undefined ? "idle" : "unknown";
  if (unavailable) label = `${context.connected ? "Activity unknown, possibly stuck" : "Disconnected"} · ${label} (last known)`;
  const step = active && typeof snapshot?.step === "string" ? snapshot.step : undefined;
  const lastActivityAt = instant(snapshot?.last_activity_at) === undefined ? undefined : snapshot!.last_activity_at as string;
  return { label, state, ...(step ? { step } : {}), ...(lastActivityAt ? { lastActivityAt } : {}), ...(elapsed === undefined ? {} : { elapsed }), active, unavailable };
}
