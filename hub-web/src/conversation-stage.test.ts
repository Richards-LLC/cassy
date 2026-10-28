// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { beforeEach, describe, expect, it } from "vitest";
import { ConversationHistory } from "./conversation-history";
import { ConversationList, type ConversationRow } from "./conversation-list";
import { arrangeConversationShell, conversationAttentionBadge, ensureConversationStage, rawOutputDrawerMarkup } from "./conversation-shell";
import { ConversationView } from "./conversation-view";
import { contextSections, syncContextRail, waitingOnOperator } from "./context-rail";
import { createAttentionItem, machineEventAttention } from "./attention";
import { renderAttentionPanel } from "./attention-view";
import type { OperatorReply, OperatorTurnKind } from "./types";

const styles = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "styles.css"), "utf8");

/** The open conversation's shell with an empty grid in its pane slot, as render() builds it. */
function openShell(): HTMLElement {
  const grid = document.createElement("section");
  grid.id = "conversation-stage";
  grid.className = "conversation-stage";
  grid.innerHTML = '<div class="empty"></div>';
  document.body.replaceChildren(arrangeConversationShell(document, { selected: true, supervisor: "patient-pelican-9", projectDir: "/projects/cas-src", host: "Atlas · Linux", machineId: "atlas-linux", loaded: true, paired: true }, { grid }));
  return grid;
}

describe("the thread mounts straight into the stage (cas-f285)", () => {
  beforeEach(() => { document.head.replaceChildren(); document.body.replaceChildren(); });

  it("puts the thread in the stage's own visible slot, with no pane host or terminal beside it", () => {
    const grid = openShell();
    const slot = ensureConversationStage(grid);
    const view = new ConversationView(document, new ConversationHistory(), { supervisor: "patient-pelican-9", project: "cas-src", header: false });
    slot.append(view.element);
    view.update();

    expect(slot.parentElement).toBe(grid);
    expect([...grid.children]).toEqual([slot]);
    expect(slot.contains(view.element)).toBe(true);
    // Nothing hidden, inert or aria-hidden sits above the thread.
    expect(view.element.closest("[hidden], [inert], [aria-hidden='true']")).toBeNull();
    // No pane card, terminal mount or canvas anywhere in the shell.
    expect(document.querySelector(".pane-host, .pane, .terminal-mount, canvas")).toBeNull();
    // A heartbeat re-run keeps the same node: the thread is never remounted.
    expect(ensureConversationStage(grid)).toBe(slot);
    expect(slot.contains(view.element)).toBe(true);
  });

  it("keeps no stylesheet rule for a pane host", () => {
    expect(styles).not.toMatch(/\.pane-host\b/);
    expect(getComputedStyle(ensureConversationStage(openShell())).display).not.toBe("none");
  });

  it("has no Terminal markup left around the thread", () => {
    const grid = openShell();
    ensureConversationStage(grid);
    const shell = document.querySelector<HTMLElement>(".conversation-shell")!;
    expect(shell.querySelector(".shell, .machine-rail, .session-header, #talk-supervisor, #conversation-terminal, #conversation-return, .mode-badge, #lease, .primary-pane-slot, .pane-header, #pane-grid")).toBeNull();
  });
});

describe("the Raw output drawer markup (cas-0546)", () => {
  it("is a titled, read-only drawer with a labelled close", () => {
    const dialog = document.createElement("dialog");
    dialog.innerHTML = rawOutputDrawerMarkup();
    expect(dialog.querySelector("h2#raw-output-title")?.textContent).toBe("Raw output");
    expect(dialog.querySelector(".raw-output-close")?.getAttribute("aria-label")).toBe("Close raw output");
    expect(dialog.querySelector("textarea, input, [contenteditable]")).toBeNull();
    expect(dialog.querySelector(".raw-output-empty")?.getAttribute("role")).toBe("status");
  });
});

const at = (hh: number, mm: number) => new Date(2026, 9, 5, hh, mm).getTime();
function said(id: number, kind: OperatorTurnKind, message: string): OperatorReply {
  return { notification_id: id, reply_to: null, message, summary: "", device_id: "d", kind };
}

describe("only real asks and blockers wait on the operator (cas-0546 condition 3)", () => {
  beforeEach(() => { document.body.replaceChildren(); });

  /** The events the fleet-wide Attention feed used to surface. */
  function lifecycleAttention() {
    const base = { machineId: "atlas-linux", machineLabel: "Atlas · Linux", session: "cas-src-patient-pelican-9", createdAt: "2026-10-05T09:00:00Z" };
    return [
      createAttentionItem({ ...base, id: "merge", kind: "awaiting_merge" }, machineEventAttention("awaiting_merge", {})),
      createAttentionItem({ ...base, id: "daemon", kind: "daemon_disconnected" }, machineEventAttention("daemon_disconnected", {})),
      createAttentionItem({ ...base, id: "worker", kind: "pane_exited" }, machineEventAttention("pane_exited", {})),
      createAttentionItem({ ...base, id: "ended", kind: "session_removed" }, machineEventAttention("session_removed", {})),
    ];
  }

  it("never counts awaiting-merge or lifecycle events as waiting on the operator", () => {
    const history = new ConversationHistory();
    // A supervisor status about a merge is a turn to read, not a question.
    history.reply(said(1, "status", "cas-1999 is awaiting merge; the daemon restarted and a worker stopped."), at(9, 0));
    const attention = lifecycleAttention();
    expect(waitingOnOperator(history)).toBe(0);
    expect(waitingOnOperator(undefined)).toBe(0);
    // The rail shows them as attention, and keeps "Waiting on you" closed.
    expect(contextSections({ history, progress: true, attention: attention.length })).toEqual(["progress", "attention"]);

    document.body.append(arrangeConversationShell(document, { selected: true, supervisor: "patient-pelican-9", projectDir: "/projects/cas-src", host: "Atlas · Linux", machineId: "atlas-linux", loaded: true, paired: true }, { attention: Object.assign(document.createElement("section"), { id: "attention-panel" }) }));
    renderAttentionPanel(document.querySelector<HTMLElement>("#attention-panel")!, attention, { dismiss: () => undefined, act: () => undefined, copy: () => undefined });
    syncContextRail(document, { history, progress: false, attention: attention.length });
    const waitingSection = document.querySelector<HTMLElement>('[data-section="waiting"]')!;
    expect(waitingSection.hidden).toBe(true);
    expect(waitingSection.querySelectorAll("li")).toHaveLength(0);
    expect(document.querySelector<HTMLElement>('[data-section="attention"]')!.hidden).toBe(false);
    // Nothing visible or spoken calls them "needs you" or "waiting on you".
    const rail = document.querySelector<HTMLElement>(".conversation-context")!;
    const visible = [...rail.querySelectorAll<HTMLElement>("[data-section]")].filter((section) => !section.hidden).map((section) => section.textContent ?? "").join(" ");
    expect(visible).not.toMatch(/needs you|waiting on you|waiting for you/i);
    expect(conversationAttentionBadge(attention.length).label).not.toMatch(/needs you|waiting/i);
  });

  it("gives the list row no waiting mark for them, and one for a real ask", () => {
    const history = new ConversationHistory();
    history.reply(said(1, "status", "cas-1999 is awaiting merge."), at(9, 0));
    const row = (attention: number): ConversationRow => ({ key: "atlas-linux:cas-src", machineId: "atlas-linux", session: "cas-src", supervisor: "patient-pelican-9", projectDir: "/projects/cas-src", host: "Atlas · Linux", freshness: "", connection: "Live", attention, selected: false });
    const list = document.createElement("nav");
    document.body.append(list);
    new ConversationList().render(list, [row(waitingOnOperator(history))], () => undefined, async () => undefined);
    const quiet = list.querySelector<HTMLElement>(".conversation-row")!;
    expect(quiet.dataset.waiting).toBe("false");
    expect(list.querySelector(".conversation-flag")).toBeNull();
    expect(list.textContent).not.toMatch(/waiting for you/i);

    history.reply(said(2, "ask", "Ship it to staging?"), at(9, 5));
    history.reply(said(3, "blocker", "CI is red on main."), at(9, 6));
    expect(waitingOnOperator(history)).toBe(2);
    new ConversationList().render(list, [row(waitingOnOperator(history))], () => undefined, async () => undefined);
    expect(list.querySelector<HTMLElement>(".conversation-row")!.dataset.waiting).toBe("true");
    expect(list.querySelector(".conversation-flag")?.getAttribute("aria-label")).toBe("2 waiting for you");
  });
});

it("ends the thread, row and composer together after the supervisor exits, naming surviving workers", () => {
  const ended = {
    at: new Date(2026, 9, 28, 11, 24).toISOString(),
    reason: "prompt_input_exit",
    workers_stopped: ["worker-a", "worker-b"],
    orphans: ["worker-c"],
  };
  const composer = document.createElement("section");
  composer.innerHTML = '<textarea id="message-text"></textarea><button id="message-send">Send</button><p id="message-status"></p>';
  const grid = document.createElement("section");
  const model = { selected: true, supervisor: "patient-pelican-9", projectDir: "/projects/cas-src", host: "Atlas · Linux", machineId: "atlas", loaded: true, paired: true, launch: "ready" as const, ended };
  const shell = arrangeConversationShell(document, model, { grid, composer });
  const options = { supervisor: "patient-pelican-9", header: false, sessionEnd: () => ended };
  const view = new ConversationView(document, new ConversationHistory(), options);
  ensureConversationStage(grid).append(view.element);
  document.body.replaceChildren(shell);
  view.update();

  expect(shell.querySelector("#conversation-connection")?.textContent).toContain("Ended");
  expect(view.element.textContent).toContain("Session ended");
  expect(view.element.textContent).toContain("/exit");
  expect(view.element.textContent).toMatch(/2 workers stopped at 11:24\s*AM/);
  expect(view.element.textContent).toContain("worker-c");
  expect(composer.querySelector<HTMLButtonElement>("#message-send")!.disabled).toBe(true);
  expect(composer.textContent).toMatch(/session ended.*New session/is);

  const container = document.createElement("div");
  shell.append(container);
  const row = { key: "atlas:s", machineId: "atlas", session: "s", supervisor: "patient-pelican-9", projectDir: "/projects/cas-src", host: "Atlas · Linux", freshness: "just now", connection: "Live", preview: "Working", attention: 0, selected: true, ended };
  new ConversationList().render(container, [row], () => {});
  expect(container.textContent).toContain("Ended");
  expect(container.textContent).not.toContain("Working");
});
