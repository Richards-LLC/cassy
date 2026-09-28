// @vitest-environment jsdom
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import {
  LaunchSheet, canLaunch, defaultSupervisorCli, filterProjects, launchErrorCopy, parseWorkers, sortProjects,
  type BrowseListing, type LaunchHost, type LaunchMachine, type LaunchProject, type LaunchRequest, type LaunchResult, type ProjectCatalog,
} from "./launch-session";
import { launchGrantCommand, parseGrantedScopes, scopeChoices, scopeSummary } from "./pairing-scopes";
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

  it("defaults the supervisor to the machine's own CLI, else Claude", () => {
    expect(defaultSupervisorCli({ defaultCli: "codex" })).toBe("codex");
    expect(defaultSupervisorCli({ defaultCli: "vim" })).toBe("claude");
    expect(defaultSupervisorCli(undefined)).toBe("claude");
  });

  it("allows launch only with the session-launch scope", () => {
    expect(canLaunch({ scopes: CONTROL })).toBe(false);
    expect(canLaunch({ scopes: [...CONTROL, "session-launch"] })).toBe(true);
    expect(canLaunch(undefined)).toBe(false);
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

type Calls = { launches: Array<{ machineId: string; request: LaunchRequest }>; opened: Array<[string, string]>; browsed: Array<[string, string]> };

function sheet(options: {
  machines: LaunchMachine[];
  catalog?: ProjectCatalog;
  listing?: (root: string, path: string) => BrowseListing;
  result?: LaunchResult;
  listedAfter?: number;
}): { sheet: LaunchSheet; calls: Calls; dialog: () => HTMLDialogElement } {
  const calls: Calls = { launches: [], opened: [], browsed: [] };
  let polls = 0;
  const host: LaunchHost = {
    machines: () => options.machines,
    currentMachineId: () => options.machines[0]?.id,
    origin: "https://hub.example",
    projects: async () => options.catalog ?? { projects: [], browse_roots: [] },
    browse: async (_machine, root, path) => { calls.browsed.push([root, path]); return options.listing!(root, path); },
    launch: async (machineId, request) => { calls.launches.push({ machineId, request }); return options.result ?? { ok: true, session: "new-otter-1", attached: false }; },
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
  it("replaces the form with the grant path when the machine lacks the scope", async () => {
    const { sheet: s, dialog } = sheet({ machines: [{ id: "atlas", label: "Atlas", scopes: CONTROL }] });
    s.open();
    await flush();
    expect(dialog().open).toBe(true);
    expect(visibleView(dialog())).toEqual(["grant"]);
    expect(dialog().querySelector(".launch-grant")!.textContent).toContain("can't start sessions on Atlas yet");
    expect(dialog().querySelector(".launch-grant-command code")!.textContent).toContain("session:launch");
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
