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
export function livePhase(_value: unknown, _context: PhaseContext): PhaseLine | undefined {
  return undefined;
}
