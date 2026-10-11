export type Scope =
  | "machine-read"
  | "session-read"
  | "pane-read"
  | "pane-input"
  | "message-send"
  | "pane-interrupt"
  | "session-launch"
  | "factory-operate"
  | "factory-manage"
  | "hub-admin";

export interface StoredMachine {
  credentialGeneration?: number;
  accountEnrollment?: import("./installation-access").AccountEnrollment;
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
  /** When the session last did anything: its newest queue row (cas-55a4). */
  last_activity_at?: string;
  /** Who that row was between, e.g. "supervisor → worker-1"; never its content. */
  last_activity?: string;
  /** When the session started; ranks Most recent when no session of a project has activity (cas-6acf). */
  started_at?: string;
  /** The project's Cassy Cloud identity (cas-eaa3), so the app switcher opens Explorer on it. */
  cloud_project_id?: string;
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
  /** Local application receipt, never shared operator-read state. */
  device_persisted?: boolean;
  /** cas-97d58 F05: storing this reply on this device failed (not just pending). */
  device_store_failed?: boolean;
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
  /**
   * Set when the row is a daemon notice about the session's plumbing, not a
   * supervisor turn (cas-e829): it belongs in attention, never the thread.
   */
  notice?: OperatorNotice | null;
  /**
   * The session of the turn `reply_to` answers, when it is another session's
   * (cas-e829). The reply belongs to this thread and only quotes that one.
   */
  reply_to_session?: string | null;
}

/** protocol.rs OperatorNotice (cas-e829). */
export interface OperatorNotice {
  /** The queue source that raised it, e.g. "relay-watchdog". */
  source: string;
  /** The queue row it is about (the unseen relay), when it is about one. */
  subject?: number;
  /** The problem it reports is over. */
  resolved?: boolean;
}

/** protocol.rs DaemonMessage::OperatorNoticeResolved (cas-e829). */
export interface OperatorNoticeResolved {
  notification_id: number;
  subject?: number;
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
  session?: string;
  at: string;
}

/** Durable supervisor reply with its queue timestamp for ordered hydration. */
export interface ConversationHistoryReply extends OperatorReply {
  session?: string;
  at: string;
}

export interface ConversationHistoryPage {
  request_id: string;
  messages: ConversationHistoryMessage[];
  replies: ConversationHistoryReply[];
  has_earlier: boolean;
  next_before?: number;
  /** Other sessions' recent turns, with the newest page only (cas-55a4). Not this session's thread. */
  earlier_messages?: ConversationHistoryMessage[];
  earlier_replies?: ConversationHistoryReply[];
}

/** Durable acknowledgment for a Commander SendMessage submission. */
export interface MessageQueued {
  client_ref: string | null;
  notification_id: number;
  target: string;
  stamped: boolean;
  device_label?: string;
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
