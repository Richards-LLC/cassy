// Cloud contract §17. Machine observations have no conversation routing.
// The replay module verifies the observer assertion and decrypts before this
// projection; neither a snapshot nor a hub connection invents an outage notice.
import type { InboxEvent } from "./store";
import { decimal, list, record, str, OperatorWireError } from "./wire";

export type ComponentName = "hub" | "serve" | "factory";
export type ComponentState = "up" | "degraded" | "down" | "unknown";
export type MonitoringState = "enabled" | "disabled" | "not_capable";
export type PresenceState = "observed" | "silenced" | "grace" | "unobserved" | "pending_first_report";

export interface PresenceComponent {
  component: ComponentName;
  state: ComponentState;
  observedAt: string;
}

export interface MachinePresence {
  machineId: string;
  hubId: string;
  monitoring: MonitoringState;
  monitoringGeneration: string;
  presence: PresenceState | null;
  lastReportAt: string | null;
  leaseExpiresAt: string | null;
  deadlineAt: string | null;
  silence: { kind: "sleep" | "reboot" | "maintenance"; until: string } | null;
  components: PresenceComponent[];
  openOutage: { epoch: string; openedAt: string; eventId: string } | null;
}

export interface PresenceSnapshot {
  observerStatus: "ok" | "unavailable";
  observerCheckedAt: string | null;
  machines: MachinePresence[];
}

export interface PresenceNotice {
  eventId: string;
  machineId: string;
  hubId: string;
  kind: "machine_unobserved" | "machine_recovered";
  outageEpoch: string;
  refEventId: string | null;
  detectedAt: string;
}

function choice<T extends string>(value: unknown, choices: readonly T[], field: string): T {
  if (typeof value !== "string" || !choices.includes(value as T)) throw new OperatorWireError(0, "invalid_response", { field }, null);
  return value as T;
}

function timestamp(value: unknown, field: string): string {
  const s = str(value, field);
  if (!Number.isFinite(Date.parse(s))) throw new OperatorWireError(0, "invalid_response", { field }, null);
  return s;
}

function optionalTime(value: unknown, field: string): string | null {
  return value === null ? null : timestamp(value, field);
}

/** Pick closed fields rather than retaining raw cloud data in the UI. */
export function parsePresenceSnapshot(value: unknown): PresenceSnapshot {
  const snapshot = record(value, "presence snapshot");
  if (snapshot.wire_version !== 1) throw new OperatorWireError(0, "invalid_response", { field: "wire_version" }, null);
  const ids = new Set<string>();
  const machines = list(snapshot.machines, "machines").map((entry): MachinePresence => {
    const row = record(entry, "machine presence");
    const machineId = str(row.machine_id, "machine_id");
    if (!machineId || ids.has(machineId)) throw new OperatorWireError(0, "invalid_response", { field: "machine_id" }, null);
    ids.add(machineId);
    const monitoring = choice(row.monitoring, ["enabled", "disabled", "not_capable"], "monitoring");
    const componentIds = new Set<ComponentName>();
    const components = (row.components === null ? [] : list(row.components, "components")).map((entry): PresenceComponent => {
      const part = record(entry, "component");
      const component = choice(part.component, ["hub", "serve", "factory"], "component");
      if (componentIds.has(component)) throw new OperatorWireError(0, "invalid_response", { field: "component" }, null);
      componentIds.add(component);
      return { component, state: choice(part.state, ["up", "degraded", "down", "unknown"], "component state"), observedAt: timestamp(part.observed_at, "observed_at") };
    });
    const silence = row.silence === null ? null : record(row.silence, "silence");
    const outage = row.open_outage === null ? null : record(row.open_outage, "open_outage");
    const presence = row.presence === null ? null : choice(row.presence, ["observed", "silenced", "grace", "unobserved", "pending_first_report"], "presence");
    if ((monitoring === "enabled") !== (presence !== null)) throw new OperatorWireError(0, "invalid_response", { field: "presence" }, null);
    return {
      machineId, hubId: str(row.hub_id, "hub_id"), monitoring,
      monitoringGeneration: decimal(row.monitoring_generation, "monitoring_generation"), presence,
      lastReportAt: optionalTime(row.last_report_at, "last_report_at"),
      leaseExpiresAt: optionalTime(row.lease_expires_at, "lease_expires_at"),
      deadlineAt: optionalTime(row.deadline_at, "deadline_at"), components,
      silence: silence ? { kind: choice(silence.kind, ["sleep", "reboot", "maintenance"], "silence kind"), until: timestamp(silence.until, "silence until") } : null,
      openOutage: outage ? { epoch: decimal(outage.outage_epoch, "outage_epoch"), openedAt: timestamp(outage.opened_at, "opened_at"), eventId: str(outage.unobserved_event_id, "unobserved_event_id") } : null,
    };
  });
  return {
    observerStatus: choice(snapshot.observer_status, ["ok", "unavailable"], "observer_status"),
    observerCheckedAt: optionalTime(snapshot.observer_checked_at, "observer_checked_at"), machines,
  };
}

/** Persisted, verified machine notices only; repeated replay is one notice. */
export function presenceNotices(events: readonly InboxEvent[]): PresenceNotice[] {
  const notices: PresenceNotice[] = [];
  const seen = new Set<string>();
  const ordered = [...events].sort((a, b) => BigInt(a.sequence) < BigInt(b.sequence) ? -1 : 1);
  for (const event of ordered) {
    if (event.verification !== "verified" || event.scope !== "machine" || event.producerKind !== "cloud_observer") continue;
    try {
      const plain = record(event.plaintext, "machine notice");
      if (plain.type !== "psc.operator.machine_presence" || plain.v !== 1 || plain.account_id !== event.accountId || plain.hub_id !== event.hubId) continue;
      const kind = choice(plain.kind, ["machine_unobserved", "machine_recovered"], "notice kind");
      const machineId = str(plain.machine_id, "machine_id");
      const outageEpoch = decimal(plain.outage_epoch, "outage_epoch");
      const refEventId = plain.ref_event_id === null || plain.ref_event_id === undefined ? null : str(plain.ref_event_id, "ref_event_id");
      if ((kind === "machine_recovered") !== !!refEventId || !machineId || outageEpoch === "0") continue;
      const key = `${machineId}:${outageEpoch}:${kind}`;
      if (seen.has(key)) continue;
      const detectedAt = timestamp(plain.detected_at, "detected_at");
      seen.add(key);
      notices.push({ eventId: event.eventId, machineId, hubId: event.hubId, kind, outageEpoch, refEventId, detectedAt });
    } catch { /* A refused shape never becomes a visible machine notice. */ }
  }
  return notices;
}

export function presenceLabel(machine: MachinePresence): string {
  if (machine.monitoring === "not_capable") return "Monitoring is not available for this machine";
  if (machine.monitoring === "disabled") return "Monitoring is off";
  switch (machine.presence) {
    case "observed": return "Reporting to Cassy Cloud";
    case "unobserved": return "Unreachable from Cassy Cloud";
    case "grace": return "A report is overdue; waiting before an alert";
    case "pending_first_report": return "Waiting for the first report";
    case "silenced": return `Alerts paused for ${machine.silence?.kind ?? "maintenance"}`;
    default: return "Status unavailable";
  }
}
