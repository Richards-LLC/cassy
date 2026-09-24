import type { AttachSnapshot, ConnectionSnapshot } from "./connection-state";

/**
 * One connection state per conversation (cas-a447, journey evaluation F10).
 *
 * The machine's control connection can stay live while a session's own socket
 * is down. The header, the list row and the footer used to read only the
 * machine and kept saying "Live" / "Connected" beside a "Connection
 * interrupted" banner. They all read this instead: the machine's state while it
 * is not live, else the session's attach lifecycle while that is not live.
 *
 * A session that has been live and dropped is reconnecting, whatever stage its
 * retry is in (dialing, attaching, a transient failure), so it reads as
 * backoff. A failure retrying cannot fix (fatal, or an auth failure) keeps its
 * own phase.
 */
export function sessionConnection(
  machine: ConnectionSnapshot | undefined,
  attach: AttachSnapshot | undefined,
  wasLive: boolean,
): ConnectionSnapshot | undefined {
  if (!machine || machine.phase !== "live") return machine;
  if (!attach || attach.phase === "live" || attach.phase === "idle") return machine;
  if (attach.fatal === true || attach.authFailure) return attach;
  return wasLive ? { ...attach, phase: "backoff" } : attach;
}

/**
 * The machine as the footer shows it: its own state, or the first of its
 * attached sessions that is not live.
 */
export function machineConnection(
  machine: ConnectionSnapshot | undefined,
  sessions: ReadonlyArray<{ attach: AttachSnapshot | undefined; wasLive: boolean }>,
): ConnectionSnapshot | undefined {
  if (!machine || machine.phase !== "live") return machine;
  for (const { attach, wasLive } of sessions) {
    const effective = sessionConnection(machine, attach, wasLive);
    if (effective && effective.phase !== "live") return effective;
  }
  return machine;
}
