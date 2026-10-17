// Cloud #174 proposal double: real caller PoP is verified by OperatorCloudDouble
// before this handler. This is contract choreography, not deployed acceptance.
import type { ReplyRequest } from "../src/inbox/reply-grants";

interface Device {
  id: string; label: string; origin: string; capabilities: string[];
  scopes: { hub_id: string; project_id: string; session_id: string | null; operations: string[] }[];
  generation: bigint; status: string; expiresAt: number;
}
const answer = (status: number, body: Record<string, unknown>) => ({ status, body });
const failure = (status: number, error: string) => answer(status, { error });
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
export class ReplyGrantDouble {
  enabled = false;
  readonly requests = new Map<string, ReplyRequest>();
  private readonly creates = new Map<string, { digest: string; requestId: string }>();
  private readonly decisions = new Map<string, { digest: string; receipt: ReplyRequest }>();
  constructor(private readonly now: () => number, private readonly device: (id: string) => Device | undefined,
    private readonly ownsTarget: (hub: string, project: string) => boolean) {}
  private expire(): void {
    for (const [id, r] of this.requests) {
      if (Date.parse(r.created_at) + 86_400_000 < this.now()) { this.requests.delete(id); continue; }
      if (r.status === "pending" && Date.parse(r.expires_at) <= this.now()) r.status = "expired";
    }
  }
  route(method: string, path: string, bytes: Uint8Array, caller: Device) {
    if (!this.enabled) return failure(404, "not_found");
    this.expire();
    const owner = caller.capabilities.includes("account:manage");
    if (method === "GET" && path === "/api/operator/reply-grant-requests") {
      return answer(200, { wire_version: 1, requests: [...this.requests.values()].filter(r => owner || r.device_id === caller.id) });
    }
    let body: Record<string, unknown>;
    try { body = JSON.parse(new TextDecoder().decode(bytes)); } catch { return failure(400, "invalid_request"); }
    if (!body || body.wire_version !== 1) return failure(400, "invalid_request");
    const digest = JSON.stringify(body, Object.keys(body).sort());
    if (method === "POST" && path === "/api/operator/reply-grant-requests") {
      const id = String(body.request_id ?? "");
      if (!UUID.test(id) || !caller.capabilities.includes("feed:read")) return failure(400, "invalid_request");
      const key = `${caller.id}:${id}`;
      const prior = this.creates.get(key);
      if (prior) return prior.digest === digest
        ? answer(200, { wire_version: 1, request: this.requests.get(prior.requestId) }) : failure(409, "reply_request_conflict");
      if (body.expected_grant_generation !== caller.generation.toString()) return failure(409, "reply_grant_generation_conflict");
      if (typeof body.session_id !== "string" || !/^s_[A-Za-z0-9_-]{43}$/.test(body.session_id) || typeof body.hub_id !== "string" || typeof body.project_id !== "string" ||
        !this.ownsTarget(body.hub_id, body.project_id) || "capabilities" in body || "operations" in body) return failure(403, "scope_not_owned");
      const pending = [...this.requests.values()].find(r => r.device_id === caller.id && r.hub_id === body.hub_id &&
        r.project_id === body.project_id && r.session_id === body.session_id && r.status === "pending");
      if (pending) { this.creates.set(key, { digest, requestId: pending.request_id }); return answer(200, { wire_version: 1, request: pending }); }
      const r: ReplyRequest = { request_id: id, device_id: caller.id, device_label: caller.label, origin: caller.origin,
        hub_id: body.hub_id, project_id: body.project_id, session_id: body.session_id,
        requested_grant_generation: caller.generation.toString(), status: "pending", created_at: new Date(this.now()).toISOString(),
        expires_at: new Date(this.now() + 600_000).toISOString(), decided_at: null };
      this.requests.set(id, r); this.creates.set(key, { digest, requestId: id });
      return answer(200, { wire_version: 1, request: r });
    }
    const match = /^\/api\/operator\/reply-grant-requests\/([^/]+)\/decision$/.exec(path);
    if (!match || method !== "POST") return failure(404, "not_found");
    if (!owner) return failure(403, "insufficient_authority");
    const r = this.requests.get(match[1]);
    if (!r) return failure(404, "not_found");
    if (!UUID.test(String(body.decision_id)) || !["approve", "deny"].includes(String(body.decision))) return failure(400, "invalid_request");
    const key = `${r.request_id}:${body.decision_id}`;
    const prior = this.decisions.get(key);
    if (prior) return prior.digest === digest ? answer(200, { wire_version: 1, request: prior.receipt }) : failure(409, "reply_decision_conflict");
    if (r.status !== "pending") return failure(409, "reply_decision_conflict");
    const target = this.device(r.device_id);
    if (!target || target.status !== "active" || target.expiresAt <= this.now() || !this.ownsTarget(r.hub_id, r.project_id)) return failure(404, "not_found");
    if (body.expected_grant_generation !== r.requested_grant_generation || target.generation.toString() !== r.requested_grant_generation) return failure(409, "reply_grant_generation_conflict");
    if (body.decision === "approve") {
      if (!target.scopes.some(s => s.hub_id === r.hub_id && s.project_id === r.project_id &&
        (s.session_id === null || s.session_id === r.session_id) && s.operations.includes("operator_message"))) {
        target.scopes.push({ hub_id: r.hub_id, project_id: r.project_id, session_id: r.session_id, operations: ["operator_message"] });
      }
      target.generation++;
    }
    r.status = body.decision === "approve" ? "approved" : "denied";
    r.decided_at = new Date(this.now()).toISOString();
    const receipt = structuredClone(r);
    this.decisions.set(key, { digest, receipt });
    return answer(200, { wire_version: 1, request: receipt });
  }
}
