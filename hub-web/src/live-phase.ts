/** A presentation of daemon facts; never an elapsed estimate from history. */
export interface PhaseLine {
  readonly label: string;
  readonly elapsed?: string;
  readonly active: boolean;
  readonly unavailable: boolean;
}
export interface PhaseContext {
  connected: boolean;
  stale?: boolean;
  waiting?: boolean;
  working?: boolean;
  /** Browser monotonic clock, paired with the moment status arrived. */
  now: number;
  receivedAt?: number;
}
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
  if (!reported && !context.waiting && !context.working) return undefined;
  const phase = reported ? lifecycle.phase : undefined;
  const ended = lifecycle?.stage === "end" || typeof lifecycle?.outcome === "string" || phase === "compacted";
  const sourceRunning = reported && !ended && ["start", "phase"].includes(lifecycle.stage as string);
  // An old unanswered ask must not override a newer executing lifecycle.
  const waiting = (context.waiting && !sourceRunning) || ["waiting", "waiting_on_you", "waiting_for_input", "awaiting_approval"].includes(String(phase));
  let label = "Phase not reported";
  let running = false;
  if (waiting) label = "Waiting on you";
  else if (phase === "compacting" || phase === "compacted") { label = ended ? "Compacted" : "Compacting"; running = !ended; }
  else if (phase === "setting_up" || phase === "setup") { label = ended ? "Setup completed" : "Setting up"; running = !ended; }
  else if (reported && ["start", "phase", "end"].includes(lifecycle.stage as string) && (phase === undefined || phase === "running")) {
    label = snapshot?.kind === "tool" ? ended ? lifecycle.outcome === "error" ? "Tool failed" : "Tool finished" : "Running a tool"
      : ended ? lifecycle.outcome === "error" ? "Turn failed" : "Turn completed" : "Working";
    running = !ended && (lifecycle.stage === "start" || lifecycle.stage === "phase");
  }
  const unavailable = !context.connected || context.stale === true || snapshot?.stale === true;
  const active = running && !waiting && !unavailable;
  const start = reported ? instant(lifecycle.started_at) : undefined;
  const observed = instant(snapshot?.observed_at);
  const end = ended ? instant(lifecycle?.ended_at) : observed === undefined ? undefined
    : observed + (active ? Math.max(0, context.now - (context.receivedAt ?? context.now)) : 0);
  const seconds = start !== undefined && end !== undefined && end >= start && !waiting ? Math.floor((end - start) / 1000) : undefined;
  const elapsed = seconds === undefined ? undefined : `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
  if (unavailable) label = `${context.connected ? "Update stale" : "Disconnected"} · ${label} (last known)`;
  return { label, ...(elapsed === undefined ? {} : { elapsed }), active, unavailable };
}
