import type { AttachSnapshot, ConnectionSnapshot } from "./connection-state";

/**
 * One connection state per conversation (cas-a447, journey evaluation F10).
 *
 * The machine's control connection can stay live while a session's own socket
 * is down. The header, the list row and the footer used to read only the
 * machine and kept saying "Live" / "Connected" beside a "Connection
 * interrupted" banner. They all read this instead: a responding session can
 * remain live during event recovery; otherwise the machine's state wins while
 * it is not live, then the session's attach lifecycle while that is not live.
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
  responding = false,
): ConnectionSnapshot | undefined {
  if (!machine) return machine;
  if (machine.phase !== "live") {
    // Event delivery retries independently of a current, speaking daemon.
    // Cached attach.phase alone is insufficient: the owner must prove its
    // OPEN/ready socket has received a frame inside the liveness window.
    if (responding && attach?.phase === "live" && machine.phase !== "idle"
      && !machine.fatal && !machine.authFailure && !machine.networkAccessHelp && !machine.degraded) return attach;
    return machine;
  }
  if (!attach || attach.phase === "live" || attach.phase === "idle") return machine;
  if (attach.fatal === true || attach.authFailure) return attach;
  if (wasLive) return { ...attach, phase: "backoff" };
  // cas-28df: the first retry of a conversation that has never opened is
  // still the conversation opening, not a drop: it reads as connecting.
  return firstAttachRetry(attach, wasLive) ? { ...attach, phase: openingPhase(attach) } : attach;
}

/**
 * How many failed attempts can pass before a first attach stops reading as a
 * conversation still opening (cas-28df). One transient miss (the 3 s "no
 * session state" rule, a socket closed before it was ready) is routine on a
 * live machine, so the first retry stays calm; a second failure is a real
 * problem and shows as one.
 */
export const FIRST_ATTACH_QUIET_FAILURES = 1;

/**
 * A conversation that has never been live, on its first retry: the attempt
 * failed for a reason retrying can fix and one more is scheduled or about to
 * be. `attempt` counts retries scheduled so far, so a failed snapshot has one
 * more failure behind it than its attempt number, a backoff one exactly as many.
 */
export function firstAttachRetry(attach: AttachSnapshot | undefined, wasLive: boolean): boolean {
  if (!attach || wasLive || attach.fatal === true || attach.authFailure) return false;
  if (attach.phase !== "failed" && attach.phase !== "backoff") return false;
  const failures = attach.phase === "failed" ? attach.attempt + 1 : attach.attempt;
  return failures <= FIRST_ATTACH_QUIET_FAILURES;
}

function openingPhase(attach: AttachSnapshot): ConnectionSnapshot["phase"] {
  return attach.stage === "live" || attach.stage === "idle" ? "attaching" : attach.stage;
}

/**
 * The machine as the footer shows it: its own state, or the first of its
 * attached sessions that is not live. A session opening for the first time
 * (never live, not failed, no retry scheduled) is a conversation attaching,
 * not the machine dropping: it does not count against the machine, so the
 * footer does not say Reconnecting or lower its connected count while a
 * conversation opens (journey F3). Nor does its first retry (cas-28df): the
 * machine's hub is connected; only a second failure counts against it.
 * Nor does a live session whose daemon link dropped and is reconnecting
 * (cas-d15c): the hub closed that one session's stream (`sessionOnly`) while
 * the machine stayed connected, and the conversation's own header, row and
 * banner say so. A socket that simply dropped, or a failure retrying cannot
 * fix, still counts.
 */
export function machineConnection(
  machine: ConnectionSnapshot | undefined,
  sessions: ReadonlyArray<{ attach: AttachSnapshot | undefined; wasLive: boolean; responding?: boolean }>,
): ConnectionSnapshot | undefined {
  if (!machine) return machine;
  if (machine.phase !== "live") {
    const speaking = sessions.find(({ attach, responding }) => responding && attach?.phase === "live");
    const effective = speaking && sessionConnection(machine, speaking.attach, speaking.wasLive, speaking.responding);
    if (effective?.phase !== "live") return machine;
    machine = effective;
  }
  for (const { attach, wasLive } of sessions) {
    if (!wasLive && attach && (firstAttachInProgress(attach) || firstAttachRetry(attach, wasLive))) continue;
    if (wasLive && attach && sessionOnlyReconnect(attach)) continue;
    const effective = sessionConnection(machine, attach, wasLive);
    if (effective && effective.phase !== "live") return effective;
  }
  return machine;
}

function firstAttachInProgress(attach: AttachSnapshot): boolean {
  return attach.fatal !== true && !attach.authFailure && attach.phase !== "failed" && attach.phase !== "backoff" && attach.phase !== "live";
}

/** A session that was live, reconnecting for a reason retrying can fix (cas-d15c). */
function sessionOnlyReconnect(attach: AttachSnapshot): boolean {
  return attach.sessionOnly === true && attach.fatal !== true && !attach.authFailure && attach.phase !== "live" && attach.phase !== "idle";
}
