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

export interface InstalledMachineIdentity {
  readonly id: string;
  readonly label: string;
}

export interface InstallAnnouncementDeps<M extends InstalledMachineIdentity> {
  readonly announcer: FirstConnectionAnnouncer;
  /** Where sentences go (the toast). */
  readonly notify: (text: string) => void;
  /** Creates and starts the connection; its onState may fire synchronously. */
  readonly startConnection: (machine: M) => void;
}

/**
 * The installation seam: a credential for `machine` is saved. Say so, arm the
 * first-connection announcement, and only then start the connection — so even
 * a connection that reports healthy live synchronously produces exactly
 * ["Access saved — connecting to X…", "X connected"], named from the installed
 * machine and never from whatever happens to be selected (review 25649).
 */
export function installPairedMachine<M extends InstalledMachineIdentity>(machine: M, deps: InstallAnnouncementDeps<M>): void {
  deps.announcer.expect(machine.id);
  deps.notify(`Access saved — connecting to ${machine.label}…`);
  deps.startConnection(machine);
}
