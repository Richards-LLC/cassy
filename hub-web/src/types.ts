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

export interface HubSession {
  name: string;
  project_dir?: string;
  supervisor: string;
  workers: string[];
  epic_id?: string;
  ws_port?: number;
  liveness: "live" | "stale_metadata" | "missing_endpoint";
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
  createdAt: string;
  seenAt?: string;
  acknowledgedAt?: string;
}
