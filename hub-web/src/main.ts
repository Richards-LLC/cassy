import "./styles.css";
import { HubConnectionSupervisor, type ConnectionState, type HubMachineInfo } from "./connection";
import { createDeviceKey } from "./dpop";
import { consumePairingFragment } from "./fragment";
import { attentionStore, catalog } from "./storage";
import { createTerminalSurface, type TerminalSurface } from "./terminal";
import type { AttentionItem, HubSession, LeaseState, PaneInfo, Scope, SessionState, StoredMachine } from "./types";

const pendingPairing = consumePairingFragment(window.location, window.history);
const app = document.querySelector<HTMLDivElement>("#app")!;
const machines = new Map<string, StoredMachine>();
const sessions = new Map<string, HubSession[]>();
const connections = new Map<string, HubConnectionSupervisor>();
const connectionStates = new Map<string, ConnectionState>();
const machineInfo = new Map<string, HubMachineInfo | undefined>();
const statuses = new Map<string, Record<string, unknown>>();
const leases = new Map<string, LeaseState>();
const surfaces = new Map<string, TerminalSurface>();
const sessionStates = new Map<string, SessionState>();
const paneBuffers = new Map<string, number[]>();
const selectedPanes = new Map<string, string>();
const leaseHeartbeats = new Map<string, number>();
const leaseExpiryTimers = new Map<string, number>();
let attention: AttentionItem[] = [];
let selectedMachineId: string | undefined;
let selectedSession: string | undefined;

function sessionKey(machineId: string, session: string): string { return `${machineId}:${session}`; }
function paneKey(machineId: string, session: string, pane: string): string { return `${machineId}:${session}:${pane}`; }
function activeConnection(): HubConnectionSupervisor | undefined { return selectedMachineId ? connections.get(selectedMachineId) : undefined; }

async function boot(): Promise<void> {
  for (const machine of await catalog.list()) machines.set(machine.id, machine);
  attention = (await attentionStore.list()).toSorted((a, b) => b.createdAt.localeCompare(a.createdAt));
  selectedMachineId = machines.keys().next().value;
  render();
  for (const machine of machines.values()) ensureConnection(machine);
}

function ensureConnection(machine: StoredMachine): HubConnectionSupervisor {
  const existing = connections.get(machine.id);
  if (existing) return existing;
  const connection = new HubConnectionSupervisor(machine, {
    onState: (state, detail) => {
      connectionStates.set(machine.id, state);
      if (state === "reconnecting" || state === "auth-blocked" || state === "offline") invalidateMachineLeases(machine.id);
      if (state === "auth-blocked") void addAttention(machine, undefined, "auth_loss", detail ?? "Authentication blocked");
      if (state === "reconnecting") void addAttention(machine, undefined, "hub_disconnected", detail ?? "Hub disconnected");
      render();
    },
    onMachineInfo: (info) => { machineInfo.set(machine.id, info); render(); },
    onSessions: (items) => { sessions.set(machine.id, items); render(); },
    onMachineEvent: (event) => {
      const kind = String(event.kind ?? "hub_event");
      if (["daemon_disconnected", "pane_exited", "session_removed"].includes(kind)) {
        void addAttention(machine, event.session as string | undefined, kind, event.diagnostic ? `Daemon ended: ${JSON.stringify(event.diagnostic)}` : kind.replaceAll("_", " "));
      }
      if (selectedMachineId === machine.id && selectedSession) void loadStatus(machine.id, selectedSession);
      if (selectedMachineId === machine.id && selectedSession) void loadLease(machine.id, selectedSession);
    },
    onSessionState: (session, state, scrollback) => void renderSessionState(machine.id, session, state, scrollback),
    onOutput: (session, pane, data) => {
      const key = paneKey(machine.id, session, pane);
      const buffered = [...(paneBuffers.get(key) ?? []), ...data];
      paneBuffers.set(key, buffered.slice(-2_000_000));
      surfaces.get(key)?.write(data);
    },
    onSocketError: (session, detail) => void addAttention(machine, session, "session_transport", detail),
  });
  connections.set(machine.id, connection);
  connection.start();
  return connection;
}

async function addAttention(machine: StoredMachine, session: string | undefined, kind: string, message: string): Promise<void> {
  const id = `${machine.id}:${session ?? "machine"}:${kind}:${message}`;
  if (attention.some((item) => item.id === id && !item.acknowledgedAt)) return;
  const item: AttentionItem = { id, machineId: machine.id, machineLabel: machine.label, session, kind, message, createdAt: new Date().toISOString() };
  attention = [item, ...attention];
  await attentionStore.put(item);
  render();
}

async function acknowledgeAttention(id: string): Promise<void> {
  const item = attention.find((candidate) => candidate.id === id);
  if (!item) return;
  item.acknowledgedAt = new Date().toISOString();
  await attentionStore.put(item);
  render();
}

async function pairMachine(form: HTMLFormElement): Promise<void> {
  if (!pendingPairing) throw new Error("Open the one-time pairing link first");
  const values = new FormData(form);
  const baseUrl = new URL(String(values.get("url"))).origin;
  const { privateKey, publicKey } = await createDeviceKey();
  const scopes = values.getAll("scope") as Scope[];
  const response = await fetch(new URL("/v1/auth/pairing/exchange", baseUrl), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    credentials: "omit",
    body: JSON.stringify({
      token: pendingPairing.token,
      hub_id: pendingPairing.hubId,
      controller_origin: location.origin,
      public_key_jwk: publicKey,
      device_label: String(values.get("device")),
      operator_label: String(values.get("operator")),
      requested_scopes: scopes,
    }),
  });
  if (!response.ok) throw new Error("Pairing was refused; create a fresh invitation and verify the hub URL");
  const credential = await response.json() as { device_id: string; credential_id: string; credential: string; expires_at: string; scopes: Scope[] };
  const machine: StoredMachine = {
    id: pendingPairing.hubId,
    label: String(values.get("label")) || pendingPairing.hubId.slice(0, 8),
    baseUrl,
    deviceId: credential.device_id,
    credentialId: credential.credential_id,
    credential: credential.credential,
    expiresAt: credential.expires_at,
    scopes: credential.scopes,
    publicKey,
    privateKey,
  };
  await catalog.put(machine);
  machines.set(machine.id, machine);
  selectedMachineId = machine.id;
  ensureConnection(machine);
  render();
}

async function openSession(machineId: string, session: string): Promise<void> {
  selectedMachineId = machineId;
  selectedSession = session;
  render();
  await Promise.all([loadStatus(machineId, session), loadLease(machineId, session)]);
  await connections.get(machineId)?.attach(session);
}

async function loadStatus(machineId: string, session: string): Promise<void> {
  const connection = connections.get(machineId);
  const machine = machines.get(machineId);
  if (!connection || !machine) return;
  try {
    const status = await connection.status(session);
    statuses.set(sessionKey(machineId, session), status);
    const tasks = [...((status.tasks_in_progress as any[]) ?? []), ...((status.tasks_ready as any[]) ?? [])];
    for (const task of tasks) {
      if (["blocked", "awaiting_merge", "awaitingmerge"].includes(String(task.status))) {
        await addAttention(machine, session, String(task.status), `${task.id}: ${task.title}`);
      }
    }
    render();
  } catch { /* connection supervisor owns transport/auth reporting */ }
}

async function loadLease(machineId: string, session: string): Promise<void> {
  try {
    const state = await connections.get(machineId)?.lease(session);
    if (state) {
      const key = sessionKey(machineId, session);
      const becameController = state.held_by_me && !leases.get(key)?.held_by_me;
      leases.set(key, state);
      if (becameController) resizeControlledPanes(machineId, session);
      const expiryTimer = leaseExpiryTimers.get(key);
      if (expiryTimer !== undefined) window.clearTimeout(expiryTimer);
      if (state.expires_at) {
        const delay = Math.max(0, new Date(state.expires_at).getTime() - Date.now() + 100);
        leaseExpiryTimers.set(key, window.setTimeout(() => void loadLease(machineId, session), delay));
      }
      if (state.held_by_me) startLeaseHeartbeat(machineId, session);
    }
    render();
  } catch { /* legacy hub may not expose lease status */ }
}

function resizeControlledPanes(machineId: string, session: string): void {
  const state = sessionStates.get(sessionKey(machineId, session));
  if (!state) return;
  for (const pane of state.panes.filter((candidate) => candidate.kind !== "Director")) {
    const surface = surfaces.get(paneKey(machineId, session, pane.id));
    if (surface) sendControl(machineId, session, { ResizePane: { pane_id: pane.id, cols: surface.cols, rows: surface.rows } });
  }
}

async function renderSessionState(machineId: string, session: string, state: SessionState, scrollback?: Record<string, number[][]>): Promise<void> {
  sessionStates.set(sessionKey(machineId, session), state);
  if (scrollback) {
    for (const [pane, chunks] of Object.entries(scrollback)) {
      paneBuffers.set(paneKey(machineId, session, pane), chunks.flat().slice(-2_000_000));
    }
  }
  if (selectedMachineId !== machineId || selectedSession !== session) return;
  const grid = document.querySelector<HTMLElement>("#pane-grid");
  if (!grid) return;
  const active = new Set(state.panes.filter((pane) => pane.kind !== "Director").map((pane) => pane.id));
  const selectedKey = sessionKey(machineId, session);
  const selectedPane = selectedPanes.get(selectedKey);
  if (!selectedPane || !active.has(selectedPane)) {
    const fallback = state.panes.find((pane) => pane.kind !== "Director" && pane.focused)
      ?? state.panes.find((pane) => pane.kind !== "Director");
    if (fallback) selectedPanes.set(selectedKey, fallback.id);
  }
  for (const [key, surface] of surfaces) {
    if (key.startsWith(`${machineId}:${session}:`) && !active.has(key.split(":").at(-1)!)) { surface.dispose(); surfaces.delete(key); }
  }
  for (const pane of state.panes.filter((candidate) => candidate.kind !== "Director")) {
    const key = paneKey(machineId, session, pane.id);
    let mount = grid.querySelector<HTMLElement>(`[data-pane-id="${CSS.escape(pane.id)}"] .terminal-mount`);
    if (!mount) {
      const card = document.createElement("section");
      card.className = "pane";
      card.dataset.paneId = pane.id;
      if (selectedPanes.get(selectedKey) === pane.id) card.classList.add("selected");
      card.onclick = () => {
        selectedPanes.set(selectedKey, pane.id);
        for (const sibling of grid.querySelectorAll(".pane.selected")) sibling.classList.remove("selected");
        card.classList.add("selected");
        surfaces.get(key)?.focus();
      };
      const title = document.createElement("header");
      title.textContent = `${pane.title || pane.id} · ${pane.kind.toLowerCase()}${pane.exited ? " · exited" : ""}`;
      mount = document.createElement("div"); mount.className = "terminal-mount";
      card.append(title, mount); grid.append(card);
    }
    const existingSurface = surfaces.get(key);
    if (existingSurface && (existingSurface.element !== mount || !existingSurface.element.isConnected)) {
      existingSurface.dispose();
      surfaces.delete(key);
    }
    if (!surfaces.has(key)) {
      const surface = await createTerminalSurface(mount, {
        onData: (data) => { if (canControl(machineId, session, "pane-input")) sendControl(machineId, session, { Input: { pane_id: pane.id, data: [...data] } }); },
        onResize: (cols, rows) => { if (canControl(machineId, session, "pane-input")) sendControl(machineId, session, { ResizePane: { pane_id: pane.id, cols, rows } }); },
      });
      const currentMount = document.querySelector<HTMLElement>(`[data-pane-id="${CSS.escape(pane.id)}"] .terminal-mount`);
      if (selectedMachineId !== machineId || selectedSession !== session || !mount.isConnected || currentMount !== mount) {
        surface.dispose();
        continue;
      }
      surfaces.set(key, surface);
      const buffered = paneBuffers.get(key);
      if (buffered) surface.write(new Uint8Array(buffered));
    }
  }
}

function hubSupports(machineId: string, capability: string): boolean {
  return machineInfo.get(machineId)?.capabilities.includes(capability) === true;
}

function canControl(machineId: string, session: string, scope: Scope): boolean {
  return hubSupports(machineId, "daemon_attach") && machines.get(machineId)?.scopes.includes(scope) === true && leases.get(sessionKey(machineId, session))?.held_by_me === true;
}

function sendControl(machineId: string, session: string, message: unknown): void {
  if (!connections.get(machineId)?.send(session, message)) toast("Terminal is reconnecting");
}

function startLeaseHeartbeat(machineId: string, session: string): void {
  const key = sessionKey(machineId, session);
  if (leaseHeartbeats.has(key)) return;
  leaseHeartbeats.set(key, window.setInterval(async () => {
    if (!leases.get(key)?.held_by_me) return;
    try {
      const state = await connections.get(machineId)?.acquireLease(session);
      if (state) leases.set(key, state);
    } catch {
      invalidateMachineLeases(machineId);
      render();
    }
  }, 10_000));
}

function invalidateMachineLeases(machineId: string): void {
  for (const [key, timer] of leaseHeartbeats) {
    if (key.startsWith(`${machineId}:`)) { window.clearInterval(timer); leaseHeartbeats.delete(key); }
  }
  for (const [key, lease] of leases) if (key.startsWith(`${machineId}:`)) leases.set(key, { ...lease, held_by_me: false });
}

function toast(message: string): void {
  const output = document.querySelector<HTMLElement>("#toast");
  if (!output) return;
  output.textContent = message;
  output.classList.add("visible");
  window.setTimeout(() => output.classList.remove("visible"), 3200);
}

function render(): void {
  const selected = selectedMachineId ? machines.get(selectedMachineId) : undefined;
  const machineSessions = selected ? sessions.get(selected.id) ?? [] : [];
  const lease = selected && selectedSession ? leases.get(sessionKey(selected.id, selectedSession)) : undefined;
  const status = selected && selectedSession ? statuses.get(sessionKey(selected.id, selectedSession)) : undefined;
  const compatibility = selected ? compatibilityWarning(selected.id) : undefined;
  const terminalSessionKey = selected && selectedSession ? sessionKey(selected.id, selectedSession) : undefined;
  const currentGrid = document.querySelector<HTMLElement>("#pane-grid");
  const preservedGrid = terminalSessionKey && currentGrid?.dataset.sessionKey === terminalSessionKey ? currentGrid : undefined;
  if (preservedGrid) {
    preservedGrid.remove();
  } else {
    for (const surface of surfaces.values()) surface.dispose();
    surfaces.clear();
  }
  app.innerHTML = `
    <div class="shell">
      <aside class="machines"><div class="brand"><span class="pulse"></span><strong>Commander</strong></div><button id="pair-toggle" class="primary">Pair machine</button><nav id="machine-list"></nav></aside>
      <aside class="sessions"><h2>${escapeHtml(selected?.label ?? "Machines")}</h2><div class="connection ${connectionStates.get(selected?.id ?? "") ?? "idle"}">${connectionStates.get(selected?.id ?? "") ?? "select a machine"}</div>${compatibility ? `<div class="compatibility-warning" role="alert">${escapeHtml(compatibility)}</div>` : ""}${selected ? '<button id="remove-machine" class="remove-machine">Remove</button>' : ""}<nav id="session-list"></nav></aside>
      <main><header class="toolbar"><div><h1>${escapeHtml(selectedSession ?? "Fleet overview")}</h1><p>${lease?.held_by_me ? "You control this session" : lease?.controller_label ? `Observed · controlled by ${escapeHtml(lease.controller_label)}` : "Observer mode"}</p></div><div class="actions"><button id="lease" ${!selected || !selectedSession || !hubSupports(selected.id, "daemon_attach") || (!lease?.held_by_me && !selected.scopes.includes("pane-input") && !selected.scopes.includes("hub-admin")) ? "disabled" : ""}>${lease?.held_by_me ? "Release control" : lease?.controller_label && selected?.scopes.includes("hub-admin") ? "Force takeover" : "Take control"}</button><button id="interrupt" class="danger" ${!selected || !selectedSession || !canControl(selected.id, selectedSession, "pane-interrupt") ? "disabled" : ""}>Interrupt</button></div></header><section id="pane-grid" class="pane-grid"${terminalSessionKey ? ` data-session-key="${escapeAttr(terminalSessionKey)}"` : ""}><div class="empty">${selectedSession ? "Connecting to terminal…" : "Choose a live session to open its panes."}</div></section></main>
      <aside class="context"><section><h2>Attention <span class="badge">${attention.filter((item) => !item.acknowledgedAt).length}</span></h2><div id="attention-list"></div></section><section><h2>Workers & tasks</h2><div id="status-view"></div></section><section class="message"><h2>Message supervisor</h2><textarea id="message-text" placeholder="Send an attributed semantic message"></textarea><button id="message-send" ${!selected || !selectedSession || !canControl(selected.id, selectedSession, "message-send") ? "disabled" : ""}>Send message</button></section></aside>
    </div><dialog id="pair-dialog"><form id="pair-form"><h2>Pair a machine</h2><p>${pendingPairing ? "One-time invitation ready. Confirm the target hub." : "Open a pairing URL generated by cas hub pair."}</p><label>Hub URL<input name="url" type="url" required value="${escapeAttr(location.origin)}"></label><label>Machine label<input name="label" required placeholder="Studio Mac"></label><label>Device label<input name="device" required placeholder="My phone"></label><label>Operator label<input name="operator" required placeholder="Your name"></label><fieldset><legend>Scopes requested</legend>${scopeChecks()}</fieldset><div class="dialog-actions"><button id="pair-cancel" type="button">Cancel</button>${pendingPairing ? "" : '<p class="pairing-disabled-reason">Pair is disabled until you open a pairing URL generated by <code>cas hub pair</code> on the machine.</p>'}<button type="submit" class="primary" ${pendingPairing ? "" : "disabled"}>Pair</button></div></form></dialog><div id="toast" role="status"></div>`;
  if (preservedGrid) document.querySelector<HTMLElement>("#pane-grid")!.replaceWith(preservedGrid);
  const machineList = document.querySelector("#machine-list")!;
  for (const machine of machines.values()) machineList.append(machineButton(machine));
  const sessionList = document.querySelector("#session-list")!;
  for (const session of machineSessions) sessionList.append(sessionButton(selected!.id, session));
  renderAttention(); renderStatus(status);
  bindEvents(selected, lease);
  if (selected && selectedSession) {
    const state = sessionStates.get(sessionKey(selected.id, selectedSession));
    if (state) queueMicrotask(() => void renderSessionState(selected.id, selectedSession!, state));
  }
}

function compatibilityWarning(machineId: string): string | undefined {
  const info = machineInfo.get(machineId);
  if (!info) return "Compatibility check unavailable: this hub may be older or newer. Read-only discovery may work, but controls stay disabled until it reports capabilities.";
  const missing = ["session_index", "daemon_attach", "machine_events"].filter((capability) => !info.capabilities.includes(capability));
  if (info.schema_version !== 1 || missing.length > 0) {
    return `Hub ${info.version} is version-skewed (schema ${info.schema_version}; missing ${missing.join(", ") || "no required capabilities"}). Upgrade or use a compatible Commander build; unsupported controls are disabled.`;
  }
  return undefined;
}

function machineButton(machine: StoredMachine): HTMLButtonElement {
  const button = document.createElement("button"); button.className = `nav-item ${machine.id === selectedMachineId ? "active" : ""}`;
  button.textContent = `${machine.label} · ${connectionStates.get(machine.id) ?? "idle"}`;
  button.onclick = () => { selectedMachineId = machine.id; selectedSession = undefined; render(); };
  return button;
}

function sessionButton(machineId: string, session: HubSession): HTMLButtonElement {
  const button = document.createElement("button"); button.className = `nav-item ${session.name === selectedSession ? "active" : ""}`;
  button.innerHTML = `<span>${escapeHtml(session.name)}</span><small>${escapeHtml(session.supervisor)} · ${session.workers.length} workers · ${session.liveness}</small>`;
  button.onclick = () => void openSession(machineId, session.name);
  return button;
}

function renderAttention(): void {
  const container = document.querySelector("#attention-list")!;
  for (const item of attention.filter((candidate) => !candidate.acknowledgedAt).slice(0, 8)) {
    const card = document.createElement("article"); card.className = "attention-item";
    const text = document.createElement("button"); text.className = "attention-open"; text.textContent = `${item.machineLabel}${item.session ? ` / ${item.session}` : ""}: ${item.message}`;
    text.onclick = () => { selectedMachineId = item.machineId; if (item.session) void openSession(item.machineId, item.session); };
    const ack = document.createElement("button"); ack.className = "ack"; ack.textContent = "Acknowledge"; ack.onclick = () => void acknowledgeAttention(item.id);
    card.append(text, ack); container.append(card);
  }
}

function renderStatus(status?: Record<string, unknown>): void {
  const container = document.querySelector("#status-view")!;
  if (!status) { container.textContent = "Open a session for push-refreshed status."; return; }
  const agents = (status.agents as any[]) ?? [];
  const tasks = [...((status.tasks_in_progress as any[]) ?? []), ...((status.tasks_ready as any[]) ?? [])];
  for (const agent of agents) { const row = document.createElement("article"); row.className = "status-row"; row.textContent = `${agent.name} · ${agent.status}${agent.current_task ? ` · ${agent.current_task}` : ""}${agent.latest_activity?.summary ? ` — ${agent.latest_activity.summary}` : ""}`; container.append(row); }
  for (const task of tasks) { const row = document.createElement("article"); row.className = "status-row"; row.textContent = `${task.id} · ${task.status} · ${task.title}`; container.append(row); }
}

function bindEvents(selected: StoredMachine | undefined, lease: LeaseState | undefined): void {
  document.querySelector<HTMLButtonElement>("#pair-toggle")!.onclick = () => (document.querySelector<HTMLDialogElement>("#pair-dialog")!).showModal();
  const pairForm = document.querySelector<HTMLFormElement>("#pair-form")!;
  document.querySelector<HTMLButtonElement>("#pair-cancel")!.onclick = () => (document.querySelector<HTMLDialogElement>("#pair-dialog")!).close();
  pairForm.onsubmit = (event) => { event.preventDefault(); void pairMachine(pairForm).then(() => (document.querySelector<HTMLDialogElement>("#pair-dialog")!).close()).catch((error) => toast(String(error))); };
  const remove = document.querySelector<HTMLButtonElement>("#remove-machine");
  if (remove && selected) remove.onclick = async () => {
    connections.get(selected.id)?.stop();
    connections.delete(selected.id); machines.delete(selected.id); sessions.delete(selected.id);
    await catalog.remove(selected.id);
    selectedMachineId = machines.keys().next().value; selectedSession = undefined;
    render();
  };
  document.querySelector<HTMLButtonElement>("#lease")!.onclick = async () => {
    if (!selected || !selectedSession) return;
    if (lease?.held_by_me) {
      await connections.get(selected.id)?.releaseLease(selectedSession);
      invalidateMachineLeases(selected.id);
    } else {
      await connections.get(selected.id)?.acquireLease(selectedSession, Boolean(lease?.controller_label && selected.scopes.includes("hub-admin")));
    }
    await loadLease(selected.id, selectedSession);
  };
  document.querySelector<HTMLButtonElement>("#interrupt")!.onclick = () => {
    if (!selected || !selectedSession) return;
    const pane = selectedPanes.get(sessionKey(selected.id, selectedSession));
    if (pane) sendControl(selected.id, selectedSession, { InterruptPane: { pane_id: pane } });
  };
  document.querySelector<HTMLButtonElement>("#message-send")!.onclick = () => {
    if (!selected || !selectedSession) return;
    const text = document.querySelector<HTMLTextAreaElement>("#message-text")!.value.trim();
    if (!text) return;
    const supervisor = sessions.get(selected.id)?.find((item) => item.name === selectedSession)?.supervisor ?? "supervisor";
    sendControl(selected.id, selectedSession, { SendMessage: { target: supervisor, text, summary: "Commander message", urgent: false, attribution: { device_id: null, credential_id: null, device_label: null, operator_label: null, controller_origin: null, request_id: null } } });
  };
}

function scopeChecks(): string {
  const scopes: Scope[] = ["machine-read", "session-read", "pane-read", "pane-input", "message-send", "pane-interrupt"];
  return scopes.map((scope) => `<label class="scope"><input type="checkbox" name="scope" value="${scope}" checked>${scope.replaceAll("-", ":")}</label>`).join("");
}

function escapeHtml(value: string): string { const span = document.createElement("span"); span.textContent = value; return span.innerHTML; }
function escapeAttr(value: string): string { return escapeHtml(value).replaceAll('"', "&quot;"); }

void boot();
