// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import {
  LaunchSheet, SessionLaunchGrantError, accountLoginCommand, accountStep, canLaunch, defaultAccount, defaultSupervisorCli, launchSheetMarkup, launchSummary,
  type LaunchProfiles, commandTokensMarkup, filterProjects, launchErrorCopy, parseWorkers, sortProjects,
  type BrowseListing, type LaunchHost, type LaunchMachine, type LaunchProject, type LaunchRequest, type LaunchResult, type ProjectCatalog,
} from "./launch-session";
import { canEnableSessionLaunch, launchDropped, launchDroppedNotice, launchGrantCommand, parseGrantedScopes, repairCommand, repairStatus, scopeChoices, scopeSummary } from "./pairing-scopes";
import type { Scope } from "./types";

const CONTROL: Scope[] = ["machine-read", "session-read", "pane-read", "pane-input", "message-send", "pane-interrupt"];

function project(name: string, touched: string, running: string | null = null): LaunchProject {
  return { id: `p-${name}`, name, path: `/home/dev/${name}`, last_touched_at: touched, touch_count: 1, running_session: running, target: { kind: "project", id: `p-${name}` } };
}

beforeAll(() => {
  // jsdom has <dialog> without the modal methods.
  const proto = HTMLDialogElement.prototype as HTMLDialogElement & { showModal: () => void; close: () => void };
  proto.showModal = function (this: HTMLDialogElement) { this.setAttribute("open", ""); };
  proto.close = function (this: HTMLDialogElement) { if (!this.hasAttribute("open")) return; this.removeAttribute("open"); this.dispatchEvent(new Event("close")); };
});

afterEach(() => { document.body.innerHTML = ""; vi.useRealTimers(); });

describe("launch model", () => {
  it("sorts projects by recency, then name", () => {
    const sorted = sortProjects([project("b", "2026-09-01T00:00:00Z"), project("a", "2026-09-20T00:00:00Z"), project("c", "2026-09-01T00:00:00Z")]);
    expect(sorted.map((p) => p.name)).toEqual(["a", "b", "c"]);
  });

  it("filters on every word across the name and path", () => {
    const all = [project("cas-src", "2026-09-01T00:00:00Z"), project("gabber-studio", "2026-09-01T00:00:00Z")];
    expect(filterProjects(all, "CAS").map((p) => p.name)).toEqual(["cas-src"]);
    expect(filterProjects(all, "dev studio").map((p) => p.name)).toEqual(["gabber-studio"]);
    expect(filterProjects(all, "  ")).toHaveLength(2);
    expect(filterProjects(all, "nothing")).toEqual([]);
  });

  it("reads the workers field as optional 0–16", () => {
    expect(parseWorkers("")).toEqual({ ok: true });
    expect(parseWorkers(" 4 ")).toEqual({ ok: true, workers: 4 });
    expect(parseWorkers("0")).toEqual({ ok: true, workers: 0 });
    expect(parseWorkers("17").ok).toBe(false);
    expect(parseWorkers("-1").ok).toBe(false);
    expect(parseWorkers("2.5").ok).toBe(false);
  });

  it("offers only the supervisors the hub launches", () => {
    document.body.innerHTML = launchSheetMarkup();
    expect([...document.querySelectorAll<HTMLInputElement>("input[name=launch-cli]")].map((input) => input.value)).toEqual(["claude", "codex", "grok"]);
  });

  it("defaults the supervisor to the machine's own CLI, else Claude", () => {
    expect(defaultSupervisorCli({ defaultCli: "codex" })).toBe("codex");
    expect(defaultSupervisorCli({ defaultCli: "opencode" })).toBe("claude");
    expect(defaultSupervisorCli(undefined)).toBe("claude");
  });

  it("allows launch only with the session-launch scope", () => {
    expect(canLaunch({ scopes: CONTROL })).toBe(false);
    expect(canLaunch({ scopes: [...CONTROL, "session-launch"] })).toBe(true);
    expect(canLaunch(undefined)).toBe(false);
  });

  it("offers self grant only with all three control scopes", () => {
    expect(canEnableSessionLaunch(CONTROL)).toBe(true);
    for (const scope of ["pane-input", "message-send", "pane-interrupt"] as Scope[]) {
      expect(canEnableSessionLaunch(CONTROL.filter((entry) => entry !== scope))).toBe(false);
    }
  });

  it("says what each environment refusal means and what to do", () => {
    expect(launchErrorCopy({ status: 422, code: "not_logged_in" }, "claude", "Atlas").title).toBe("Claude isn't logged in on Atlas.");
    expect(launchErrorCopy({ status: 422, code: "cli_missing" }, "codex", "Atlas").title).toContain("Codex isn't installed");
    expect(launchErrorCopy({ status: 422, code: "profile_missing" }, "claude", "Atlas").title).toContain("account");
    expect(launchErrorCopy({ status: 422, code: "cli_probe_failed" }, "claude", "Atlas").advice).toContain("cas doctor");
    expect(launchErrorCopy({ status: 400, code: "invalid_target" }, "claude", "Atlas").title).toBe("That folder can't be started.");
    expect(launchErrorCopy({ status: 0 }, "claude", "Atlas").title).toBe("Couldn't reach Atlas.");
    expect(launchErrorCopy({ status: 405 }, "claude", "Atlas").title).toContain("can't start sessions yet");
    expect(launchErrorCopy({ status: 500, code: "launch_failed" }, "claude", "Atlas").title).toBe("Atlas couldn't start the session.");
    expect(launchErrorCopy({ status: 503, code: "containment_unavailable" }, "claude", "Atlas").advice).toContain("Nothing was started");
  });
});

describe("session launch scope (cas-0f51)", () => {
  it("keeps an invitation that grants session:launch instead of downgrading it to read-only", () => {
    expect(parseGrantedScopes("machine:read,session:launch")).toEqual(["machine-read", "session-launch"]);
  });

  it("offers the launch box only when the invitation grants it", () => {
    expect(scopeChoices(CONTROL, CONTROL).map((c) => c.scope)).not.toContain("session-launch");
    expect(scopeChoices(undefined, CONTROL).map((c) => c.scope)).not.toContain("session-launch");
    const granted = [...CONTROL, "session-launch"] as Scope[];
    expect(scopeChoices(granted, granted).find((c) => c.scope === "session-launch")).toMatchObject({ granted: true, checked: true });
  });

  it("names launch in the plain summary", () => {
    expect(scopeSummary([...CONTROL, "session-launch"])).toContain("Start new sessions");
  });

  it("re-pairs with the current scopes plus launch", () => {
    expect(launchGrantCommand("https://hub.example", CONTROL)).toBe("cas hub pair --origin https://hub.example --scopes machine:read,session:read,pane:read,pane:input,message:send,pane:interrupt,session:launch");
    expect(launchGrantCommand("https://hub.example", ["machine-read", "session-launch"])).toBe("cas hub pair --origin https://hub.example --scopes machine:read,session:launch");
  });
});

type Calls = { launches: Array<{ machineId: string; request: LaunchRequest }>; grants: string[]; opened: Array<[string, string]>; browsed: Array<[string, string]> };

function sheet(options: {
  machines: LaunchMachine[];
  catalog?: ProjectCatalog;
  listing?: (root: string, path: string) => BrowseListing;
  result?: LaunchResult;
  listedAfter?: number;
  profiles?: LaunchProfiles;
  grantError?: SessionLaunchGrantError;
  grantWait?: Promise<void>;
}): { sheet: LaunchSheet; calls: Calls; dialog: () => HTMLDialogElement } {
  const calls: Calls = { launches: [], grants: [], opened: [], browsed: [] };
  let polls = 0;
  const host: LaunchHost = {
    machines: () => options.machines,
    currentMachineId: () => options.machines[0]?.id,
    origin: "https://hub.example",
    projects: async () => options.catalog ?? { projects: [], browse_roots: [] },
    profiles: async () => options.profiles ?? {},
    browse: async (_machine, root, path) => { calls.browsed.push([root, path]); return options.listing!(root, path); },
    launch: async (machineId, request) => { calls.launches.push({ machineId, request }); return options.result ?? { ok: true, session: "new-otter-1", attached: false }; },
    grant: async (machineId) => { calls.grants.push(machineId); await options.grantWait; if (options.grantError) throw options.grantError; const machine = options.machines.find((entry) => entry.id === machineId); if (machine) machine.scopes = [...machine.scopes, "session-launch"]; },
    sessionListed: async () => ++polls >= (options.listedAfter ?? 1),
    open: (machineId, session) => { calls.opened.push([machineId, session]); },
    copy: async () => undefined,
  };
  const instance = new LaunchSheet(host, document);
  return { sheet: instance, calls, dialog: () => document.querySelector<HTMLDialogElement>("#launch-dialog")! };
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
const visibleView = (dialog: HTMLDialogElement) => [...dialog.querySelectorAll<HTMLElement>("[data-launch-view]")].filter((v) => !v.hidden).map((v) => v.dataset.launchView);
const ATLAS: LaunchMachine = { id: "atlas", label: "Atlas", scopes: [...CONTROL, "session-launch"] };

describe("LaunchSheet", () => {
  it("grants launch once from the named consent and focuses project search", async () => {
    const { sheet: s, calls, dialog } = sheet({ machines: [{ id: "atlas", label: "Atlas", scopes: CONTROL }] });
    s.open();
    await flush();
    expect(dialog().open).toBe(true);
    expect(visibleView(dialog())).toEqual(["grant"]);
    expect(dialog().querySelector(".launch-grant")!.textContent).toContain("Allow starting sessions on Atlas");
    expect((dialog().querySelector('[data-launch-action="allow"]') as HTMLButtonElement).hidden).toBe(false);
    (dialog().querySelector('[data-launch-action="allow"]') as HTMLButtonElement).click();
    await vi.waitFor(() => expect(visibleView(dialog())).toEqual(["form"]));
    expect(calls.grants).toEqual(["atlas"]);
    expect(calls.launches).toEqual([]);
    await vi.waitFor(() => expect(document.activeElement).toBe(dialog().querySelector('input[name="launch-query"]')));
  });

  it("sends one grant while Allow is pending and leaves no automatic session launch", async () => {
    let release!: () => void;
    const grantWait = new Promise<void>((resolve) => { release = resolve; });
    const { sheet: s, calls, dialog } = sheet({ machines: [{ id: "atlas", label: "Atlas", scopes: [...CONTROL] }], grantWait });
    s.open();
    await flush();
    const allow = dialog().querySelector('[data-launch-action="allow"]') as HTMLButtonElement;
    allow.click();
    expect(allow.disabled).toBe(true);
    // A queued activation must not create a second scope request either.
    allow.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(calls.grants).toEqual(["atlas"]);
    release();
    await vi.waitFor(() => expect(visibleView(dialog())).toEqual(["form"]));
    expect(calls.launches).toEqual([]);
  });

  it("keeps invitation guidance for a read-only pairing", async () => {
    const { sheet: s, calls, dialog } = sheet({ machines: [{ id: "atlas", label: "Atlas", scopes: ["machine-read", "session-read", "pane-read"] }] });
    s.open();
    await flush();
    expect((dialog().querySelector('[data-launch-action="allow"]') as HTMLButtonElement).hidden).toBe(true);
    expect(dialog().querySelector(".launch-grant-command code")!.textContent).toContain("session:launch");
    dialog().querySelector('[data-launch-action="allow"]')!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(calls.grants).toEqual([]);
  });

  it("names and grants only the newly selected machine after one consent", async () => {
    const machines: LaunchMachine[] = [
      { id: "atlas", label: "Atlas", scopes: [...CONTROL] },
      { id: "soundwave", label: "soundwave", scopes: [...CONTROL] },
    ];
    const { sheet: s, calls, dialog } = sheet({ machines });
    s.open("atlas");
    await flush();
    const picker = dialog().querySelector('select[name="launch-machine"]') as HTMLSelectElement;
    picker.value = "soundwave";
    picker.dispatchEvent(new Event("change", { bubbles: true }));
    expect(visibleView(dialog())).toEqual(["grant"]);
    expect(dialog().querySelector('.launch-grant .launch-lead')!.textContent).toContain("soundwave");
    const allow = dialog().querySelector('[data-launch-action="allow"]') as HTMLButtonElement;
    expect(allow.textContent).toBe("Allow starting sessions on soundwave");
    allow.click();
    await vi.waitFor(() => expect(calls.grants).toEqual(["soundwave"]));
    expect(machines[0]!.scopes).not.toContain("session-launch");
  });

  it("closing the consent without Allow leaves permission unchanged, including on revisit", async () => {
    const machine: LaunchMachine = { id: "atlas", label: "Atlas", scopes: [...CONTROL] };
    const { sheet: s, calls, dialog } = sheet({ machines: [machine] });
    s.open();
    await flush();
    (dialog().querySelector('.launch-grant [data-launch-action="close"]') as HTMLButtonElement).click();
    expect(calls.grants).toEqual([]);
    expect(machine.scopes).toEqual(CONTROL);
    s.open();
    await flush();
    expect(visibleView(dialog())).toEqual(["grant"]);
    expect(calls.grants).toEqual([]);
    s.close();
  });

  it("offers an invitation after a 403 and clears the refusal on reopen", async () => {
    const { sheet: s, dialog } = sheet({ machines: [{ id: "atlas", label: "Atlas", scopes: CONTROL }], grantError: new SessionLaunchGrantError(403, "Pair with a control invitation.") });
    s.open();
    await flush();
    expect((dialog().querySelector(".launch-grant-invite") as HTMLElement).hidden).toBe(true);
    (dialog().querySelector('[data-launch-action="allow"]') as HTMLButtonElement).click();
    await vi.waitFor(() => expect((dialog().querySelector(".launch-grant-error") as HTMLElement).hidden).toBe(false));
    expect((dialog().querySelector(".launch-grant-invite") as HTMLElement).hidden).toBe(false);
    expect((dialog().querySelector(".launch-grant-command") as HTMLElement).hidden).toBe(false);
    expect((dialog().querySelector('[data-launch-action="allow"]') as HTMLButtonElement).hidden).toBe(true);
    expect(document.activeElement).toBe(dialog().querySelector(".launch-grant-error"));
    s.close();
    s.open();
    await flush();
    expect((dialog().querySelector(".launch-grant-error") as HTMLElement).hidden).toBe(true);
    expect((dialog().querySelector(".launch-grant-invite") as HTMLElement).hidden).toBe(true);
    expect((dialog().querySelector('[data-launch-action="allow"]') as HTMLButtonElement).hidden).toBe(false);
  });

  it("opens on the machine it is asked for, else a machine that can launch", async () => {
    const studio: LaunchMachine = { id: "studio", label: "Studio", scopes: CONTROL };
    const { sheet: s, dialog } = sheet({ machines: [studio, ATLAS] });
    s.open();
    await flush();
    expect((dialog().querySelector("select[name=launch-machine]") as HTMLSelectElement).value).toBe("atlas");
    expect(visibleView(dialog())).toEqual(["form"]);
    s.close();
    s.open("studio");
    await flush();
    expect((dialog().querySelector("select[name=launch-machine]") as HTMLSelectElement).value).toBe("studio");
    expect(visibleView(dialog())).toEqual(["grant"]);
  });

  it("lists projects by recency, offers Attach on a running one, and hides Browse without roots", async () => {
    const { sheet: s, calls, dialog } = sheet({ machines: [ATLAS], catalog: { projects: [project("old", "2026-01-01T00:00:00Z"), project("cas-src", "2026-09-27T00:00:00Z", "patient-pelican-9")], browse_roots: [] } });
    s.open();
    await flush();
    const rows = [...dialog().querySelectorAll(".launch-row .launch-row-name")].map((n) => n.textContent);
    expect(rows).toEqual(["cas-src", "old"]);
    expect((dialog().querySelector("#launch-tab-browse") as HTMLElement).hidden).toBe(true);
    (dialog().querySelector("[data-launch-attach]") as HTMLButtonElement).click();
    expect(calls.opened).toEqual([["atlas", "patient-pelican-9"]]);
    expect(calls.launches).toEqual([]);
    expect(dialog().open).toBe(false);
  });

  it("starts the chosen project with the chosen supervisor and lands once the session is listed", async () => {
    const { sheet: s, calls, dialog } = sheet({ machines: [ATLAS], catalog: { projects: [project("cas-src", "2026-09-27T00:00:00Z")], browse_roots: [] }, listedAfter: 2 });
    s.open();
    await flush();
    const start = dialog().querySelector<HTMLButtonElement>('[data-launch-action="start"]')!;
    expect(start.getAttribute("aria-disabled")).toBe("true");
    const radio = dialog().querySelector<HTMLInputElement>('input[name="launch-project"]')!;
    radio.checked = true;
    radio.dispatchEvent(new Event("change", { bubbles: true }));
    const codex = dialog().querySelector<HTMLInputElement>('input[name="launch-cli"][value="codex"]')!;
    codex.checked = true;
    codex.dispatchEvent(new Event("change", { bubbles: true }));
    (dialog().querySelector('input[name="launch-workers"]') as HTMLInputElement).value = "2";
    expect(dialog().querySelector(".launch-summary")!.textContent).toBe("Start cas-src with Codex on Atlas.");
    expect(start.getAttribute("aria-disabled")).toBe("false");
    start.click();
    await vi.waitFor(() => expect(calls.opened).toEqual([["atlas", "new-otter-1"]]), { timeout: 3000 });
    expect(calls.launches).toEqual([{ machineId: "atlas", request: { target: { kind: "project", id: "p-cas-src" }, supervisor_cli: "codex", workers: 2 } }]);
    expect(dialog().open).toBe(false);
  });

  it("lands straight away when the machine joins an already-running session", async () => {
    const { sheet: s, calls, dialog } = sheet({ machines: [ATLAS], catalog: { projects: [project("cas-src", "2026-09-27T00:00:00Z")], browse_roots: [] }, result: { ok: true, session: "patient-pelican-9", attached: true } });
    s.open();
    await flush();
    const radio = dialog().querySelector<HTMLInputElement>('input[name="launch-project"]')!;
    radio.checked = true;
    radio.dispatchEvent(new Event("change", { bubbles: true }));
    dialog().querySelector<HTMLButtonElement>('[data-launch-action="start"]')!.click();
    await vi.waitFor(() => expect(calls.opened).toEqual([["atlas", "patient-pelican-9"]]));
  });

  it("shows an environment refusal plainly, with the machine's own words, and goes back to the form", async () => {
    const { sheet: s, dialog } = sheet({ machines: [ATLAS], catalog: { projects: [project("cas-src", "2026-09-27T00:00:00Z")], browse_roots: [] }, result: { ok: false, status: 422, code: "not_logged_in", detail: "claude auth status: not logged in (profile main)" } });
    s.open();
    await flush();
    const radio = dialog().querySelector<HTMLInputElement>('input[name="launch-project"]')!;
    radio.checked = true;
    radio.dispatchEvent(new Event("change", { bubbles: true }));
    dialog().querySelector<HTMLButtonElement>('[data-launch-action="start"]')!.click();
    await vi.waitFor(() => expect(visibleView(dialog())).toEqual(["error"]));
    expect(dialog().querySelector(".launch-error-title")!.textContent).toBe("Claude isn't logged in on Atlas.");
    expect(dialog().querySelector(".launch-error-detail pre")!.textContent).toContain("not logged in (profile main)");
    dialog().querySelector<HTMLButtonElement>('[data-launch-action="back"]')!.click();
    expect(visibleView(dialog())).toEqual(["form"]);
  });

  it("turns a scope refusal into the grant path", async () => {
    const { sheet: s, dialog } = sheet({ machines: [ATLAS], catalog: { projects: [project("cas-src", "2026-09-27T00:00:00Z")], browse_roots: [] }, result: { ok: false, status: 403, code: "scope_denied" } });
    s.open();
    await flush();
    const radio = dialog().querySelector<HTMLInputElement>('input[name="launch-project"]')!;
    radio.checked = true;
    radio.dispatchEvent(new Event("change", { bubbles: true }));
    dialog().querySelector<HTMLButtonElement>('[data-launch-action="start"]')!.click();
    await vi.waitFor(() => expect(visibleView(dialog())).toEqual(["grant"]));
  });

  it("refuses an out-of-range worker count before asking the machine", async () => {
    const { sheet: s, calls, dialog } = sheet({ machines: [ATLAS], catalog: { projects: [project("cas-src", "2026-09-27T00:00:00Z")], browse_roots: [] } });
    s.open();
    await flush();
    const radio = dialog().querySelector<HTMLInputElement>('input[name="launch-project"]')!;
    radio.checked = true;
    radio.dispatchEvent(new Event("change", { bubbles: true }));
    (dialog().querySelector('input[name="launch-workers"]') as HTMLInputElement).value = "40";
    dialog().querySelector<HTMLButtonElement>('[data-launch-action="start"]')!.click();
    await flush();
    expect(calls.launches).toEqual([]);
    expect((dialog().querySelector(".launch-invalid") as HTMLElement).hidden).toBe(false);
  });

  it("browses a launch root: folders open, only repositories are selectable", async () => {
    const root = { id: "root-1", name: "code", path: "/home/dev/code" };
    const { sheet: s, calls, dialog } = sheet({
      machines: [ATLAS],
      catalog: { projects: [], browse_roots: [root] },
      listing: (_root, path) => path === ""
        ? { root, path: "", truncated: false, entries: [{ name: "clients", path: "clients", launchable: false, project_id: null, target: null }] }
        : { root, path, truncated: true, entries: [{ name: "acme", path: "clients/acme", launchable: true, project_id: null, target: { kind: "browse", root_id: "root-1", path: "clients/acme" } }] },
    });
    s.open();
    await flush();
    const browseTab = dialog().querySelector<HTMLButtonElement>("#launch-tab-browse")!;
    expect(browseTab.hidden).toBe(false);
    browseTab.click();
    await flush();
    expect(calls.browsed).toEqual([["root-1", ""]]);
    expect(dialog().querySelectorAll('[data-launch-list="browse"] input[type=radio]')).toHaveLength(0);
    dialog().querySelector<HTMLButtonElement>('[data-launch-folder="clients"]')!.click();
    await flush();
    expect(calls.browsed.at(-1)).toEqual(["root-1", "clients"]);
    expect(dialog().querySelector(".launch-crumbs")!.textContent).toContain("clients");
    expect(dialog().querySelector(".launch-truncated")).not.toBeNull();
    const repo = dialog().querySelector<HTMLInputElement>('[data-launch-list="browse"] input[type=radio]')!;
    repo.checked = true;
    repo.dispatchEvent(new Event("change", { bubbles: true }));
    dialog().querySelector<HTMLButtonElement>('[data-launch-action="start"]')!.click();
    await vi.waitFor(() => expect(calls.launches[0]?.request.target).toEqual({ kind: "browse", root_id: "root-1", path: "clients/acme" }));
  });
});

const ACCOUNTS: LaunchProfiles = {
  claude: { installed: true, profiles: [
    { name: "main", logged_in: true, is_default: true },
    { name: "support@petrastella.io", logged_in: true, is_default: false },
    { name: "old@petrastella.io", logged_in: false, is_default: false },
  ] },
  codex: { installed: true, profiles: [], error: "cli_probe_failed" },
  grok: { installed: true, profiles: [] },
};

describe("accounts (cas-9666)", () => {
  it("preselects the logged-in default, else the first logged-in account", () => {
    expect(defaultAccount(ACCOUNTS.claude)).toBe("main");
    expect(defaultAccount({ installed: true, profiles: [{ name: "a", logged_in: false, is_default: true }, { name: "b", logged_in: true, is_default: false }] })).toBe("b");
    expect(defaultAccount({ installed: true, profiles: [{ name: "a", logged_in: false, is_default: true }] })).toBeUndefined();
  });

  it("names the login command, quoting anything unusual", () => {
    expect(accountLoginCommand("claude", "support@petrastella.io")).toBe("cas claude login support@petrastella.io");
    expect(accountLoginCommand("codex", "my team")).toBe("cas codex login 'my team'");
  });

  it("hides the step for Grok and a missing CLI, and falls back to the default when a check fails", () => {
    const ready = { status: "ready", data: { ...ACCOUNTS, codex: { installed: false, profiles: [] } } };
    expect(accountStep("grok", ready, "Atlas").kind).toBe("hidden");
    expect(accountStep("codex", ready, "Atlas").kind).toBe("hidden");
    expect(accountStep("claude", ready, "Atlas").kind).toBe("list");
    const failed = accountStep("codex", { status: "ready", data: ACCOUNTS }, "Atlas");
    expect(failed).toEqual({ kind: "unavailable", message: "Atlas couldn't check Codex's accounts. The session uses the machine's default account." });
    expect(accountStep("claude", { status: "failed" }, "Atlas").kind).toBe("unavailable");
    expect(accountStep("claude", { status: "loading" }, "Atlas").kind).toBe("loading");
  });

  it("says the account and a non-default crew in the summary", () => {
    expect(launchSummary({ verb: "Start", project: "cas-src", cli: "claude", account: "support@petrastella.io", machine: "soundwave", end: "." })).toBe("Start cas-src with Claude (support@petrastella.io) on soundwave.");
    expect(launchSummary({ verb: "Start", project: "cas-src", cli: "codex", workers: 2, machine: "Atlas", end: "." })).toBe("Start cas-src with Codex and 2 workers on Atlas.");
    expect(launchSummary({ verb: "Start", project: "cas-src", cli: "grok", workers: 0, machine: "Atlas", end: "." })).toBe("Start cas-src with Grok on Atlas.");
  });

  it("maps a vanished or logged-out account to plain advice", () => {
    expect(launchErrorCopy({ status: 400, code: "invalid_profile" }, "claude", "Atlas").title).toBe("That Claude account isn't on Atlas any more.");
    const out = launchErrorCopy({ status: 422, code: "not_logged_in" }, "claude", "Atlas", "old@petrastella.io");
    expect(out.title).toBe("The Claude account old@petrastella.io isn't logged in on Atlas.");
    // cas-cee5 (journey F32): the command is its own copyable code, not prose.
    expect(out.advice).toBe("Run this on Atlas, or pick another account, then start again.");
    expect(out.command).toBe("cas claude login old@petrastella.io");
  });

  const pick = (dialog: HTMLDialogElement, selector: string) => {
    const input = dialog.querySelector<HTMLInputElement>(selector)!;
    input.checked = true;
    input.dispatchEvent(new Event("change", { bubbles: true }));
  };

  it("starts on a chosen non-default account and never offers a logged-out one", async () => {
    const { sheet: s, calls, dialog } = sheet({ machines: [ATLAS], profiles: ACCOUNTS, catalog: { projects: [project("cas-src", "2026-09-27T00:00:00Z")], browse_roots: [] } });
    s.open();
    await flush();
    const step = dialog().querySelector<HTMLElement>(".launch-account")!;
    expect(step.hidden).toBe(false);
    expect(dialog().querySelector<HTMLInputElement>('input[name="launch-account"][value="main"]')!.checked).toBe(true);
    expect(dialog().querySelector<HTMLInputElement>('input[name="launch-account"][value="old@petrastella.io"]')!.disabled).toBe(true);
    expect(step.querySelector(".launch-account-out code")!.textContent).toBe("cas claude login old@petrastella.io");
    pick(dialog(), 'input[name="launch-project"]');
    pick(dialog(), 'input[name="launch-account"][value="support@petrastella.io"]');
    expect(dialog().querySelector(".launch-summary")!.textContent).toBe("Start cas-src with Claude (support@petrastella.io) on Atlas.");
    dialog().querySelector<HTMLButtonElement>('[data-launch-action="start"]')!.click();
    await vi.waitFor(() => expect(calls.launches[0]?.request).toEqual({ target: { kind: "project", id: "p-cas-src" }, supervisor_cli: "claude", profile: "support@petrastella.io" }));
  });

  it("hides the step for Grok, shows a failed check with a retry for Codex, and sends no account for either", async () => {
    const { sheet: s, calls, dialog } = sheet({ machines: [ATLAS], profiles: ACCOUNTS, catalog: { projects: [project("cas-src", "2026-09-27T00:00:00Z")], browse_roots: [] } });
    s.open();
    await flush();
    pick(dialog(), 'input[name="launch-project"]');
    pick(dialog(), 'input[name="launch-cli"][value="codex"]');
    expect(dialog().querySelector(".launch-account")!.textContent).toContain("couldn't check Codex's accounts");
    expect(dialog().querySelector('[data-launch-action="retry-accounts"]')).not.toBeNull();
    pick(dialog(), 'input[name="launch-cli"][value="grok"]');
    expect(dialog().querySelector<HTMLElement>(".launch-account")!.hidden).toBe(true);
    dialog().querySelector<HTMLButtonElement>('[data-launch-action="start"]')!.click();
    await vi.waitFor(() => expect(calls.launches[0]?.request).toEqual({ target: { kind: "project", id: "p-cas-src" }, supervisor_cli: "grok" }));
  });

  it("reopens on the machine's defaults, not the last launch's choices (QA F01)", async () => {
    const { sheet: s, dialog } = sheet({ machines: [ATLAS], profiles: ACCOUNTS, catalog: { projects: [project("cas-src", "2026-09-27T00:00:00Z")], browse_roots: [] } });
    s.open();
    await flush();
    pick(dialog(), 'input[name="launch-project"]');
    pick(dialog(), 'input[name="launch-cli"][value="codex"]');
    const workers = dialog().querySelector<HTMLInputElement>('input[name="launch-workers"]')!;
    workers.value = "2";
    workers.dispatchEvent(new Event("input", { bubbles: true }));
    expect(dialog().querySelector(".launch-summary")!.textContent).toBe("Start cas-src with Codex and 2 workers on Atlas.");
    s.close();
    s.open();
    await flush();
    expect(dialog().querySelector<HTMLInputElement>('input[name="launch-cli"][value="claude"]')!.checked).toBe(true);
    expect(dialog().querySelector<HTMLInputElement>('input[name="launch-workers"]')!.value).toBe("");
    expect(dialog().querySelector<HTMLInputElement>('input[name="launch-account"][value="main"]')!.checked).toBe(true);
  });
});

describe("re-pairing and session launch (cas-0e14 F29)", () => {
  it("names a dropped launch permission only when the old pairing had it and the new one does not", () => {
    expect(launchDropped([...CONTROL, "session-launch"], CONTROL)).toBe(true);
    expect(launchDropped([...CONTROL, "session-launch"], [...CONTROL, "session-launch"])).toBe(false);
    expect(launchDropped(CONTROL, CONTROL)).toBe(false);
    expect(launchDropped(undefined, CONTROL)).toBe(false);
  });

  it("warns before a code re-pair that starting sessions must be allowed again, and offers the link command that keeps it as its own code (cas-093d F02)", () => {
    const copy = repairStatus("Atlas · Linux", [...CONTROL, "session-launch"]);
    expect(copy).toContain("Re-pairing Atlas · Linux");
    expect(copy).toContain("starting sessions will need to be allowed again");
    expect(copy).toMatch(/run this on Atlas · Linux and open the link it prints instead:$/);
    expect(copy, "the command is not set as prose").not.toContain("cas hub pair");
    expect(repairCommand([...CONTROL, "session-launch"], "https://hub.example")).toBe("cas hub pair --origin https://hub.example --scopes machine:read,session:read,pane:read,pane:input,message:send,pane:interrupt,session:launch");
    // Without launch there is nothing to lose: the plain wording stays, with no command.
    expect(repairStatus("Atlas · Linux", CONTROL)).toBe("Re-pairing Atlas · Linux: create a new code and approve it on that machine. Its saved access here is replaced when the new credential is installed.");
    expect(repairCommand(CONTROL, "https://hub.example")).toBeUndefined();
  });

  it("says plainly after a re-pair that starting sessions was not kept, and how to get it back", () => {
    expect(launchDroppedNotice("Atlas · Linux")).toEqual({
      headline: "Starting sessions needs allowing again",
      detail: "Re-pairing Atlas · Linux with a code didn't include starting sessions. Open New session to allow it again.",
    });
  });

  it("leads the grant view with the dropped permission", async () => {
    const { sheet: s, dialog } = sheet({ machines: [{ id: "atlas", label: "Atlas", scopes: CONTROL, launchDropped: true }] });
    s.open();
    await flush();
    expect(visibleView(dialog())).toEqual(["grant"]);
    expect(dialog().querySelector(".launch-grant .launch-lead")!.textContent).toBe("Re-pairing Atlas didn't keep starting sessions. Allow it again from this browser.");
  });
});

describe("New session and a machine's connection (cas-0e14 F30)", () => {
  function offlineSheet(machines: LaunchMachine[], current?: string) {
    const loads: string[] = [];
    const host: LaunchHost = {
      machines: () => machines,
      currentMachineId: () => current ?? machines[0]?.id,
      origin: "https://hub.example",
      projects: async (machineId) => { loads.push(machineId); return { projects: [project("cas-src", "2026-09-27T00:00:00Z")], browse_roots: [] }; },
      profiles: async () => ({}),
      browse: async () => { throw new Error("unused"); },
      launch: async () => ({ ok: true, session: "x", attached: false }),
      grant: async () => undefined,
      sessionListed: async () => true,
      open: () => undefined,
      copy: async () => undefined,
    };
    return { sheet: new LaunchSheet(host, document), loads, dialog: () => document.querySelector<HTMLDialogElement>("#launch-dialog")! };
  }
  const options = (dialog: HTMLDialogElement) => [...dialog.querySelectorAll<HTMLOptionElement>("select[name=launch-machine] option")].map((option) => option.textContent);

  it("names each machine's connection state in the picker", async () => {
    const { sheet: s, dialog } = offlineSheet([
      { ...ATLAS, connection: "Reconnecting" },
      { id: "studio", label: "Studio", scopes: [...CONTROL, "session-launch"], connection: "Needs pairing" },
      { id: "forge", label: "Forge", scopes: [...CONTROL, "session-launch"], connection: "Live" },
      { id: "shed", label: "Shed", scopes: CONTROL, connection: "Live" },
    ]);
    s.open();
    await flush();
    expect(options(dialog())).toEqual(["Atlas · reconnecting", "Studio · needs pairing", "Forge", "Shed · can't start sessions yet"]);
  });

  it("opens on a live machine that can launch rather than the current one that is reconnecting", async () => {
    const { sheet: s, dialog, loads } = offlineSheet([
      { ...ATLAS, connection: "Reconnecting" },
      { id: "forge", label: "Forge", scopes: [...CONTROL, "session-launch"], connection: "Live" },
    ], "atlas");
    s.open();
    await flush();
    expect((dialog().querySelector("select[name=launch-machine]") as HTMLSelectElement).value).toBe("forge");
    expect(loads).toEqual(["forge"]);
  });

  it("says a reconnecting machine is reconnecting instead of loading its projects, and loads them once it is back", async () => {
    const machines: LaunchMachine[] = [{ ...ATLAS, connection: "Reconnecting" }];
    const { sheet: s, dialog, loads } = offlineSheet(machines);
    s.open();
    await flush();
    expect(visibleView(dialog())).toEqual(["form"]);
    expect(dialog().querySelector('[data-launch-list="known"]')!.textContent).toBe("Lost connection to Atlas. Reconnecting… Its projects load once it's back.");
    expect(loads).toEqual([]);
    expect(dialog().querySelector('[data-launch-action="start"]')!.getAttribute("aria-disabled")).toBe("true");
    // No "Loading accounts…" that can never finish.
    expect(dialog().querySelector<HTMLElement>(".launch-account")!.hidden).toBe(true);
    machines[0] = { ...machines[0]!, connection: "Live" };
    s.refresh();
    await flush();
    expect(loads).toEqual(["atlas"]);
    expect(dialog().querySelector('[data-launch-list="known"]')!.textContent).toContain("cas-src");
  });

  it("says a machine that needs pairing needs pairing, and never asks it for projects", async () => {
    const { sheet: s, dialog, loads } = offlineSheet([{ ...ATLAS, connection: "Needs pairing" }]);
    s.open();
    await flush();
    expect(dialog().querySelector('[data-launch-list="known"]')!.textContent).toBe("Atlas needs pairing again before it can start sessions.");
    expect(loads).toEqual([]);
  });

  it("swaps the project list for the outage when the machine drops while the sheet is open", async () => {
    const machines: LaunchMachine[] = [{ ...ATLAS, connection: "Live" }];
    const { sheet: s, dialog, loads } = offlineSheet(machines);
    s.open();
    await flush();
    expect(loads).toEqual(["atlas"]);
    machines[0] = { ...machines[0]!, connection: "Reconnecting" };
    s.refresh();
    expect(dialog().querySelector('[data-launch-list="known"]')!.textContent).toBe("Lost connection to Atlas. Reconnecting… Its projects load once it's back.");
    expect(dialog().querySelector('[data-launch-action="start"]')!.getAttribute("aria-disabled")).toBe("true");
    expect(options(dialog())).toEqual(["Atlas · reconnecting"]);
  });

  it("treats an unsteady machine as reachable", async () => {
    const { sheet: s, loads } = offlineSheet([{ ...ATLAS, connection: "Unsteady" }]);
    s.open();
    await flush();
    expect(loads).toEqual(["atlas"]);
  });
});

describe("New session polish (cas-c107)", () => {
  const css = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "styles.css"), "utf8");
  const rule = (selector: string) => {
    const at = css.indexOf(`${selector} {`);
    return at < 0 ? "" : css.slice(at, css.indexOf("}", at) + 1);
  };

  it("gives the supervisor's and the account's \"Default\" one treatment: a plain sub-label, not a pill", () => {
    const shared = /\.launch-cli-default,\s*\.launch-account-tag \{([^}]*)\}/.exec(css)?.[1] ?? "";
    expect(shared).toContain("color: var(--text-mid)");
    expect(shared).toContain("font-size: var(--fs-xs)");
    expect(shared).toContain("font-weight: var(--weight-regular)");
    // The shared rule is the account tag's only styling: no pill fill or radius elsewhere.
    expect(css.match(/\.launch-account-tag\b/g)).toHaveLength(1);
  });

  it("bounds the logged-out row's Copy button, and keeps the boundary in forced colors", () => {
    expect(rule(".launch-login button")).toContain("border: var(--line-width) solid var(--color-transparent)");
    const forced = [...css.matchAll(/@media \(forced-colors: active\) \{([\s\S]*?)\n\}/g)].map((match) => match[1]).join("\n");
    expect(forced).toContain(".launch-login button { border-color: ButtonBorder; }");
  });
});

describe("New session sheet copy (cas-cee5, journey F31-F34)", () => {
  it("renders a command as whole tokens: breaks only at spaces and after a scope's comma", () => {
    const code = document.createElement("code");
    code.innerHTML = commandTokensMarkup("cas hub pair --origin https://hub.example --scopes machine:read,pane:input,session:launch");
    const pieces = [...code.querySelectorAll(".launch-command-piece")].map((piece) => piece.textContent);
    expect(pieces).toEqual(["cas", "hub", "pair", "--origin", "https://hub.example", "--scopes", "machine:read,", "pane:input,", "session:launch"]);
    expect(code.textContent).toBe("cas hub pair --origin https://hub.example --scopes machine:read,pane:input,session:launch");
    expect(code.querySelectorAll("wbr")).toHaveLength(2);
    const css = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "styles.css"), "utf8");
    expect(css).toContain(".launch-command-piece { white-space: nowrap; }");
    expect(css).not.toMatch(/\.launch-grant-command code \{[^}]*overflow-wrap: anywhere/);
  });

  it("explains the Supervisor and Workers fields in words that agree with the field", () => {
    const dialog = document.createElement("div");
    dialog.innerHTML = launchSheetMarkup();
    expect(dialog.querySelector("#launch-cli-hint")!.textContent).toBe("Which assistant runs the supervisor.");
    const workers = dialog.querySelector<HTMLInputElement>("input[name=launch-workers]")!;
    expect(workers.placeholder).toBe("None");
    expect(dialog.querySelector("#launch-workers-hint")!.textContent).toBe("Up to 16. None starts the supervisor alone.");
    // The grant view says once what opening the link does.
    expect(dialog.querySelector(".launch-grant-invite")!.textContent).not.toContain("kept");
    expect(dialog.querySelector(".launch-grant-note")!.textContent).toBe("The link re-pairs this browser with the machine: what it can do now is kept, and starting sessions is added.");
  });
});
