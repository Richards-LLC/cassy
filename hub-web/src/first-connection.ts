import type { ConnectionState } from "./connection";

/**
 * "Access saved" is what pairing can truthfully say; "connected" is the
 * connection's answer and is said once, when that machine first reaches a
 * healthy live phase after its credential was saved (cas-8051 F8).
 *
 * One entry per machine, so pairing a second machine while the first is still
 * dialing does not lose the first machine's announcement, and re-pairing a
 * machine that is already live announces its new credential's connection once
 * more. A reconnect after a drop is not a first connection and says nothing;
 * the header, fleet board and attention feed carry connection health.
 */
export class FirstConnectionAnnouncer {
  private readonly pending = new Set<string>();

  /** A credential for `machineId` was just saved. */
  expect(machineId: string): void {
    this.pending.add(machineId);
  }

  /** The machine was removed or its pairing was replaced before it connected. */
  forget(machineId: string): void {
    this.pending.delete(machineId);
  }

  isPending(machineId: string): boolean {
    return this.pending.has(machineId);
  }

  /** Returns the sentence to announce, or undefined when there is nothing new to say. */
  observe(machineId: string, label: string, state: Pick<ConnectionState, "phase" | "degraded">): string | undefined {
    if (!this.pending.has(machineId)) return undefined;
    if (state.phase !== "live" || state.degraded) return undefined;
    this.pending.delete(machineId);
    return `${label} connected`;
  }
}
