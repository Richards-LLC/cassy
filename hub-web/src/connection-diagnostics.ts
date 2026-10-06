import type { ConnectionSnapshot } from "./connection-state";
export type ConnectionCause = "browser_offline" | "network_or_browser_policy_unknown" | "health_http_unavailable" | "request_timeout" | "auth_expired" | "auth_revoked" | "auth_scope_mismatch" | "auth_needs_pairing" | "proof_refused" | "http_refused" | "socket_closed" | "session_upstream_unavailable" | "event_sequence_gap" | "event_retention_gap" | "event_epoch_changed" | "viewer_lagged" | "stream_closed" | "unsupported_browser";
export interface CauseEvidence {
  code: ConnectionCause;
  layer: "browser" | "http" | "auth" | "socket" | "session" | "events";
  retryable: boolean;
  status?: number;
  closeCode?: number;
  permission?: "denied" | "prompt" | "granted" | "unknown";
  requestId?: string;
  reason_code?: ConnectionCause;
}
/**
 * Each cause in the operator's words: `title` names it, `action` is the next
 * step, and `detail` (cas-97d58 F08) is the engineering note that belongs
 * behind the Connection log's Technical details, never in its lead.
 */
export const CAUSE_COPY: Record<ConnectionCause, { title: string; action: string; detail?: string }> = {
  browser_offline: { title: "Browser reports offline", action: "Restore this device's network; Commander retries when it returns." },
  network_or_browser_policy_unknown: { title: "Network or browser policy blocked the request", action: "Check that Tailscale is connected on this device and that this site may reach local networks.", detail: "DNS, TLS, CORS and local-network policy are indistinguishable to this page." },
  health_http_unavailable: { title: "Health probe returned HTTP 503", action: "This browser received an unavailable response. Compare its network route with hub diagnostics; another device's successful health check does not prove this route works." },
  request_timeout: { title: "Request deadline exceeded", action: "Commander retries reads. A timed-out action may have reached the hub; check its result before repeating it." },
  auth_expired: { title: "Hub says access expired", action: "Refresh access or pair this device again." },
  auth_revoked: { title: "Hub says access was revoked", action: "Pair this device again with the machine's approval." },
  auth_scope_mismatch: { title: "Hub refused the required permission", action: "Approve the required scope on the machine." },
  auth_needs_pairing: { title: "Hub refused this credential", action: "Pair this device again." },
  proof_refused: { title: "Hub refused a fresh proof", action: "Commander retries with the hub's clock. Check this device's clock if it persists." },
  http_refused: { title: "HTTP request was refused", action: "Check the measured HTTP status and hub diagnostics." },
  socket_closed: { title: "The live link to the machine dropped", action: "Commander reconnects it by itself.", detail: "Terminal socket closed; the HTTP hub may still be healthy." },
  session_upstream_unavailable: { title: "Session's upstream is unavailable", action: "Commander retries this session. Check that its supervisor is running." },
  event_sequence_gap: { title: "Event sequence gap detected", action: "Commander reopens retained events and refreshes the session catalog." },
  event_retention_gap: { title: "Some events are outside hub retention", action: "Commander refreshes current state. Expired events cannot be replayed." },
  event_epoch_changed: { title: "Hub event stream restarted", action: "Commander resets the event cursor and refreshes current state." },
  viewer_lagged: { title: "Event viewer fell behind", action: "Commander replays retained events and refreshes current state." },
  stream_closed: { title: "Hub event stream closed", action: "Commander reconnects automatically." },
  unsupported_browser: { title: "Browser cannot support this connection", action: "Open Commander in a supported browser." },
};
interface TransitionRecord {
  scope: "machine" | "session";
  at: number; phase: ConnectionSnapshot["phase"]; stage: ConnectionSnapshot["stage"]; attempt: number;
  generation: number; cause?: CauseEvidence; nextRetryAt?: number; lastSuccessAt?: number; recovered?: ConnectionCause;
}
/** Typed projection only: no raw error, report, path, machine/session label or message. */
export class ConnectionDiagnostics {
  private records: TransitionRecord[] = [];
  private failed = new Map<string, ConnectionCause>();
  private hubBuild?: string;
  build(version: string): void { if (/^[a-zA-Z0-9.+-]{1,64}$/.test(version)) this.hubBuild = version; }
  record(snapshot: ConnectionSnapshot, generation: number, source = "machine"): void {
    const c = snapshot.cause;
    const cause: CauseEvidence | undefined = c ? { code: c.code, reason_code: c.code, layer: c.layer, retryable: c.retryable,
      status: c.status, closeCode: c.closeCode, permission: c.permission,
      requestId: c.requestId && /^[a-f0-9-]{36}$/.test(c.requestId) ? c.requestId : undefined } : undefined;
    const recovered = snapshot.phase === "live" && !snapshot.degraded && !cause ? this.failed.get(source) : undefined;
    if (cause) { this.failed.set(source, cause.code); if (this.failed.size > 64) this.failed.delete(this.failed.keys().next().value!); }
    if (recovered || snapshot.phase === "idle") this.failed.delete(source);
    this.records.push({ scope: source === "machine" ? "machine" : "session", at: snapshot.since, phase: snapshot.phase, stage: snapshot.stage, attempt: snapshot.attempt, generation,
      cause, nextRetryAt: snapshot.nextRetryAt, lastSuccessAt: snapshot.lastSuccessAt, recovered });
    this.records = this.records.slice(-64);
  }
  export(hub: unknown, online: boolean | undefined): Record<string, unknown> {
    const input = typeof hub === "object" && hub !== null ? hub as Record<string, unknown> : {};
    const summary = typeof input.connection_recovery === "object" && input.connection_recovery !== null ? input.connection_recovery as Record<string, unknown> : {};
    const counts = Array.isArray(summary.counts) ? summary.counts.slice(0, 64).flatMap(value => {
      if (typeof value !== "object" || !value) return [];
      const row = value as Record<string, unknown>;
      const category = typeof row.category === "string" && /^(health|machine|sessions|events|auth|diagnostics|other)$/.test(row.category) ? row.category : "other";
      const requestId = typeof row.request_id === "string" && /^[a-f0-9-]{36}$/.test(row.request_id) ? row.request_id : undefined;
      return [{ category, preflight: row.preflight === true, status: typeof row.status === "number" ? row.status : undefined,
        count: typeof row.count === "number" ? row.count : undefined, request_id: requestId }];
    }) : [];
    const refusals = typeof summary.refusals === "object" && summary.refusals !== null
      ? Object.fromEntries(Object.entries(summary.refusals).filter(([key, value]) => /^(expired|revoked|unknown_credential|scope_mismatch|stale_proof|proof_replay|invalid_proof|other|viewer_lagged)$/.test(key) && typeof value === "number").slice(0, 16)) : {};
    const permission = [...this.records].reverse().find(row => row.cause?.permission !== undefined)?.cause?.permission ?? "unknown";
    return { schema_version: 1, client_protocol: 2, client_build: typeof __HUB_BUILD__ === "undefined" ? "unknown" : __HUB_BUILD__, hub_build: this.hubBuild ?? "unknown", browser: { online: online ?? "unknown", local_permission: permission },
      transitions: this.records, hub: { measured: counts.length > 0, counts, refusals },
      limits: { transitions: 64, hub_buckets: 64, bytes: 65_536 },
      uncertainty: "Browser fetch failures do not distinguish DNS, TLS, CORS or local-network policy. A healthy health probe does not prove authenticated routes work." };
  }
}
