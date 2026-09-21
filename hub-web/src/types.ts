export type Scope =
  | "machine-read"
  | "session-read"
  | "pane-read"
  | "pane-input"
  | "message-send"
  | "pane-interrupt"
  | "factory-manage"
  | "hub-admin";

export interface StoredMachine {
  id: string;
  label: string;
  baseUrl: string;
  deviceId: string;
  credentialId: string;
  credential: string;
  expiresAt: string;
  scopes: Scope[];
  publicKey: JsonWebKey;
  privateKey: CryptoKey;
}

export interface PairingInstallIdentity {
  machineId: string;
  credentialId: string;
  generation: number;
}

export interface HubSession {
  name: string;
  project_dir?: string;
  supervisor: string;
  workers: string[];
  epic_id?: string;
  ws_port?: number;
  liveness: "live" | "stale_metadata" | "missing_endpoint";
  /** Metadata survived after its registered supervisor stopped being live. */
  dormant?: boolean;
  /** Browser-only retention of a destination with an in-flight message. */
  unreachable?: boolean;
}

export interface PaneInfo {
  id: string;
  kind: "Worker" | "Supervisor" | "Director" | "Shell";
  focused: boolean;
  title: string;
  exited: boolean;
}

export interface SessionState {
  focused_pane?: string;
  panes: PaneInfo[];
  epic_id?: string;
  epic_title?: string;
  cols: number;
  rows: number;
}

export type OperatorTurnKind = "answer" | "status" | "receipt" | "ask" | "blocker";

export interface ArtifactRef {
  artifact_id: string;
  name: string;
  mime: string;
  size_bytes: number;
  sha256: string;
}

/** Supervisor turn routed to this paired Commander device. */
export interface OperatorReply {
  notification_id: number;
  reply_to: number | null;
  message: string;
  summary: string;
  device_id: string;
  operator_label?: string;
  kind?: OperatorTurnKind;
  attachments?: ArtifactRef[];
  /** Quick-reply choices for an `ask`. Not yet in OperatorReplyPayload
   * (protocol.rs); consumed when a payload carries it, else the defaults. */
  options?: string[];
}

/** Durable operator message projected by the daemon's history page. */
export interface ConversationHistoryMessage {
  notification_id: number;
  target: string;
  text: string;
  state: "sending" | "acknowledged";
  stamped: boolean;
  reply_to?: number;
  device_id: string;
  operator_label?: string;
  at: string;
}

/** Durable supervisor reply with its queue timestamp for ordered hydration. */
export interface ConversationHistoryReply extends OperatorReply {
  at: string;
}

export interface ConversationHistoryPage {
  request_id: string;
  messages: ConversationHistoryMessage[];
  replies: ConversationHistoryReply[];
  has_earlier: boolean;
  next_before?: number;
}

/** Durable acknowledgment for a Commander SendMessage submission. */
export interface MessageQueued {
  client_ref: string | null;
  notification_id: number;
  target: string;
  stamped: boolean;
}

export type SessionPhase = "planning" | "editing" | "testing" | "building" | "blocked" | "reviewing" | "idle";

export interface SessionCardSummary {
  title: string;
  description: string;
  phase: SessionPhase;
  blocked_on?: string;
  generated_at: string;
}

export interface LeaseState {
  controller_device_id?: string;
  controller_label?: string;
  expires_at?: string;
  held_by_me: boolean;
  local_preempted?: boolean;
}

export interface AttentionItem {
  id: string;
  machineId: string;
  machineLabel: string;
  session?: string;
  kind: string;
  message: string;
  headline?: string;
  detail?: string;
  cause?: string;
  severity?: "critical" | "warning" | "info" | "incident" | "notice";
  action?: "repair" | "view_pane" | "retry" | "open_pr" | "none";
  ticketId?: string;
  payload?: unknown;
  fingerprint?: string;
  enrichmentPending?: boolean;
  enrichedAt?: string;
  createdAt: string;
  /** Set when repeats collapse into this entry: when it was first seen. */
  firstSeenAt?: string;
  /** Occurrences collapsed into this entry; absent means one. */
  repeatCount?: number;
  seenAt?: string;
  acknowledgedAt?: string;
}
