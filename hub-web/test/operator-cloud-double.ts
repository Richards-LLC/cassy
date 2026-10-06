// A protocol double of the petra-stella-cloud operator inbox (wire v1).
//
// It implements the device-facing routes of the cloud contract
// (docs/specs/2026-10-05-operator-inbox-cloud-contract.md, snapshot in the
// cas-9b7d task artifacts) with the real rules that the client depends on:
// PoP verification in §4.2 order, one-use enrollment challenges with the HPKE
// key check, per-device epoch wraps made on demand, gap-free sequences with
// expired intervals, per-device ACKs and cursors, monotonic read marks, and
// command intake with an atomic history event. It is a double, not the cloud:
// the S5 journey against the deployed cloud is the acceptance gate.
//
// Use `double.fetch` from vitest (it plays the browser `Origin`), or
// `serveOperatorCloudDouble` (operator-cloud-double-server.ts) from Playwright.

import {
  ENROLLMENT_CHECK_INFO,
  EPOCH_WRAP_INFO,
  b64urlDecode,
  b64urlEncode,
  deriveKeyPair,
  digest,
  epochWrapAad,
  exportPublicKey,
  importPublicKey,
  operatorSuite,
  seal,
  sealEvent,
  sealObserverNoticeWith,
  type EventIds,
} from "../src/inbox/hpke";
import { EMPTY_BODY_DIGEST, bodyDigest, jwkThumbprint } from "../src/inbox/pop";

const encoder = new TextEncoder();
const decoder = new TextDecoder();
const DECIMAL = /^(0|[1-9][0-9]{0,18})$/;
const DAY_MS = 86_400_000;

export interface DoubleRequest {
  method: string;
  /** Path and query exactly as sent. */
  path: string;
  headers: Record<string, string>;
  body: Uint8Array;
}

const RESPONSE = Symbol("double-response");

export interface DoubleResponse {
  [RESPONSE]: true;
  status: number;
  headers: Record<string, string>;
  body: Record<string, unknown>;
}

function isResponse(value: unknown): value is DoubleResponse {
  return typeof value === "object" && value !== null && RESPONSE in value;
}

interface Epoch {
  epoch: bigint;
  feedGeneration: bigint;
  secret: Uint8Array;
  publicRaw: Uint8Array;
  publicKey: CryptoKey;
  status: "active" | "retired" | "destroyed";
  activatedAt: string;
  retiredAt: string | null;
}

interface Device {
  id: string;
  label: string;
  origin: string;
  signingJwk: JsonWebKey;
  jkt: string;
  encryptionPublic: Uint8Array;
  capabilities: string[];
  scopes: { hub_id: string; project_id: string; session_id: string | null; operations: string[] }[];
  generation: bigint;
  status: "active" | "revoked";
  expiresAt: number;
  createdAt: string;
  revokedAt: string | null;
}

interface Machine {
  id: string;
  hubId: string;
  label: string;
  projects: string[];
  commandPublic: Uint8Array;
  commandKeyId: string;
  generation: bigint;
  status: "active" | "revoked";
}

interface StoredEvent {
  sequence: bigint;
  eventId: string;
  feedGeneration: bigint;
  scope: "session" | "machine";
  producerKind: "principal" | "cloud_observer";
  hubId: string;
  projectId: string | null;
  sessionId: string | null;
  keyEpoch: bigint;
  ciphertext: Uint8Array;
  digest: string;
  storedAt: number;
  expiresAt: number;
  observerAssertion: string | null;
}

interface Enrollment {
  id: string;
  userCode: string;
  pollSecret: string;
  origin: string;
  label: string;
  signingJwk: JsonWebKey;
  jkt: string;
  encryptionPublic: Uint8Array;
  checkPlain: string;
  status: "pending" | "approved" | "denied" | "completed";
  expiresAt: number;
  lastPolledAt: number;
  capabilities: string[];
  scopes: Device["scopes"];
}

interface Command {
  commandId: string;
  machineId: string;
  deviceId: string;
  hubId: string;
  projectId: string;
  sessionId: string;
  operation: string;
  machineDigest: string;
  machineCiphertext: string;
  historyEventId: string;
  historyDigest: string;
  historySequence: bigint;
  status: "pending_machine" | "reserved" | "accepted" | "rejected_by_machine" | "cancelled" | "expired";
  createdAt: number;
  expiresAt: number;
}

export interface DoubleOptions {
  /** The PSC base URL the double claims to be (PoP `aud` prefix and `iss`). */
  baseUrl?: string;
  allowedOrigins?: string[];
  accountId?: string;
  now?: () => number;
}

function json(status: number, body: Record<string, unknown>, headers: Record<string, string> = {}): DoubleResponse {
  return { [RESPONSE]: true, status, headers: { "Cache-Control": "no-store", ...headers }, body: { ...body } };
}

function error(status: number, code: string, extra: Record<string, unknown> = {}, headers: Record<string, string> = {}): DoubleResponse {
  return json(status, { error: code, error_description: code.replaceAll("_", " "), ...extra }, headers);
}

function randomId(bytes = 16): string {
  return b64urlEncode(crypto.getRandomValues(new Uint8Array(bytes)));
}

function uuid(): string {
  return crypto.randomUUID();
}

const CODE_ALPHABET = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
function userCode(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(8));
  const chars = Array.from(bytes, (value) => CODE_ALPHABET[value % CODE_ALPHABET.length]);
  return `${chars.slice(0, 4).join("")}-${chars.slice(4).join("")}`;
}

function iso(ms: number): string {
  return new Date(ms).toISOString();
}

export class OperatorCloudDouble {
  readonly baseUrl: string;
  readonly audience: string;
  readonly accountId: string;
  readonly allowedOrigins: string[];
  private readonly clock: () => number;

  private issuer: { privateKey: CryptoKey; jwk: JsonWebKey; kid: string } | null = null;
  feedGeneration = 1n;
  startSequence = 1n;
  head = 0n;
  policyVersion = 1n;
  readonly epochs = new Map<bigint, Epoch>();
  activeEpoch = 0n;
  readonly devices = new Map<string, Device>();
  readonly machines = new Map<string, Machine>();
  // Presence snapshots are client-choreography fixtures, not a watchdog or
  // evidence that deployed cloud leases work. Notices use the real crypto.
  readonly presence = new Map<string, Record<string, unknown>>();
  observerStatus: "ok" | "unavailable" = "ok";
  private readonly monitoringDecisions = new Map<string, { body: string; response: Record<string, unknown> }>();
  readonly events: StoredEvent[] = [];
  readonly tombstones = new Map<string, { sequence: bigint; digest: string }>();
  readonly enrollments = new Map<string, Enrollment>();
  readonly acks = new Map<string, Map<string, string>>();
  readonly cursors = new Map<string, { cursor: bigint | null; acceptedExpiredThrough: bigint | null; updatedAt: string }>();
  readonly readMarks = new Map<string, { hub_id: string; project_id: string; session_id: string; sequence: bigint; updatedAt: string; by: string }>();
  readonly commands = new Map<string, Command>();
  private readonly nonces = new Set<string>();
  /** Every request seen, for assertions (method, path, status). */
  readonly log: { method: string; path: string; status: number; code?: string }[] = [];
  /** Inject a one-shot failure for the next request matching `path` prefix. */
  readonly faults: { method: string; pathPrefix: string; status: number; code: string; times: number }[] = [];
  /** Deny the next requests with 503 when true (cloud outage). */
  outage = false;

  constructor(options: DoubleOptions = {}) {
    this.baseUrl = options.baseUrl ?? "https://psc-double.test";
    this.audience = `${this.baseUrl}/api/operator`;
    this.accountId = options.accountId ?? "acct-double-0001";
    this.allowedOrigins = options.allowedOrigins ?? ["https://hub.petrastella.io", "http://127.0.0.1"];
    this.clock = options.now ?? Date.now;
  }

  now(): number {
    return this.clock();
  }

  // ---------------------------------------------------------------- issuer

  async issuerKey() {
    if (!this.issuer) {
      const pair = (await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, ["sign", "verify"])) as CryptoKeyPair;
      const jwk = await crypto.subtle.exportKey("jwk", pair.publicKey);
      this.issuer = { privateKey: pair.privateKey, jwk, kid: "op-double-1" };
    }
    return this.issuer;
  }

  async jwks() {
    const { jwk, kid } = await this.issuerKey();
    return { keys: [{ kty: "EC", crv: "P-256", x: jwk.x, y: jwk.y, kid, alg: "ES256", use: "sig" }] };
  }

  async sign(typ: string, claims: Record<string, unknown>): Promise<string> {
    const { privateKey, kid } = await this.issuerKey();
    const header = b64urlEncode(encoder.encode(JSON.stringify({ typ, alg: "ES256", kid })));
    const payload = b64urlEncode(encoder.encode(JSON.stringify(claims)));
    const signature = await crypto.subtle.sign({ name: "ECDSA", hash: "SHA-256" }, privateKey, encoder.encode(`${header}.${payload}`));
    return `${header}.${payload}.${b64urlEncode(signature)}`;
  }

  // ---------------------------------------------------------------- epochs

  private async newEpoch(): Promise<Epoch> {
    const next = this.activeEpoch + 1n;
    const pair = await deriveKeyPair(crypto.getRandomValues(new Uint8Array(32)));
    const secret = new Uint8Array(await operatorSuite().kem.serializePrivateKey(pair.privateKey));
    const epoch: Epoch = {
      epoch: next,
      feedGeneration: this.feedGeneration,
      secret,
      publicRaw: await exportPublicKey(pair.publicKey),
      publicKey: pair.publicKey,
      status: "active",
      activatedAt: iso(this.now()),
      retiredAt: null,
    };
    const previous = this.epochs.get(this.activeEpoch);
    if (previous) {
      previous.status = "retired";
      previous.retiredAt = iso(this.now());
    }
    this.epochs.set(next, epoch);
    this.activeEpoch = next;
    this.policyVersion += 1n;
    return epoch;
  }

  async ensureAccount() {
    if (this.activeEpoch === 0n) await this.newEpoch();
  }

  async manifest(epoch: Epoch): Promise<string> {
    const iat = Math.floor(this.now() / 1000);
    return this.sign("psc-op-epoch-manifest+jwt", {
      acct: this.accountId,
      fgen: epoch.feedGeneration.toString(),
      epoch: epoch.epoch.toString(),
      status: epoch.status,
      suite: { kem: 16, kdf: 1, aead: 2 },
      pk: b64urlEncode(epoch.publicRaw),
      policy_version: this.policyVersion.toString(),
      activated_at: epoch.activatedAt,
      retired_at: epoch.retiredAt,
      upload_state: "open",
      iat,
      exp: iat + 86_400,
    });
  }

  // ---------------------------------------------------------------- test hooks

  /** The account authority approves a pending device challenge (§5.2). */
  async approve(code: string, options: { capabilities?: string[]; scopes?: Device["scopes"]; deny?: boolean } = {}) {
    const enrollment = [...this.enrollments.values()].find((entry) => entry.userCode === code);
    if (!enrollment || enrollment.status !== "pending") throw new Error(`no pending enrollment ${code}`);
    await this.ensureAccount();
    enrollment.status = options.deny ? "denied" : "approved";
    enrollment.capabilities = ["feed:read", ...(options.capabilities ?? [])];
    enrollment.scopes = options.scopes ?? [];
  }

  /** Register an active machine for `hubId` (as §5.4 would) with a command key. */
  async enrollMachine(hubId: string, projects: string[], commandPublic: Uint8Array, label = "soundwave"): Promise<Machine> {
    await this.ensureAccount();
    for (const machine of this.machines.values()) {
      if (machine.hubId === hubId && machine.status === "active") machine.status = "revoked";
    }
    const machine: Machine = {
      id: uuid(),
      hubId,
      label,
      projects,
      commandPublic,
      commandKeyId: b64urlEncode(await crypto.subtle.digest("SHA-256", commandPublic as BufferSource)),
      generation: 1n,
      status: "active",
    };
    this.machines.set(machine.id, machine);
    return machine;
  }

  async machineBinding(machine: Machine): Promise<string> {
    const iat = Math.floor(this.now() / 1000);
    return this.sign("psc-op-machine-binding+jwt", {
      acct: this.accountId,
      mch: machine.id,
      hub: machine.hubId,
      sig_jkt: "machine-signing-jkt",
      cmd_pk: b64urlEncode(machine.commandPublic),
      cmd_kid: machine.commandKeyId,
      projects: machine.projects,
      gen: machine.generation.toString(),
      iat,
      exp: iat + 86_400,
    });
  }

  private storeEvent(event: Omit<StoredEvent, "sequence" | "storedAt" | "expiresAt" | "feedGeneration">): StoredEvent {
    this.head += 1n;
    const stored: StoredEvent = {
      ...event,
      sequence: this.head,
      feedGeneration: this.feedGeneration,
      storedAt: this.now(),
      expiresAt: this.now() + 90 * DAY_MS,
    };
    this.events.push(stored);
    return stored;
  }

  /** A machine append (§7.1) of a session event sealed under the active epoch. */
  async appendSessionEvent(input: { hubId: string; projectId: string; sessionId: string; plaintext: unknown; eventId?: string }) {
    await this.ensureAccount();
    const epoch = this.epochs.get(this.activeEpoch)!;
    const eventId = input.eventId ?? randomId();
    const ids: EventIds = {
      accountId: this.accountId,
      feedGeneration: this.feedGeneration.toString(),
      keyEpoch: epoch.epoch.toString(),
      eventId,
      hubId: input.hubId,
      projectId: input.projectId,
      sessionId: input.sessionId,
    };
    const sealed = await sealEvent(epoch.publicKey, encoder.encode(JSON.stringify(input.plaintext)), ids);
    return this.storeEvent({
      eventId,
      scope: "session",
      producerKind: "principal",
      hubId: input.hubId,
      projectId: input.projectId,
      sessionId: input.sessionId,
      keyEpoch: epoch.epoch,
      ciphertext: sealed.bytes,
      digest: sealed.digest,
      observerAssertion: null,
    });
  }

  /** A cloud observer notice (§7.4) for `machine`, sealed and asserted. */
  async appendObserverNotice(machine: Machine, kind: "machine_unobserved" | "machine_recovered", options: { tamperClaim?: string; refEventId?: string; outageEpoch?: string } = {}) {
    await this.ensureAccount();
    const epoch = this.epochs.get(this.activeEpoch)!;
    const eventId = randomId();
    const plaintext = {
      type: "psc.operator.machine_presence",
      v: 1,
      kind,
      account_id: this.accountId,
      machine_id: machine.id,
      hub_id: machine.hubId,
      outage_epoch: options.outageEpoch ?? "1",
      ref_event_id: options.refEventId ?? null,
      detected_at: iso(this.now()),
      last_report_at: iso(this.now() - 300_000),
      deadline_at: iso(this.now() - 60_000),
      silence: null,
      components: [],
      outage_opened_at: null,
      recovered_at: null,
    };
    const ids = {
      accountId: this.accountId,
      feedGeneration: this.feedGeneration.toString(),
      keyEpoch: epoch.epoch.toString(),
      eventId,
      hubId: machine.hubId,
    };
    const sealed = await sealObserverNoticeWith(epoch.publicKey, encoder.encode(JSON.stringify(plaintext)), ids, {
      contentKey: crypto.getRandomValues(new Uint8Array(32)),
      nonce: crypto.getRandomValues(new Uint8Array(12)),
    });
    const iat = Math.floor(this.now() / 1000);
    const claims: Record<string, unknown> = {
      iss: this.baseUrl,
      acct: this.accountId,
      mch: machine.id,
      hub: machine.hubId,
      kind,
      outage_epoch: options.outageEpoch ?? "1",
      event_id: eventId,
      digest: sealed.digest,
      fgen: this.feedGeneration.toString(),
      key_epoch: epoch.epoch.toString(),
      iat,
      exp: iat + 91 * 86_400,
    };
    if (options.refEventId) claims.ref_event_id = options.refEventId;
    if (options.tamperClaim) claims[options.tamperClaim] = "tampered";
    const assertion = await this.sign("psc-op-machine-observation+jwt", claims);
    return this.storeEvent({
      eventId,
      scope: "machine",
      producerKind: "cloud_observer",
      hubId: machine.hubId,
      projectId: null,
      sessionId: null,
      keyEpoch: epoch.epoch,
      ciphertext: sealed.bytes,
      digest: sealed.digest,
      observerAssertion: assertion,
    });
  }

  /** Retention: tombstone every event with sequence ≤ `through` (§9.2). */
  expireThrough(through: bigint) {
    for (let index = this.events.length - 1; index >= 0; index -= 1) {
      const event = this.events[index];
      if (event.sequence <= through) {
        this.tombstones.set(event.eventId, { sequence: event.sequence, digest: event.digest });
        this.events.splice(index, 1);
      }
    }
  }

  /** Revoke a device and rotate the epoch atomically (§6.5). */
  async revokeDevice(deviceId: string) {
    const device = this.devices.get(deviceId);
    if (!device) throw new Error(`unknown device ${deviceId}`);
    device.status = "revoked";
    device.generation += 1n;
    device.revokedAt = iso(this.now());
    await this.newEpoch();
  }

  /** Account reset: a new feed generation (§9.5). */
  async resetFeed() {
    for (const event of this.events) this.tombstones.set(event.eventId, { sequence: event.sequence, digest: event.digest });
    this.events.length = 0;
    this.feedGeneration += 1n;
    this.startSequence = this.head + 1n;
    for (const epoch of this.epochs.values()) epoch.status = "destroyed";
    await this.newEpoch();
  }

  retainedFloor(): bigint {
    const live = this.events.filter((event) => event.expiresAt > this.now());
    return live.length === 0 ? this.head + 1n : live.reduce((low, event) => (event.sequence < low ? event.sequence : low), this.head + 1n);
  }

  // ---------------------------------------------------------------- transport

  /** A fetch for vitest. `pageOrigin` plays the browser-set `Origin`. */
  fetchFor(pageOrigin: string): (input: RequestInfo | URL, init?: RequestInit) => Promise<Response> {
    return async (input, init = {}) => {
      const url = new URL(typeof input === "string" || input instanceof URL ? input.toString() : input.url);
      const headers: Record<string, string> = { origin: pageOrigin };
      for (const [key, value] of Object.entries((init.headers ?? {}) as Record<string, string>)) headers[key.toLowerCase()] = value;
      let body: Uint8Array = new Uint8Array();
      if (init.body instanceof Uint8Array) body = new Uint8Array(init.body);
      else if (typeof init.body === "string") body = encoder.encode(init.body);
      const result = await this.handle({ method: (init.method ?? "GET").toUpperCase(), path: `${url.pathname}${url.search}`, headers, body });
      return new Response(JSON.stringify(result.body), {
        status: result.status,
        headers: { "Content-Type": "application/json", Date: new Date(this.now()).toUTCString(), ...result.headers },
      });
    };
  }

  async handle(request: DoubleRequest): Promise<DoubleResponse> {
    let response: DoubleResponse;
    const fault = this.faults.find((entry) => entry.times > 0 && entry.method === request.method && request.path.startsWith(entry.pathPrefix));
    if (fault) {
      fault.times -= 1;
      response = error(fault.status, fault.code, {}, fault.status === 429 ? { "Retry-After": "1" } : {});
    } else if (this.outage) {
      response = error(503, "temporarily_unavailable");
    } else {
      try {
        response = await this.route(request);
      } catch (cause) {
        response = error(503, "temporarily_unavailable", { detail: String(cause) });
      }
    }
    const origin = request.headers.origin;
    if (origin && this.allowedOrigins.some((allowed) => origin === allowed || (allowed === "http://127.0.0.1" && origin.startsWith("http://127.0.0.1:")))) {
      response.headers["Access-Control-Allow-Origin"] = origin;
      response.headers["Access-Control-Expose-Headers"] = "Retry-After";
      response.headers.Vary = "Origin";
    }
    this.log.push({ method: request.method, path: request.path, status: response.status, code: response.body.error as string | undefined });
    return response;
  }

  private originAllowed(origin: string | undefined): boolean {
    if (!origin) return false;
    return this.allowedOrigins.some((allowed) => origin === allowed || (allowed === "http://127.0.0.1" && /^http:\/\/127\.0\.0\.1:\d+$/.test(origin)));
  }

  private parseBody(request: DoubleRequest): Record<string, unknown> | DoubleResponse {
    if (request.headers["content-encoding"]) return error(415, "unsupported_content_encoding");
    let value: unknown;
    try {
      value = JSON.parse(decoder.decode(request.body));
    } catch {
      return error(400, "invalid_request", { field: "body" });
    }
    if (typeof value !== "object" || value === null || Array.isArray(value)) return error(400, "invalid_request", { field: "body" });
    const record = value as Record<string, unknown>;
    if (record.wire_version !== 1) return error(400, "unsupported_wire_version", { supported_versions: [1] });
    return record;
  }

  /** §4.2 checks, in order. Returns the device or an error response. */
  private async authenticate(request: DoubleRequest, options: { allowStaleGeneration?: boolean } = {}): Promise<Device | DoubleResponse> {
    const authorization = request.headers.authorization ?? "";
    const proof = request.headers["psc-pop-proof"];
    const match = /^PSC-PoP (\S+)$/.exec(authorization);
    if (!match || !proof) return error(401, "pop_required");
    const device = this.devices.get(match[1]);
    if (!device) return error(401, "grant_unknown");
    const verified = await this.verifyProof(proof, device.signingJwk, device.jkt, request);
    if (isResponse(verified)) return verified;
    if (verified.gid !== device.id) return error(401, "pop_binding_mismatch");
    if (device.status !== "active") return error(401, "grant_revoked");
    if (device.expiresAt <= this.now()) return error(401, "grant_expired");
    if (!options.allowStaleGeneration && verified.gen !== device.generation.toString()) {
      return error(401, "grant_generation_stale", { current_generation: device.generation.toString() });
    }
    if (request.headers.origin !== device.origin) return error(403, "origin_mismatch");
    return device;
  }

  private async verifyProof(proof: string, jwk: JsonWebKey, jkt: string, request: DoubleRequest): Promise<Record<string, unknown> | DoubleResponse> {
    const parts = proof.split(".");
    if (parts.length !== 3) return error(401, "pop_required");
    let header: Record<string, unknown>;
    let claims: Record<string, unknown>;
    try {
      header = JSON.parse(decoder.decode(b64urlDecode(parts[0], "header")));
      claims = JSON.parse(decoder.decode(b64urlDecode(parts[1], "claims")));
    } catch {
      return error(401, "pop_required");
    }
    if (header.typ !== "psc-op-pop+jwt" || header.alg !== "ES256" || "jwk" in header) return error(401, "pop_invalid");
    if (header.kid !== jkt) return error(401, "pop_key_mismatch");
    const key = await crypto.subtle.importKey("jwk", jwk, { name: "ECDSA", namedCurve: "P-256" }, false, ["verify"]);
    const ok = await crypto.subtle.verify(
      { name: "ECDSA", hash: "SHA-256" },
      key,
      b64urlDecode(parts[2], "signature") as BufferSource,
      encoder.encode(`${parts[0]}.${parts[1]}`),
    );
    if (!ok) return error(401, "pop_invalid");
    const expectedBdg = request.body.length === 0 ? EMPTY_BODY_DIGEST : await bodyDigest(request.body);
    if (claims.aud !== this.audience || claims.htm !== request.method || claims.htp !== request.path || claims.bdg !== expectedBdg) {
      return error(401, "pop_binding_mismatch");
    }
    const now = Math.floor(this.now() / 1000);
    const iat = Number(claims.iat);
    const exp = Number(claims.exp);
    if (!(Math.abs(iat - now) <= 60 && iat < exp && exp <= iat + 120 && exp > now)) return error(401, "pop_expired");
    const nonce = `${jkt}|${String(claims.jti)}`;
    if (typeof claims.jti !== "string" || claims.jti.length < 22 || this.nonces.has(nonce)) return error(401, "pop_replay");
    this.nonces.add(nonce);
    return claims;
  }

  private async route(request: DoubleRequest): Promise<DoubleResponse> {
    const url = new URL(request.path, this.baseUrl);
    const path = url.pathname;
    const method = request.method;
    if (method === "OPTIONS") return json(204, {});
    if (method === "GET" && path === "/api/operator/jwks") return json(200, await this.jwks(), { "Cache-Control": "public, max-age=3600" });

    if (method === "POST" && path === "/api/operator/enrollments") return this.createEnrollment(request);
    let match = /^\/api\/operator\/enrollments\/([^/]+)\/poll$/.exec(path);
    if (method === "POST" && match) return this.pollEnrollment(request, match[1]);
    match = /^\/api\/operator\/enrollments\/([^/]+)\/complete$/.exec(path);
    if (method === "POST" && match) return this.completeEnrollment(request, match[1]);

    const device = await this.authenticate(request, { allowStaleGeneration: method === "GET" && path === "/api/operator/grants/me" });
    if (isResponse(device)) return device;

    if (method === "GET" && path === "/api/operator/grants/me") return json(200, { wire_version: 1, grant: this.grantView(device) });
    if (method === "POST" && path === "/api/operator/grants/renew") {
      device.expiresAt = this.now() + 90 * DAY_MS;
      return json(200, { wire_version: 1, grant: this.grantView(device) });
    }
    if (method === "DELETE" && path === "/api/operator/devices/me") {
      await this.revokeDevice(device.id);
      return json(200, { wire_version: 1, revoked_at: device.revokedAt, active_epoch: this.activeEpoch.toString(), upload_state: "open" });
    }
    if (method === "GET" && path === "/api/operator/principals") return this.principals(device);
    if (method === "GET" && path === "/api/operator/machine-presence") {
      if (!device.capabilities.includes("feed:read")) return error(403, "capability_required");
      return json(200, {
        wire_version: 1, observer_status: this.observerStatus,
        observer_checked_at: this.observerStatus === "ok" ? iso(this.now()) : null,
        machines: [...this.machines.values()].filter((machine) => machine.status === "active").map((machine) => ({
          machine_id: machine.id, hub_id: machine.hubId, monitoring: "not_capable", monitoring_generation: "0",
          presence: null, last_report_at: null, lease_expires_at: null, deadline_at: null, silence: null, components: null, open_outage: null,
          ...this.presence.get(machine.id),
        })),
      });
    }
    match = /^\/api\/operator\/machines\/([^/]+)\/monitoring$/.exec(path);
    if (method === "PUT" && match) {
      if (!device.capabilities.includes("account:manage")) return error(403, "insufficient_authority");
      const machine = this.machines.get(match[1]);
      if (!machine) return error(404, "machine_not_found");
      const body = JSON.parse(decoder.decode(request.body));
      const row = this.presence.get(machine.id);
      const key = `${machine.id}:${body.decision_id}`;
      const prior = this.monitoringDecisions.get(key);
      const digest = decoder.decode(request.body);
      if (prior) return prior.body === digest ? json(200, prior.response) : error(409, "monitoring_decision_conflict");
      const generation = String(row?.monitoring_generation ?? "0");
      if (body.expected_monitoring_generation !== generation) return error(409, "monitoring_generation_conflict", { current_monitoring_generation: generation });
      if (body.enabled && machine.status !== "active") return error(422, "machine_unavailable");
      if (!row || row.monitoring === "not_capable") return error(422, "presence_not_capable");
      const state = body.enabled ? "enabled" : "disabled";
      const changed = row.monitoring !== state;
      const next = changed ? (BigInt(generation) + 1n).toString() : generation;
      this.presence.set(machine.id, { ...row, monitoring: state, monitoring_generation: next,
        presence: body.enabled ? "pending_first_report" : null, last_report_at: null, lease_expires_at: null,
        deadline_at: body.enabled ? iso(this.now() + 240_000) : null, components: null, silence: null, open_outage: null,
      });
      const response = { wire_version: 1, machine_id: machine.id, hub_id: machine.hubId, changed,
        monitoring: { state, monitoring_generation: next, decision_id: body.decision_id, decided_at: iso(this.now()) },
      };
      this.monitoringDecisions.set(key, { body: digest, response });
      return json(200, response);
    }
    if (method === "GET" && path === "/api/operator/keys/wraps") return this.keyWraps(device);
    if (method === "GET" && path === "/api/operator/feed") return this.replay(device, url.searchParams);
    if (method === "POST" && path === "/api/operator/feed/acks") return this.ackPersisted(request, device);
    if (path === "/api/operator/devices/me/cursor") return method === "GET" ? this.getCursor(device) : this.putCursor(request, device);
    if (method === "PUT" && path === "/api/operator/read-marks") return this.putReadMark(request, device);
    if (method === "GET" && path === "/api/operator/read-marks") return this.getReadMarks(url.searchParams);
    if (method === "POST" && path === "/api/operator/commands") return this.submitCommand(request, device);
    if (method === "POST" && path === "/api/operator/assertions") return this.enrollmentAssertion(request, device);
    match = /^\/api\/operator\/commands\/([^/]+)$/.exec(path);
    if (method === "GET" && match) return this.commandStatus(match[1]);
    match = /^\/api\/operator\/commands\/([^/]+)\/cancel$/.exec(path);
    if (method === "POST" && match) return this.cancelCommand(match[1]);
    return error(404, "not_found");
  }

  // ---------------------------------------------------------------- enrollment

  private async createEnrollment(request: DoubleRequest): Promise<DoubleResponse> {
    const body = this.parseBody(request);
    if (isResponse(body)) return body;
    const origin = request.headers.origin;
    if (!this.originAllowed(origin) || body.origin !== origin) return error(403, "origin_mismatch");
    const jwk = body.signing_jwk as JsonWebKey | undefined;
    if (!jwk || jwk.kty !== "EC" || jwk.crv !== "P-256" || typeof jwk.x !== "string" || typeof jwk.y !== "string") {
      return error(400, "invalid_request", { field: "signing_jwk" });
    }
    if (typeof body.encryption_public_key !== "string") return error(400, "invalid_request", { field: "encryption_public_key" });
    const encryptionPublic = b64urlDecode(body.encryption_public_key, "encryption_public_key");
    const label = typeof body.device_label === "string" ? body.device_label.trim() : "";
    if (label.length < 1 || label.length > 80) return error(400, "invalid_request", { field: "device_label" });
    const signingRaw = new Uint8Array([4, ...b64urlDecode(jwk.x, "x"), ...b64urlDecode(jwk.y, "y")]);
    if (signingRaw.every((value, index) => value === encryptionPublic[index]) && signingRaw.length === encryptionPublic.length) {
      return error(400, "key_reuse");
    }
    const jkt = await jwkThumbprint({ kty: "EC", crv: "P-256", x: jwk.x, y: jwk.y });
    if ([...this.devices.values()].some((device) => device.status === "active" && device.jkt === jkt)) return error(409, "key_already_enrolled");
    const id = uuid();
    const checkPlain = crypto.getRandomValues(new Uint8Array(32));
    let check;
    try {
      check = await seal(await importPublicKey(encryptionPublic), ENROLLMENT_CHECK_INFO, checkPlain, id);
    } catch {
      return error(400, "invalid_request", { field: "encryption_public_key" });
    }
    const enrollment: Enrollment = {
      id,
      userCode: userCode(),
      pollSecret: randomId(32),
      origin: origin!,
      label,
      signingJwk: { kty: "EC", crv: "P-256", x: jwk.x, y: jwk.y },
      jkt,
      encryptionPublic,
      checkPlain: b64urlEncode(checkPlain),
      status: "pending",
      expiresAt: this.now() + 15 * 60_000,
      lastPolledAt: 0,
      capabilities: [],
      scopes: [],
    };
    this.enrollments.set(id, enrollment);
    return json(201, {
      wire_version: 1,
      enrollment_id: id,
      user_code: enrollment.userCode,
      poll_secret: enrollment.pollSecret,
      approval_url: `${this.baseUrl}/operator/approve?code=${enrollment.userCode}`,
      expires_at: iso(enrollment.expiresAt),
      interval: 5,
      encryption_key_check: { enc: b64urlEncode(check.enc), ct: b64urlEncode(check.ciphertext) },
    });
  }

  /** Poll interval enforcement is relaxed to 0 s when `fastPoll` is set (tests). */
  fastPoll = true;

  private pollEnrollment(request: DoubleRequest, id: string): DoubleResponse {
    const body = this.parseBody(request);
    if (isResponse(body)) return body;
    const enrollment = this.enrollments.get(id);
    if (!enrollment) return error(404, "enrollment_not_found");
    if (body.poll_secret !== enrollment.pollSecret) return error(403, "enrollment_mismatch");
    if (enrollment.expiresAt <= this.now()) return error(410, "enrollment_expired");
    if (!this.fastPoll && this.now() - enrollment.lastPolledAt < 5_000) return error(429, "slow_down", {}, { "Retry-After": "5" });
    enrollment.lastPolledAt = this.now();
    if (enrollment.status === "pending") return json(200, { wire_version: 1, status: "authorization_pending", interval: 5 });
    if (enrollment.status === "denied") return json(200, { wire_version: 1, status: "denied", interval: 5 });
    return json(200, {
      wire_version: 1,
      status: "approved",
      interval: 5,
      approved_capabilities: enrollment.capabilities,
      approved_scopes: enrollment.scopes,
      account_email_hint: "o•••@example.com",
    });
  }

  private async completeEnrollment(request: DoubleRequest, id: string): Promise<DoubleResponse> {
    const enrollment = this.enrollments.get(id);
    const body = this.parseBody(request);
    if (isResponse(body)) return body;
    if (!enrollment) return error(404, "enrollment_not_found");
    if (enrollment.expiresAt <= this.now()) return error(410, "enrollment_expired");
    const proof = request.headers["psc-pop-proof"];
    if (!proof || request.headers.authorization) return error(401, "pop_required");
    const claims = await this.verifyProof(proof, enrollment.signingJwk, enrollment.jkt, request);
    if (isResponse(claims)) return claims;
    if (claims.enr !== id) return error(401, "pop_binding_mismatch");
    if (enrollment.status === "completed") return error(409, "enrollment_completed");
    if (enrollment.status !== "approved") return error(409, "enrollment_not_approved");
    if (body.poll_secret !== enrollment.pollSecret) return error(403, "enrollment_mismatch");
    if (body.encryption_key_check !== enrollment.checkPlain) return error(403, "encryption_key_check_failed");
    enrollment.status = "completed";
    const device: Device = {
      id: uuid(),
      label: enrollment.label,
      origin: enrollment.origin,
      signingJwk: enrollment.signingJwk,
      jkt: enrollment.jkt,
      encryptionPublic: enrollment.encryptionPublic,
      capabilities: enrollment.capabilities,
      scopes: enrollment.scopes,
      generation: 1n,
      status: "active",
      expiresAt: this.now() + 90 * DAY_MS,
      createdAt: iso(this.now()),
      revokedAt: null,
    };
    this.devices.set(device.id, device);
    return json(201, {
      wire_version: 1,
      device_id: device.id,
      grant: this.grantView(device),
      account_id: this.accountId,
      feed_generation: this.feedGeneration.toString(),
      active_epoch: this.activeEpoch.toString(),
      issuer_jwks_url: `${this.baseUrl}/api/operator/jwks`,
    });
  }

  private grantView(device: Device) {
    return {
      grant_id: device.id,
      kind: "device",
      account_id: this.accountId,
      origin: device.origin,
      capabilities: device.capabilities,
      scopes: device.scopes,
      grant_generation: device.generation.toString(),
      status: device.status,
      expires_at: iso(device.expiresAt),
    };
  }

  private async principals(device: Device): Promise<DoubleResponse> {
    const machines = await Promise.all(
      [...this.machines.values()].map(async (machine) => ({
        machine_id: machine.id,
        hub_id: machine.hubId,
        label: machine.label,
        projects: machine.projects,
        status: machine.status,
        last_seen_at: null,
        machine_binding: machine.status === "active" ? await this.machineBinding(machine) : null,
        capabilities: [],
        monitoring_enabled: false,
      })),
    );
    const own = this.grantView(device);
    return json(200, { wire_version: 1, devices: [{ device_id: device.id, label: device.label, ...own }], machines });
  }

  // ---------------------------------------------------------------- keys

  private async keyWraps(device: Device): Promise<DoubleResponse> {
    const devicePublic = await importPublicKey(device.encryptionPublic);
    const epochs = [];
    for (const epoch of [...this.epochs.values()].sort((a, b) => (a.epoch < b.epoch ? 1 : -1))) {
      if (epoch.status === "destroyed" || epoch.feedGeneration !== this.feedGeneration) continue;
      const aad = epochWrapAad({
        accountId: this.accountId,
        feedGeneration: this.feedGeneration.toString(),
        epoch: epoch.epoch.toString(),
        deviceId: device.id,
      });
      const wrap = await seal(devicePublic, EPOCH_WRAP_INFO, epoch.secret, aad);
      epochs.push({
        epoch: epoch.epoch.toString(),
        manifest: await this.manifest(epoch),
        wrapped_private_key: { enc: b64urlEncode(wrap.enc), ct: b64urlEncode(wrap.ciphertext) },
      });
    }
    return json(200, {
      wire_version: 1,
      feed_generation: this.feedGeneration.toString(),
      active_epoch: this.activeEpoch.toString(),
      policy_version: this.policyVersion.toString(),
      epochs,
      unavailable_epochs: [],
    });
  }

  // ---------------------------------------------------------------- replay (§8.1)

  /** Page size cap, settable by tests to force multi-page replay. */
  pageLimit = 500;

  private replay(_device: Device, params: URLSearchParams): DoubleResponse {
    const generation = params.get("generation") ?? "";
    const after = params.get("after") ?? "";
    if (!DECIMAL.test(generation) || !DECIMAL.test(after)) return error(400, "invalid_request", { field: "after" });
    if (BigInt(generation) !== this.feedGeneration) {
      return error(409, "feed_generation_changed", { feed_generation: this.feedGeneration.toString(), start_sequence: this.startSequence.toString() });
    }
    const afterSeq = BigInt(after);
    const limit = Math.min(Math.max(Number(params.get("limit") ?? "200") || 200, 1), 500, this.pageLimit);
    const floor = this.retainedFloor();
    if (afterSeq > this.head) return error(409, "cursor_ahead", { head: this.head.toString() });
    if (afterSeq < floor - 1n) {
      return error(410, "history_expired", {
        feed_generation: this.feedGeneration.toString(),
        retained_floor: floor.toString(),
        head: this.head.toString(),
        expired_through: (floor - 1n).toString(),
      });
    }
    const live = new Map(this.events.filter((event) => event.expiresAt > this.now()).map((event) => [event.sequence, event]));
    const events = [];
    const intervals: { from: string; to: string; reason: string }[] = [];
    let cursor = afterSeq;
    let gapFrom: bigint | null = null;
    while (cursor < this.head && events.length < limit) {
      const next = cursor + 1n;
      const event = live.get(next);
      if (event) {
        if (gapFrom !== null) {
          intervals.push({ from: gapFrom.toString(), to: cursor.toString(), reason: "retention" });
          gapFrom = null;
        }
        events.push(this.eventView(event));
      } else if (gapFrom === null) {
        gapFrom = next;
      }
      cursor = next;
    }
    if (gapFrom !== null) intervals.push({ from: gapFrom.toString(), to: cursor.toString(), reason: "retention" });
    return json(200, {
      wire_version: 1,
      feed_generation: this.feedGeneration.toString(),
      retained_floor: floor.toString(),
      head: this.head.toString(),
      events,
      expired_intervals: intervals,
      next_cursor: cursor.toString(),
      has_more: cursor < this.head,
      poll_after_ms: cursor < this.head ? 0 : 5000,
    });
  }

  private eventView(event: StoredEvent) {
    return {
      sequence: event.sequence.toString(),
      event_id: event.eventId,
      event_scope: event.scope,
      producer_kind: event.producerKind,
      hub_id: event.hubId,
      project_id: event.projectId,
      session_id: event.sessionId,
      key_epoch: event.keyEpoch.toString(),
      ciphertext: b64urlEncode(event.ciphertext),
      digest: event.digest,
      stored_at: iso(event.storedAt),
      expires_at: iso(event.expiresAt),
      attachment_ids: [],
      observer_assertion: event.observerAssertion,
    };
  }

  private ackPersisted(request: DoubleRequest, device: Device): DoubleResponse {
    const body = this.parseBody(request);
    if (isResponse(body)) return body;
    if (body.feed_generation !== this.feedGeneration.toString()) {
      return error(409, "feed_generation_changed", { feed_generation: this.feedGeneration.toString(), start_sequence: this.startSequence.toString() });
    }
    const acks = Array.isArray(body.acks) ? body.acks : [];
    if (acks.length < 1 || acks.length > 500) return error(400, "invalid_request", { field: "acks" });
    const mine = this.acks.get(device.id) ?? new Map<string, string>();
    this.acks.set(device.id, mine);
    const rows = acks.map((entry) => {
      const ack = entry as { event_id?: string; digest?: string };
      const event = this.events.find((candidate) => candidate.eventId === ack.event_id);
      if (!event) {
        if (ack.event_id && this.tombstones.has(ack.event_id)) return { event_id: ack.event_id, outcome: "rejected", error: "event_expired" };
        return { event_id: ack.event_id, outcome: "rejected", error: "event_unknown" };
      }
      if (event.digest !== ack.digest) return { event_id: ack.event_id, outcome: "rejected", error: "digest_mismatch" };
      if (mine.has(event.eventId)) return { event_id: ack.event_id, outcome: "already_acked" };
      mine.set(event.eventId, event.digest);
      return { event_id: ack.event_id, outcome: "acked" };
    });
    return json(200, { wire_version: 1, rows });
  }

  private cursorKey(device: Device) {
    return `${device.id}|${this.feedGeneration}`;
  }

  private getCursor(device: Device): DoubleResponse {
    const stored = this.cursors.get(this.cursorKey(device));
    return json(200, {
      wire_version: 1,
      feed_generation: this.feedGeneration.toString(),
      cursor: stored?.cursor?.toString() ?? null,
      accepted_expired_through: stored?.acceptedExpiredThrough?.toString() ?? null,
      updated_at: stored?.updatedAt ?? null,
    });
  }

  private putCursor(request: DoubleRequest, device: Device): DoubleResponse {
    const body = this.parseBody(request);
    if (isResponse(body)) return body;
    if (body.feed_generation !== this.feedGeneration.toString()) {
      return error(409, "feed_generation_changed", { feed_generation: this.feedGeneration.toString(), start_sequence: this.startSequence.toString() });
    }
    if (typeof body.cursor !== "string" || !DECIMAL.test(body.cursor)) return error(400, "invalid_request", { field: "cursor" });
    const cursor = BigInt(body.cursor);
    const stored = this.cursors.get(this.cursorKey(device));
    if (stored?.cursor !== null && stored?.cursor !== undefined && cursor < stored.cursor) {
      return error(409, "cursor_regression", { cursor: stored.cursor.toString() });
    }
    if (cursor > this.head) return error(409, "cursor_ahead", { head: this.head.toString() });
    let accepted = stored?.acceptedExpiredThrough ?? null;
    if (body.accepted_expired_through !== undefined) {
      if (typeof body.accepted_expired_through !== "string" || !DECIMAL.test(body.accepted_expired_through)) {
        return error(400, "invalid_request", { field: "accepted_expired_through" });
      }
      const through = BigInt(body.accepted_expired_through);
      if (through > this.retainedFloor() - 1n) return error(400, "invalid_request", { field: "accepted_expired_through" });
      accepted = through;
    }
    const updatedAt = iso(this.now());
    this.cursors.set(this.cursorKey(device), { cursor, acceptedExpiredThrough: accepted, updatedAt });
    return json(200, {
      wire_version: 1,
      feed_generation: this.feedGeneration.toString(),
      cursor: cursor.toString(),
      accepted_expired_through: accepted?.toString() ?? null,
      updated_at: updatedAt,
    });
  }

  private putReadMark(request: DoubleRequest, device: Device): DoubleResponse {
    const body = this.parseBody(request);
    if (isResponse(body)) return body;
    const { hub_id, project_id, session_id, sequence } = body as Record<string, string>;
    if (![hub_id, project_id, session_id].every((value) => typeof value === "string" && /^[A-Za-z0-9._:@/-]{1,200}$/.test(value))) {
      return error(400, "invalid_request", { field: "session_id" });
    }
    if (typeof sequence !== "string" || !DECIMAL.test(sequence)) return error(400, "invalid_request", { field: "sequence" });
    const key = `${hub_id}|${project_id}|${session_id}`;
    const stored = this.readMarks.get(key);
    const highestLive = this.events
      .filter((event) => event.expiresAt > this.now() && event.hubId === hub_id && event.projectId === project_id && event.sessionId === session_id)
      .reduce((high, event) => (event.sequence > high ? event.sequence : high), 0n);
    const bound = [highestLive, stored?.sequence ?? 0n].reduce((a, b) => (a > b ? a : b), 0n);
    const requested = BigInt(sequence);
    if (requested > bound) return error(422, "read_mark_out_of_range", { max_sequence: bound.toString() });
    if (stored && requested <= stored.sequence) {
      return json(200, { wire_version: 1, hub_id, project_id, session_id, sequence: stored.sequence.toString(), updated_at: stored.updatedAt });
    }
    const updatedAt = iso(this.now());
    this.readMarks.set(key, { hub_id, project_id, session_id, sequence: requested, updatedAt, by: device.id });
    return json(200, { wire_version: 1, hub_id, project_id, session_id, sequence: requested.toString(), updated_at: updatedAt });
  }

  private getReadMarks(params: URLSearchParams): DoubleResponse {
    const updatedAfter = params.get("updated_after");
    const marks = [...this.readMarks.values()]
      .filter((mark) => !updatedAfter || mark.updatedAt > updatedAfter)
      .sort((a, b) => (a.updatedAt < b.updatedAt ? -1 : 1))
      .map((mark) => ({
        hub_id: mark.hub_id,
        project_id: mark.project_id,
        session_id: mark.session_id,
        sequence: mark.sequence.toString(),
        updated_at: mark.updatedAt,
        updated_by_device_id: mark.by,
      }));
    return json(200, { wire_version: 1, read_marks: marks, next_cursor: null });
  }

  // ---------------------------------------------------------------- hub assertion (§5.5)

  private async enrollmentAssertion(request: DoubleRequest, device: Device): Promise<DoubleResponse> {
    const body = this.parseBody(request);
    if (isResponse(body)) return body;
    const { hub_id, hub_challenge, installation_jkt } = body as Record<string, string>;
    if (typeof hub_id !== "string" || typeof hub_challenge !== "string" || typeof installation_jkt !== "string") {
      return error(400, "invalid_request", { field: "hub_challenge" });
    }
    const machine = [...this.machines.values()].find((entry) => entry.hubId === hub_id && entry.status === "active");
    if (!machine) return error(422, "hub_not_enrolled");
    const iat = Math.floor(this.now() / 1000);
    const { kid } = await this.issuerKey();
    const assertion = await this.sign("psc-op-enrollment+jwt", {
      iss: this.baseUrl,
      aud: `cas-hub:${hub_id}`,
      sub: device.id,
      acct: this.accountId,
      hub: hub_id,
      projects: machine.projects,
      cmd_scopes: device.scopes.filter((scope) => scope.hub_id === hub_id),
      origin: device.origin,
      ins_jkt: installation_jkt,
      dev_jkt: device.jkt,
      gen: device.generation.toString(),
      cak: kid,
      fgen: this.feedGeneration.toString(),
      epoch: this.activeEpoch.toString(),
      chl: hub_challenge,
      iat,
      exp: iat + 300,
      jti: randomId(),
    });
    return json(200, { assertion, expires_at: iso((iat + 300) * 1000) });
  }

  // ---------------------------------------------------------------- commands (§10)

  private async submitCommand(request: DoubleRequest, device: Device): Promise<DoubleResponse> {
    const body = this.parseBody(request);
    if (isResponse(body)) return body;
    const command = body as Record<string, unknown>;
    const history = command.history_event as Record<string, unknown> | undefined;
    if (typeof command.command_id !== "string" || !history || typeof history.event_id !== "string") {
      return error(400, "invalid_request", { field: "command_id" });
    }
    const existing = this.commands.get(command.command_id);
    if (existing) {
      if (existing.machineDigest !== command.machine_digest || existing.historyDigest !== history.digest) return error(409, "command_conflict");
      return json(200, this.commandView(existing));
    }
    const machine = this.machines.get(String(command.machine_id));
    if (!machine || machine.status !== "active" || machine.hubId !== command.hub_id) return error(422, "machine_unavailable");
    if (command.machine_key_id !== machine.commandKeyId) return error(409, "machine_key_stale");
    if (command.operation !== "operator_message") return error(422, "operation_not_allowed");
    const covered = device.scopes.some(
      (scope) =>
        scope.hub_id === command.hub_id &&
        scope.project_id === command.project_id &&
        (scope.session_id === null || scope.session_id === command.session_id) &&
        scope.operations.includes(String(command.operation)),
    );
    if (!covered) return error(403, "scope_not_granted");
    const keyEpoch = BigInt(String(history.key_epoch));
    if (keyEpoch !== this.activeEpoch) {
      return error(409, "epoch_retired", { active_epoch: this.activeEpoch.toString(), policy_version: this.policyVersion.toString() });
    }
    const ciphertext = b64urlDecode(String(history.ciphertext), "ciphertext");
    if ((await digest(ciphertext)) !== history.digest) return error(400, "invalid_request", { field: "history_event.digest" });
    if ((await digest(b64urlDecode(String(command.machine_ciphertext), "machine_ciphertext"))) !== command.machine_digest) {
      return error(400, "invalid_request", { field: "machine_digest" });
    }
    const stored = this.storeEvent({
      eventId: history.event_id,
      scope: "session",
      producerKind: "principal",
      hubId: String(command.hub_id),
      projectId: String(command.project_id),
      sessionId: String(command.session_id),
      keyEpoch,
      ciphertext,
      digest: String(history.digest),
      observerAssertion: null,
    });
    const record: Command = {
      commandId: command.command_id,
      machineId: machine.id,
      deviceId: device.id,
      hubId: String(command.hub_id),
      projectId: String(command.project_id),
      sessionId: String(command.session_id),
      operation: String(command.operation),
      machineDigest: String(command.machine_digest),
      machineCiphertext: String(command.machine_ciphertext),
      historyEventId: history.event_id,
      historyDigest: String(history.digest),
      historySequence: stored.sequence,
      status: "pending_machine",
      createdAt: this.now(),
      expiresAt: this.now() + DAY_MS,
    };
    this.commands.set(record.commandId, record);
    return json(201, this.commandView(record));
  }

  private commandView(command: Command) {
    const status = command.status === "pending_machine" && command.expiresAt <= this.now() ? "expired" : command.status;
    return {
      wire_version: 1,
      command_id: command.commandId,
      status,
      reason: null,
      created_at: iso(command.createdAt),
      expires_at: iso(command.expiresAt),
      reserved_at: null,
      terminal_at: null,
      receipt: null,
      history_receipt: { event_id: command.historyEventId, sequence: command.historySequence.toString(), digest: command.historyDigest },
    };
  }

  private commandStatus(commandId: string): DoubleResponse {
    const command = this.commands.get(commandId);
    return command ? json(200, this.commandView(command)) : error(404, "command_not_found");
  }

  private cancelCommand(commandId: string): DoubleResponse {
    const command = this.commands.get(commandId);
    if (!command) return error(404, "command_not_found");
    if (command.status === "pending_machine" && command.expiresAt > this.now()) command.status = "cancelled";
    if (command.status === "cancelled") return json(200, { wire_version: 1, status: "cancelled" });
    if (command.status === "reserved") return error(409, "handoff_in_progress", { status: "reserved", message: "Machine handoff in progress; cancellation not confirmed" });
    if (command.status === "expired" || command.expiresAt <= this.now()) return error(410, "command_expired");
    return error(409, "command_terminal", { receipt: null });
  }

  /** Machine side of §10.3–10.4, as a test hook: reserve then accept. */
  acceptCommand(commandId: string, outcome: "accepted" | "rejected_by_machine" = "accepted") {
    const command = this.commands.get(commandId);
    if (!command) throw new Error(`unknown command ${commandId}`);
    if (command.status === "pending_machine") command.status = "reserved";
    if (command.status === "reserved") command.status = outcome;
    return command;
  }
}
