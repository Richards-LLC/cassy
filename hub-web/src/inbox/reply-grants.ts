// Proposed Cloud #174 contract. Dormant until the existing introspection route
// advertises the complete wire version; no speculative production endpoints.
import { decimal, list, OperatorWireError, record, str, type CommandScope, type OperatorClient } from "./wire";

export const REPLY_UPGRADE_FEATURE = "reply-grant-upgrade-v1";
export interface ReplyTarget { hubId: string; projectId: string; sessionId: string }
export interface ReplyRequest {
  request_id: string; device_id: string; device_label: string; origin: string;
  hub_id: string; project_id: string; session_id: string;
  requested_grant_generation: string;
  status: "pending" | "approved" | "denied" | "expired";
  created_at: string; expires_at: string; decided_at: string | null;
}
export interface ReplyGrantsSnapshot { supported: boolean; owner: boolean; deviceId: string | null; requests: ReplyRequest[] }

export function replyScopeCovers(scopes: readonly CommandScope[], target: ReplyTarget): boolean {
  return scopes.some(s => s.hub_id === target.hubId && s.project_id === target.projectId &&
    (s.session_id === null || s.session_id === target.sessionId) && s.operations.includes("operator_message"));
}
function receipt(value: unknown): ReplyRequest {
  const r = record(value, "reply_request");
  const result = Object.fromEntries(["request_id", "device_id", "device_label", "origin", "hub_id", "project_id", "session_id", "created_at", "expires_at"].map(k => [k, str(r[k], k)]));
  if (!["pending", "approved", "denied", "expired"].includes(String(r.status)) ||
    !Number.isFinite(Date.parse(result.created_at)) || !Number.isFinite(Date.parse(result.expires_at))) {
    throw new OperatorWireError(0, "invalid_response", { field: "reply_request" }, null);
  }
  return { ...result, requested_grant_generation: decimal(r.requested_grant_generation, "requested_grant_generation"),
    status: r.status, decided_at: r.decided_at === null ? null : str(r.decided_at, "decided_at") } as ReplyRequest;
}

export class ReplyGrantRequests {
  supported = false;
  owner = false;
  private deviceId: string | null = null;
  private revision = 0;
  requests: ReplyRequest[] = [];
  private readonly asks = new Map<string, Record<string, unknown>>();
  private readonly decisions = new Map<string, Record<string, unknown>>();
  constructor(private readonly client: OperatorClient) {}
  snapshot(): ReplyGrantsSnapshot { return { supported: this.supported, owner: this.owner, deviceId: this.deviceId, requests: [...this.requests] }; }
  configure(features: unknown, owner: boolean, deviceId: string | null = null): void {
    if (this.deviceId !== deviceId) this.revision++;
    this.deviceId = deviceId;
    this.supported = Array.isArray(features) && features.includes(REPLY_UPGRADE_FEATURE);
    this.owner = owner;
    if (!this.supported) this.requests = [];
  }
  clear(): void { this.revision++; this.deviceId = null; this.supported = false; this.owner = false; this.requests = []; this.asks.clear(); this.decisions.clear(); }
  async refresh(deviceId: string): Promise<void> {
    if (!this.supported) return;
    const revision = this.revision;
    const body = await this.client.request({ method: "GET", path: "/api/operator/reply-grant-requests", auth: "grant" });
    if (revision !== this.revision) return;
    if (body.wire_version !== 1) throw new OperatorWireError(0, "invalid_response", {}, null);
    const requests = list(body.requests, "requests").map(receipt);
    if (!this.owner && requests.some(r => r.device_id !== deviceId)) throw new OperatorWireError(0, "invalid_response", {}, null);
    this.requests = requests;
  }
  latest(target: ReplyTarget): ReplyRequest | undefined {
    return [...this.requests].reverse().find(r => r.device_id === this.deviceId && r.hub_id === target.hubId && r.project_id === target.projectId && r.session_id === target.sessionId);
  }
  private async mutate(path: string, body: Record<string, unknown>): Promise<ReplyRequest> {
    if (!this.supported) throw new Error("Reply requests are not available from this cloud.");
    const revision = this.revision;
    const response = await this.client.request({ method: "POST", path, body, auth: "grant" });
    if (revision !== this.revision) throw new Error("The browser was signed out while the request was pending.");
    if (response.wire_version !== 1) throw new OperatorWireError(0, "invalid_response", {}, null);
    const next = receipt(response.request);
    this.requests = [...this.requests.filter(r => r.request_id !== next.request_id), next];
    return next;
  }
  async ask(target: ReplyTarget, generation: string): Promise<ReplyRequest> {
    const key = JSON.stringify(target);
    const existing = this.latest(target);
    if (existing?.status === "pending") return existing;
    let body = this.asks.get(key);
    if (!body) {
      body = { wire_version: 1, request_id: crypto.randomUUID(), hub_id: target.hubId, project_id: target.projectId,
        session_id: target.sessionId, expected_grant_generation: generation };
      this.asks.set(key, body);
    }
    const response = await this.mutate("/api/operator/reply-grant-requests", body);
    this.asks.delete(key);
    return response;
  }
  async decide(request: ReplyRequest, decision: "approve" | "deny"): Promise<ReplyRequest> {
    const key = `${request.request_id}:${decision}`;
    let body = this.decisions.get(key);
    if (!body) {
      body = { wire_version: 1, decision_id: crypto.randomUUID(), decision,
        expected_grant_generation: request.requested_grant_generation };
      this.decisions.set(key, body);
    }
    const response = await this.mutate(`/api/operator/reply-grant-requests/${encodeURIComponent(request.request_id)}/decision`, body);
    this.decisions.delete(key);
    return response;
  }
}
