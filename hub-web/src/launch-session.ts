// New session (cas-0f51): start a factory session on a paired machine from
// Commander — machine → project → supervisor → start — and land on its
// supervisor. The sheet lives on <body>, outside #app, so a shell rebuild
// (render()) never replaces it mid-choice; it talks to the hub only through
// the LaunchHost it is given.
//
// Wire contracts (cas-cli hub/projects.rs, hub/server.rs):
//   GET  /v1/projects                  {projects[], browse_roots[]}
//   GET  /v1/projects/browse?root&path {root, path, entries[], truncated}
//   POST /v1/sessions {target, supervisor_cli, workers?}
//        202/200 {session, attached} · error {error, detail} · 403 scope_denied
import { escapeHtml, projectTitle } from "./cloud-brand";
import { LAUNCH_SCOPE, launchGrantCommand } from "./pairing-scopes";
import type { Scope } from "./types";

export type SupervisorCli = "claude" | "codex" | "grok" | "opencode";

export const SUPERVISOR_CLIS: ReadonlyArray<{ readonly id: SupervisorCli; readonly label: string }> = [
  { id: "claude", label: "Claude" },
  { id: "codex", label: "Codex" },
  { id: "grok", label: "Grok" },
  { id: "opencode", label: "OpenCode" },
];

export type LaunchTarget = { readonly kind: "project"; readonly id: string } | { readonly kind: "browse"; readonly root_id: string; readonly path: string };

export interface LaunchProject {
  id: string;
  name: string;
  path: string;
  last_touched_at: string;
  touch_count: number;
  running_session: string | null;
  target: LaunchTarget;
}

export interface BrowseRoot { id: string; name: string; path: string }

export interface ProjectCatalog { projects: LaunchProject[]; browse_roots: BrowseRoot[] }

export interface BrowseEntry {
  name: string;
  path: string;
  launchable: boolean;
  project_id: string | null;
  target: LaunchTarget | null;
}

export interface BrowseListing { root: BrowseRoot; path: string; entries: BrowseEntry[]; truncated: boolean }

export interface LaunchRequest { target: LaunchTarget; supervisor_cli: SupervisorCli; workers?: number }

export type LaunchResult =
  | { readonly ok: true; readonly session: string; readonly attached: boolean }
  | { readonly ok: false; readonly status: number; readonly code?: string; readonly detail?: string };

/** One paired machine as the sheet sees it. */
export interface LaunchMachine {
  id: string;
  label: string;
  scopes: readonly Scope[];
  /** The machine's own default supervisor CLI, when its hub says. */
  defaultCli?: string;
}

/** Everything the sheet needs from the app; no hub call happens elsewhere. */
export interface LaunchHost {
  machines(): LaunchMachine[];
  currentMachineId(): string | undefined;
  origin: string;
  projects(machineId: string, signal: AbortSignal): Promise<ProjectCatalog>;
  browse(machineId: string, rootId: string, path: string, signal: AbortSignal): Promise<BrowseListing>;
  launch(machineId: string, request: LaunchRequest): Promise<LaunchResult>;
  /** Refresh the machine's session list; whether `session` is on it now. */
  sessionListed(machineId: string, session: string): Promise<boolean>;
  /** Land on the session's supervisor. */
  open(machineId: string, session: string): void;
  copy(text: string): Promise<void>;
  /** Where focus returns when the sheet closes without landing anywhere. */
  returnFocus?(): void;
}

export function canLaunch(machine: Pick<LaunchMachine, "scopes"> | undefined): boolean {
  return machine?.scopes.includes(LAUNCH_SCOPE) === true;
}

export function supervisorCliLabel(cli: string): string {
  return SUPERVISOR_CLIS.find((entry) => entry.id === cli)?.label ?? cli;
}

/** The machine's default when its hub names a CLI this build offers, else Claude. */
export function defaultSupervisorCli(machine: Pick<LaunchMachine, "defaultCli"> | undefined): SupervisorCli {
  const named = SUPERVISOR_CLIS.find((entry) => entry.id === machine?.defaultCli);
  return named?.id ?? "claude";
}

/** Most recently touched first; the name breaks ties so the order is stable. */
export function sortProjects(projects: readonly LaunchProject[]): LaunchProject[] {
  return projects.toSorted((a, b) => (Date.parse(b.last_touched_at) || 0) - (Date.parse(a.last_touched_at) || 0) || a.name.localeCompare(b.name));
}

/** Every whitespace-separated word must appear in the name or the path. */
export function filterProjects(projects: readonly LaunchProject[], query: string): LaunchProject[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return [...projects];
  return projects.filter((project) => {
    const haystack = `${project.name} ${project.path}`.toLowerCase();
    return words.every((word) => haystack.includes(word));
  });
}

/** The workers field: empty is the machine's default; otherwise 0–16. */
export function parseWorkers(value: string): { ok: true; workers?: number } | { ok: false; message: string } {
  const trimmed = value.trim();
  if (!trimmed) return { ok: true };
  if (!/^\d{1,2}$/.test(trimmed) || Number(trimmed) > 16) return { ok: false, message: "Workers must be a whole number from 0 to 16." };
  return { ok: true, workers: Number(trimmed) };
}

/**
 * A refused launch in the operator's words. The heading says what is wrong
 * and what to do; the machine's own detail follows verbatim for whoever fixes
 * it.
 */
export function launchErrorCopy(result: { status: number; code?: string; detail?: string }, cli: string, machine: string): { title: string; advice: string } {
  const name = supervisorCliLabel(cli);
  switch (result.code) {
    case "cli_missing":
      return { title: `${name} isn't installed where ${machine}'s hub can find it.`, advice: `Install ${name} on ${machine}, or add it to the hub service's PATH, then start again.` };
    case "profile_missing":
      return { title: `The ${name} account the hub is set to use doesn't exist on ${machine}.`, advice: `Check the hub's launch profile for ${name} on ${machine} (cas doctor shows it), then start again.` };
    case "not_logged_in":
      return { title: `${name} isn't logged in on ${machine}.`, advice: `Log in to ${name} on ${machine}, then start again.` };
    case "cli_probe_failed":
      return { title: `${machine} couldn't check that ${name} is ready.`, advice: "Try again. If it keeps failing, run cas doctor on the machine." };
    case "invalid_target":
      return { title: "That folder can't be started.", advice: "Only a project's main folder can start a session, and it must still be there. Pick it again from the list." };
    case "invalid_workers":
      return { title: "The machine refused that number of workers.", advice: "Choose from 0 to 16 workers, or leave it empty for the machine's default." };
    case "invalid_supervisor_cli":
      return { title: `${machine} doesn't know the ${name} supervisor.`, advice: "Choose another supervisor, or update Cassy on the machine." };
    case "name_in_use":
      return { title: "A session with that name already exists.", advice: "Start again; the machine picks a fresh name." };
    case "revoked":
      return { title: `This browser's pairing with ${machine} is no longer active.`, advice: `Pair ${machine} again, then start the session.` };
    case "scope_denied":
      return { title: `This browser isn't allowed to start sessions on ${machine}.`, advice: "Pair again with session launch allowed." };
    default:
      if (result.status === 0) return { title: `Couldn't reach ${machine}.`, advice: "Check that the machine is awake and on your network, then start again." };
      if (result.status === 404 || result.status === 405) return { title: `${machine}'s hub can't start sessions yet.`, advice: "Update Cassy on the machine, restart its hub, then try again." };
      return { title: `${machine} couldn't start the session.`, advice: "Try again. The machine's message is below." };
  }
}

type View = "form" | "grant" | "starting" | "error";
type Mode = "known" | "browse";
type Selection = { target: LaunchTarget; name: string; path: string };
type Load<T> = { status: "idle" } | { status: "loading" } | { status: "ready"; data: T } | { status: "failed"; message: string };

/** How long a started session may take to report in before the sheet says so. */
export const LAUNCH_SETTLE_MS = 90_000;
const LAUNCH_POLL_MS = 1_000;

export function launchSheetMarkup(): string {
  const clis = SUPERVISOR_CLIS.map((cli) => `<label class="launch-cli-option"><input type="radio" name="launch-cli" value="${cli.id}"><span>${escapeHtml(cli.label)}</span><small class="launch-cli-default" hidden>Default</small></label>`).join("");
  return `<dialog id="launch-dialog" class="launch-sheet" aria-labelledby="launch-title">
  <div class="launch-flow">
    <header class="launch-head"><h2 id="launch-title">New session</h2><button type="button" class="launch-close" data-launch-action="close" aria-label="Close new session">×</button></header>
    <label class="launch-machine" data-launch-machine-field>Machine<select name="launch-machine"></select></label>
    <section class="launch-view" data-launch-view="form" aria-labelledby="launch-title">
      <fieldset class="launch-project">
        <legend>Project</legend>
        <div class="launch-tabs" role="tablist" aria-label="How to find the project">
          <button type="button" role="tab" id="launch-tab-known" data-launch-mode="known" aria-controls="launch-panel-known" aria-selected="true">Known projects</button>
          <button type="button" role="tab" id="launch-tab-browse" data-launch-mode="browse" aria-controls="launch-panel-browse" aria-selected="false" hidden>Browse</button>
        </div>
        <div id="launch-panel-known" role="tabpanel" aria-labelledby="launch-tab-known" class="launch-panel">
          <input type="search" name="launch-query" aria-label="Filter projects" placeholder="Filter projects" autocomplete="off">
          <div class="launch-list" data-launch-list="known" role="radiogroup" aria-label="Projects"></div>
        </div>
        <div id="launch-panel-browse" role="tabpanel" aria-labelledby="launch-tab-browse" class="launch-panel" hidden>
          <nav class="launch-crumbs" aria-label="Folder"></nav>
          <div class="launch-list" data-launch-list="browse" role="radiogroup" aria-label="Folders"></div>
        </div>
      </fieldset>
      <!-- Account step slot: a profile picker fed by the machine's profile list goes here once the hub exposes one. -->
      <fieldset class="launch-cli" aria-describedby="launch-cli-hint"><legend>Supervisor</legend><div class="launch-cli-options">${clis}</div><small id="launch-cli-hint" class="field-hint">Runs the session's supervisor.</small></fieldset>
      <label class="launch-workers"><span>Workers <span class="launch-optional">(optional)</span></span><input name="launch-workers" type="text" inputmode="numeric" pattern="[0-9]*" autocomplete="off" placeholder="Machine default" aria-describedby="launch-workers-hint"><small id="launch-workers-hint" class="field-hint">0 to 16. Leave empty for the machine's default.</small></label>
      <p class="launch-invalid" role="alert" hidden></p>
      <div class="dialog-actions launch-actions"><p class="launch-summary" aria-live="polite"></p><button type="button" data-launch-action="close">Cancel</button><button type="button" class="primary" data-launch-action="start" aria-disabled="true">Start</button></div>
    </section>
    <section class="launch-view launch-grant" data-launch-view="grant" hidden>
      <p class="launch-lead"></p>
      <p>Starting sessions is a separate permission this browser asks for once per machine. Run this on the machine, then open the link it prints in this browser. What this browser can do now is kept.</p>
      <div class="pair-code-actions launch-grant-command"><code></code><button type="button" data-launch-action="copy">Copy command</button></div>
      <p class="field-hint launch-grant-note">Opening the new link replaces this browser's pairing with the machine.</p>
      <div class="dialog-actions"><button type="button" data-launch-action="close">Close</button></div>
    </section>
    <section class="launch-view launch-starting" data-launch-view="starting" hidden>
      <div role="status" class="launch-progress"><p class="launch-progress-title"></p><p class="launch-progress-step"></p></div>
      <p class="field-hint">You can close this. The session keeps starting on the machine and appears in your conversations.</p>
      <div class="dialog-actions"><button type="button" data-launch-action="close">Close</button></div>
    </section>
    <section class="launch-view launch-error" data-launch-view="error" hidden>
      <div role="alert"><h3 class="launch-error-title"></h3><p class="launch-error-advice"></p></div>
      <details class="launch-error-detail" hidden><summary>The machine's message</summary><pre></pre></details>
      <div class="dialog-actions"><button type="button" data-launch-action="close">Close</button><button type="button" class="primary" data-launch-action="back">Back</button></div>
    </section>
  </div>
</dialog>`;
}

export class LaunchSheet {
  private dialog: HTMLDialogElement | undefined;
  private machineId: string | undefined;
  private view: View = "form";
  private mode: Mode = "known";
  private query = "";
  private catalog: Load<ProjectCatalog> = { status: "idle" };
  private browseRoot: string | undefined;
  private browsePath = "";
  private listing: Load<BrowseListing> = { status: "idle" };
  private selection: Selection | undefined;
  private cli: SupervisorCli = "claude";
  private cliChosen = false;
  private loads = new AbortController();
  /** Bumped per launch; a stale wait never lands the operator anywhere. */
  private launchGeneration = 0;
  private startedAt = 0;
  private ticker: number | undefined;
  private landing = false;

  constructor(private readonly host: LaunchHost, private readonly doc: Document = document) {}

  get isOpen(): boolean { return this.dialog?.open === true; }

  /**
   * Open on `machineId` when given; otherwise on the current machine if it
   * can launch, else the first machine that can, else the current one (whose
   * grant path then shows).
   */
  open(machineId?: string): void {
    const dialog = this.ensureDialog();
    const machines = this.host.machines();
    const current = this.host.currentMachineId();
    const chosen = (machineId ? machines.find((m) => m.id === machineId) : undefined)
      ?? machines.find((m) => m.id === current && canLaunch(m)) ?? machines.find(canLaunch) ?? machines.find((m) => m.id === current) ?? machines[0];
    this.view = "form";
    this.selectMachine(chosen?.id);
    if (!dialog.open) dialog.showModal();
    this.focusFirst();
  }

  close(): void {
    this.launchGeneration += 1;
    this.stopTicker();
    this.loads.abort();
    if (this.dialog?.open) this.dialog.close();
  }

  private ensureDialog(): HTMLDialogElement {
    if (this.dialog?.isConnected) return this.dialog;
    const template = this.doc.createElement("template");
    template.innerHTML = launchSheetMarkup();
    const dialog = template.content.firstElementChild as HTMLDialogElement;
    this.doc.body.append(dialog);
    this.dialog = dialog;
    this.bind(dialog);
    return dialog;
  }

  private $(selector: string): HTMLElement {
    return this.dialog!.querySelector<HTMLElement>(selector)!;
  }

  private bind(dialog: HTMLDialogElement): void {
    dialog.addEventListener("close", () => {
      this.launchGeneration += 1;
      this.stopTicker();
      this.loads.abort();
      if (!this.landing) this.host.returnFocus?.();
    });
    dialog.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      const action = target.closest<HTMLElement>("[data-launch-action]")?.dataset.launchAction;
      if (action === "close") { this.close(); return; }
      if (action === "start") { void this.start(); return; }
      if (action === "back") { this.showView("form"); this.focusFirst(); return; }
      if (action === "copy") { void this.copyGrant(target as HTMLButtonElement); return; }
      const mode = target.closest<HTMLElement>("[data-launch-mode]")?.dataset.launchMode as Mode | undefined;
      if (mode) { this.setMode(mode); return; }
      const attach = target.closest<HTMLElement>("[data-launch-attach]")?.dataset.launchAttach;
      if (attach && this.machineId) { this.land(this.machineId, attach); return; }
      const folder = target.closest<HTMLElement>("[data-launch-folder]");
      if (folder) { this.navigate(folder.dataset.launchRoot, folder.dataset.launchFolder ?? ""); return; }
    });
    dialog.addEventListener("change", (event) => {
      const target = event.target as HTMLInputElement | HTMLSelectElement;
      if (target.name === "launch-machine") { this.selectMachine(target.value); return; }
      if (target.name === "launch-cli") { this.cli = target.value as SupervisorCli; this.cliChosen = true; this.syncChecked(); this.syncActions(); return; }
      if (target.name === "launch-project") { this.choose(target as HTMLInputElement); return; }
    });
    dialog.addEventListener("input", (event) => {
      const target = event.target as HTMLInputElement;
      if (target.name === "launch-query") { this.query = target.value; this.renderKnown(); return; }
      if (target.name === "launch-workers") { this.$(".launch-invalid").hidden = true; this.syncActions(); }
    });
    dialog.addEventListener("keydown", (event) => {
      // Arrow keys move between the two tabs, as a tablist does.
      const tab = (event.target as HTMLElement).closest<HTMLElement>("[role=tab]");
      if (!tab || (event.key !== "ArrowLeft" && event.key !== "ArrowRight")) return;
      const tabs = [...dialog.querySelectorAll<HTMLElement>("[role=tab]:not([hidden])")];
      const next = tabs[(tabs.indexOf(tab) + (event.key === "ArrowRight" ? 1 : tabs.length - 1)) % tabs.length];
      if (next) { event.preventDefault(); this.setMode(next.dataset.launchMode as Mode); next.focus(); }
    });
  }

  private machine(): LaunchMachine | undefined {
    return this.host.machines().find((m) => m.id === this.machineId);
  }

  private selectMachine(machineId: string | undefined): void {
    this.loads.abort();
    this.loads = new AbortController();
    this.machineId = machineId;
    this.catalog = { status: "idle" };
    this.listing = { status: "idle" };
    this.browseRoot = undefined;
    this.browsePath = "";
    this.selection = undefined;
    this.mode = "known";
    const machine = this.machine();
    if (!this.cliChosen) this.cli = defaultSupervisorCli(machine);
    this.renderMachines();
    if (this.view === "form" || this.view === "grant") this.view = canLaunch(machine) ? "form" : "grant";
    this.renderAll();
    if (machine && canLaunch(machine)) void this.loadCatalog(machine.id);
  }

  private async loadCatalog(machineId: string): Promise<void> {
    const signal = this.loads.signal;
    this.catalog = { status: "loading" };
    this.renderKnown();
    try {
      const data = await this.host.projects(machineId, signal);
      if (signal.aborted || this.machineId !== machineId) return;
      this.catalog = { status: "ready", data: { projects: sortProjects(data.projects ?? []), browse_roots: data.browse_roots ?? [] } };
    } catch (error) {
      if (signal.aborted || this.machineId !== machineId) return;
      this.catalog = { status: "failed", message: `Couldn't load ${this.machine()?.label ?? "the machine"}'s projects. ${error instanceof Error ? error.message : ""}`.trim() };
    }
    this.renderAll();
    // The sheet opened on its title while the list loaded; the filter is next.
    if (this.dialog?.open && this.doc.activeElement === this.$(".launch-head h2")) this.focusFirst();
  }

  private setMode(mode: Mode): void {
    if (this.mode === mode) return;
    this.mode = mode;
    this.selection = undefined;
    this.renderAll();
    if (mode === "browse" && this.listing.status === "idle") {
      const roots = this.catalog.status === "ready" ? this.catalog.data.browse_roots : [];
      // One root opens straight into it; several start from the list of roots.
      if (roots.length === 1) this.navigate(roots[0]!.id, "");
    }
  }

  private navigate(rootId: string | undefined, path: string): void {
    const machineId = this.machineId;
    if (!machineId) return;
    this.selection = undefined;
    if (!rootId) {
      this.browseRoot = undefined;
      this.browsePath = "";
      this.listing = { status: "idle" };
      this.renderBrowse();
      this.syncActions();
      return;
    }
    this.browseRoot = rootId;
    this.browsePath = path;
    this.listing = { status: "loading" };
    this.renderBrowse();
    this.syncActions();
    const signal = this.loads.signal;
    this.host.browse(machineId, rootId, path, signal).then((data) => {
      if (signal.aborted || this.machineId !== machineId || this.browseRoot !== rootId || this.browsePath !== path) return;
      this.listing = { status: "ready", data };
      this.renderBrowse();
      this.dialog?.querySelector<HTMLElement>('[data-launch-list="browse"] :is(input, button)')?.focus();
    }, (error: unknown) => {
      if (signal.aborted || this.browseRoot !== rootId || this.browsePath !== path) return;
      this.listing = { status: "failed", message: `Couldn't open that folder. ${error instanceof Error ? error.message : ""}`.trim() };
      this.renderBrowse();
    });
  }

  private choose(input: HTMLInputElement): void {
    const target = JSON.parse(input.dataset.launchTarget ?? "null") as LaunchTarget | null;
    if (!target) return;
    this.selection = { target, name: input.dataset.launchName ?? input.value, path: input.dataset.launchPath ?? "" };
    this.syncChecked();
    this.$(".launch-invalid").hidden = true;
    this.syncActions();
  }

  private renderMachines(): void {
    const machines = this.host.machines();
    const select = this.$("select[name=launch-machine]") as HTMLSelectElement;
    select.innerHTML = machines.map((m) => `<option value="${escapeHtml(m.id)}"${m.id === this.machineId ? " selected" : ""}>${escapeHtml(m.label)}${canLaunch(m) ? "" : " · can't start sessions yet"}</option>`).join("");
    // One machine needs no picker; its name leads the summary instead.
    this.$("[data-launch-machine-field]").hidden = machines.length < 2;
  }

  private renderAll(): void {
    if (!this.dialog) return;
    this.showView(this.view);
    this.renderGrant();
    const roots = this.catalog.status === "ready" ? this.catalog.data.browse_roots : [];
    const browseTab = this.$("#launch-tab-browse");
    browseTab.hidden = roots.length === 0;
    if (browseTab.hidden && this.mode === "browse") this.mode = "known";
    for (const tab of this.dialog.querySelectorAll<HTMLElement>("[role=tab]")) {
      const selected = tab.dataset.launchMode === this.mode;
      tab.setAttribute("aria-selected", String(selected));
      tab.tabIndex = selected ? 0 : -1;
    }
    this.$("#launch-panel-known").hidden = this.mode !== "known";
    this.$("#launch-panel-browse").hidden = this.mode !== "browse";
    for (const input of this.dialog.querySelectorAll<HTMLInputElement>("input[name=launch-cli]")) {
      input.checked = input.value === this.cli;
      const tag = input.parentElement?.querySelector<HTMLElement>(".launch-cli-default");
      if (tag) tag.hidden = input.value !== defaultSupervisorCli(this.machine());
    }
    this.renderKnown();
    this.renderBrowse();
    this.syncChecked();
    this.syncActions();
  }

  /** The checked choice's row carries a class, not only :has(), so browsers without :has still show it. */
  private syncChecked(): void {
    for (const input of this.dialog?.querySelectorAll<HTMLInputElement>("input[type=radio]") ?? []) input.closest("label")?.classList.toggle("is-checked", input.checked);
  }

  private showView(view: View): void {
    this.view = view;
    for (const section of this.dialog!.querySelectorAll<HTMLElement>("[data-launch-view]")) section.hidden = section.dataset.launchView !== view;
    // The machine picker belongs to choosing; a launch in progress keeps its machine.
    const machineField = this.$("[data-launch-machine-field]");
    if (view === "starting" || view === "error") machineField.hidden = true;
    else machineField.hidden = this.host.machines().length < 2;
  }

  private renderGrant(): void {
    const machine = this.machine();
    const label = machine?.label ?? "this machine";
    this.$(".launch-grant .launch-lead").innerHTML = `This browser can't start sessions on <strong>${escapeHtml(label)}</strong> yet.`;
    const command = launchGrantCommand(this.host.origin, machine?.scopes ?? []);
    const code = this.$(".launch-grant-command code");
    code.textContent = command;
    this.$(".launch-grant-command button").dataset.command = command;
  }

  private renderKnown(): void {
    const list = this.dialog?.querySelector<HTMLElement>('[data-launch-list="known"]');
    if (!list) return;
    const search = this.$("input[name=launch-query]") as HTMLInputElement;
    if (search.value !== this.query) search.value = this.query;
    if (this.catalog.status === "loading" || this.catalog.status === "idle") {
      list.innerHTML = `<p class="launch-empty" role="status">Loading projects…</p>`;
      search.disabled = true;
      return;
    }
    if (this.catalog.status === "failed") {
      list.innerHTML = `<p class="launch-empty" role="alert">${escapeHtml(this.catalog.message)}</p>`;
      search.disabled = true;
      return;
    }
    search.disabled = false;
    const all = this.catalog.data.projects;
    const shown = filterProjects(all, this.query);
    if (!all.length) {
      list.innerHTML = `<p class="launch-empty">No projects on this machine yet.${this.catalog.data.browse_roots.length ? " Browse to find one." : " Run Cassy in a project on the machine once and it appears here."}</p>`;
      return;
    }
    if (!shown.length) {
      list.innerHTML = `<p class="launch-empty" role="status">No project matches “${escapeHtml(this.query)}”.</p>`;
      return;
    }
    const selectedId = this.selection?.target.kind === "project" ? this.selection.target.id : undefined;
    list.innerHTML = shown.map((project) => projectRowMarkup(project, project.id === selectedId)).join("");
  }

  private renderBrowse(): void {
    const list = this.dialog?.querySelector<HTMLElement>('[data-launch-list="browse"]');
    const crumbs = this.dialog?.querySelector<HTMLElement>(".launch-crumbs");
    if (!list || !crumbs) return;
    const roots = this.catalog.status === "ready" ? this.catalog.data.browse_roots : [];
    const root = roots.find((r) => r.id === this.browseRoot);
    crumbs.innerHTML = crumbsMarkup(roots, root, this.browsePath);
    if (!root) {
      list.innerHTML = roots.map((r) => `<button type="button" class="launch-row launch-folder" data-launch-root="${escapeHtml(r.id)}" data-launch-folder=""><span class="launch-row-name">${escapeHtml(r.name)}</span><small class="launch-row-path">${escapeHtml(r.path)}</small><span class="launch-chevron" aria-hidden="true">›</span></button>`).join("");
      return;
    }
    if (this.listing.status === "loading" || this.listing.status === "idle") { list.innerHTML = `<p class="launch-empty" role="status">Opening folder…</p>`; return; }
    if (this.listing.status === "failed") { list.innerHTML = `<p class="launch-empty" role="alert">${escapeHtml(this.listing.message)}</p>`; return; }
    const { entries, truncated } = this.listing.data;
    const selected = this.selection?.target.kind === "browse" ? this.selection.target.path : undefined;
    const rows = entries.map((entry) => entry.launchable && entry.target
      ? browseProjectRowMarkup(entry, root, entry.target.kind === "browse" && entry.target.path === selected)
      : `<button type="button" class="launch-row launch-folder" data-launch-root="${escapeHtml(root.id)}" data-launch-folder="${escapeHtml(entry.path)}"><span class="launch-row-name">${escapeHtml(entry.name)}</span><small class="launch-row-path">Folder</small><span class="launch-chevron" aria-hidden="true">›</span></button>`).join("");
    list.innerHTML = (rows || `<p class="launch-empty">No folders here.</p>`)
      + (truncated ? `<p class="launch-empty launch-truncated">Showing the first ${entries.length} folders. Open a folder to narrow it down.</p>` : "");
  }

  private syncActions(): void {
    if (!this.dialog) return;
    const start = this.$('[data-launch-action="start"]');
    const summary = this.$(".launch-summary");
    const machine = this.machine();
    const ready = Boolean(this.selection && machine && canLaunch(machine));
    start.setAttribute("aria-disabled", String(!ready));
    summary.textContent = this.selection && machine
      ? `Start ${this.selection.name} with ${supervisorCliLabel(this.cli)} on ${machine.label}.`
      : "Choose a project to start.";
  }

  private async start(): Promise<void> {
    const machine = this.machine();
    if (!machine || !this.selection) {
      const invalid = this.$(".launch-invalid");
      invalid.textContent = "Choose a project first.";
      invalid.hidden = false;
      return;
    }
    const workers = parseWorkers((this.$("input[name=launch-workers]") as HTMLInputElement).value);
    if (!workers.ok) {
      const invalid = this.$(".launch-invalid");
      invalid.textContent = workers.message;
      invalid.hidden = false;
      (this.$("input[name=launch-workers]") as HTMLInputElement).focus();
      return;
    }
    const generation = ++this.launchGeneration;
    const selection = this.selection;
    const cli = this.cli;
    const request: LaunchRequest = { target: selection.target, supervisor_cli: cli, ...(workers.workers === undefined ? {} : { workers: workers.workers }) };
    this.showStarting(`Starting ${selection.name} with ${supervisorCliLabel(cli)} on ${machine.label}…`, `Asking ${machine.label} to start it.`);
    let result: LaunchResult;
    try {
      result = await this.host.launch(machine.id, request);
    } catch (error) {
      result = { ok: false, status: 0, detail: error instanceof Error ? error.message : String(error) };
    }
    if (generation !== this.launchGeneration) return;
    if (!result.ok) {
      if (result.status === 403 && (result.code === "scope_denied" || result.code === undefined)) {
        this.stopTicker();
        this.view = "grant";
        this.renderAll();
        return;
      }
      this.showError(launchErrorCopy(result, cli, machine.label), result.detail);
      return;
    }
    if (result.attached) { this.land(machine.id, result.session); return; }
    this.$(".launch-progress-step").textContent = `Waiting for ${result.session} to come up on ${machine.label}.`;
    const deadline = Date.now() + LAUNCH_SETTLE_MS;
    while (generation === this.launchGeneration) {
      let listed = false;
      try { listed = await this.host.sessionListed(machine.id, result.session); } catch { /* keep waiting: the machine may be busy starting it */ }
      if (generation !== this.launchGeneration) return;
      if (listed) { this.land(machine.id, result.session); return; }
      if (Date.now() >= deadline) {
        this.showError({
          title: `${machine.label} started ${result.session}, but it hasn't come up yet.`,
          advice: "It may still be starting. It appears in your conversations when it does; if it never does, check cas doctor on the machine.",
        });
        return;
      }
      await new Promise((resolve) => setTimeout(resolve, LAUNCH_POLL_MS));
    }
  }

  private showStarting(title: string, step: string): void {
    this.showView("starting");
    this.$(".launch-progress-title").textContent = title;
    this.$(".launch-progress-step").textContent = step;
    this.startedAt = Date.now();
    this.stopTicker();
    // A quiet elapsed count, not a looping animation (DESIGN.md).
    const elapsed = this.doc.createElement("span");
    elapsed.className = "launch-elapsed";
    elapsed.setAttribute("aria-hidden", "true");
    this.$(".launch-progress-title").append(" ", elapsed);
    const tick = () => { elapsed.textContent = `${Math.floor((Date.now() - this.startedAt) / 1000)} s`; };
    tick();
    this.ticker = window.setInterval(tick, 1000);
    this.$(".launch-starting [data-launch-action=close]").focus();
  }

  private showError(copy: { title: string; advice: string }, detail?: string): void {
    this.stopTicker();
    this.showView("error");
    this.$(".launch-error-title").textContent = copy.title;
    this.$(".launch-error-advice").textContent = copy.advice;
    const details = this.$(".launch-error-detail") as HTMLDetailsElement;
    details.hidden = !detail;
    details.querySelector("pre")!.textContent = detail ?? "";
    this.$('.launch-error [data-launch-action="back"]').focus();
  }

  private land(machineId: string, session: string): void {
    this.launchGeneration += 1;
    this.stopTicker();
    this.loads.abort();
    // Landing moves focus to the conversation, not back to the opener.
    this.landing = true;
    try { if (this.dialog?.open) this.dialog.close(); } finally { this.landing = false; }
    this.host.open(machineId, session);
  }

  private async copyGrant(button: HTMLButtonElement): Promise<void> {
    const command = button.dataset.command ?? "";
    try {
      await this.host.copy(command);
      button.textContent = "Copied";
    } catch {
      button.textContent = "Select and copy the command";
    }
    window.setTimeout(() => { button.textContent = "Copy command"; }, 2000);
  }

  private focusFirst(): void {
    queueMicrotask(() => {
      if (!this.dialog?.open) return;
      if (this.view === "grant") { this.$('.launch-grant [data-launch-action="copy"]').focus(); return; }
      const search = this.$("input[name=launch-query]") as HTMLInputElement;
      if (!search.disabled && !search.closest("[hidden]")) { search.focus(); return; }
      const title = this.$(".launch-head h2");
      title.tabIndex = -1;
      title.focus();
    });
  }

  private stopTicker(): void {
    if (this.ticker !== undefined) window.clearInterval(this.ticker);
    this.ticker = undefined;
  }
}

function projectRowMarkup(project: LaunchProject, checked: boolean): string {
  const name = escapeHtml(project.name || projectTitle(project.path) || project.path);
  const path = escapeHtml(project.path);
  if (project.running_session) {
    // Already running: one session per project, so the choice is to attach.
    return `<div class="launch-row launch-running"><span class="launch-row-text"><span class="launch-row-name">${name}</span><small class="launch-row-path">${path}</small><small class="launch-row-state"><span class="launch-dot" aria-hidden="true"></span>Running · <span class="codename">${escapeHtml(project.running_session)}</span></small></span><button type="button" class="launch-attach primary" data-launch-attach="${escapeHtml(project.running_session)}" aria-label="Attach to ${name} (${escapeHtml(project.running_session)})">Attach</button></div>`;
  }
  return `<label class="launch-row launch-choice"><input type="radio" name="launch-project" value="${escapeHtml(project.id)}" data-launch-name="${name}" data-launch-path="${path}" data-launch-target="${escapeHtml(JSON.stringify(project.target))}"${checked ? " checked" : ""}><span class="launch-row-text"><span class="launch-row-name">${name}</span><small class="launch-row-path">${path}</small></span></label>`;
}

function browseProjectRowMarkup(entry: BrowseEntry, root: BrowseRoot, checked: boolean): string {
  const name = escapeHtml(entry.name);
  const path = escapeHtml(`${root.path.replace(/\/$/, "")}/${entry.path}`);
  return `<label class="launch-row launch-choice"><input type="radio" name="launch-project" value="${escapeHtml(entry.path)}" data-launch-name="${name}" data-launch-path="${path}" data-launch-target="${escapeHtml(JSON.stringify(entry.target))}"${checked ? " checked" : ""}><span class="launch-row-text"><span class="launch-row-name">${name}</span><small class="launch-row-path">Project folder</small></span></label>`;
}

function crumbsMarkup(roots: readonly BrowseRoot[], root: BrowseRoot | undefined, path: string): string {
  if (!root) return `<span class="launch-crumb-current">${roots.length > 1 ? "Choose where to look" : ""}</span>`;
  const parts = path.split("/").filter(Boolean);
  const crumbs: string[] = [];
  if (roots.length > 1) crumbs.push(`<button type="button" class="launch-crumb" data-launch-folder="" aria-label="All launch folders">All</button>`);
  const rootLabel = escapeHtml(root.name);
  crumbs.push(parts.length ? `<button type="button" class="launch-crumb" data-launch-root="${escapeHtml(root.id)}" data-launch-folder="">${rootLabel}</button>` : `<span class="launch-crumb-current" aria-current="location">${rootLabel}</span>`);
  parts.forEach((part, index) => {
    const sub = parts.slice(0, index + 1).join("/");
    crumbs.push(index === parts.length - 1
      ? `<span class="launch-crumb-current" aria-current="location">${escapeHtml(part)}</span>`
      : `<button type="button" class="launch-crumb" data-launch-root="${escapeHtml(root.id)}" data-launch-folder="${escapeHtml(sub)}">${escapeHtml(part)}</button>`);
  });
  return crumbs.join('<span class="launch-crumb-sep" aria-hidden="true">/</span>');
}
