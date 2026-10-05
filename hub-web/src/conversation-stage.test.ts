// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { beforeEach, describe, expect, it } from "vitest";
import { ConversationHistory } from "./conversation-history";
import { ConversationList, type ConversationRow } from "./conversation-list";
import { arrangeConversationShell, conversationAttentionBadge, createPaneHost, ensureConversationStage, rawOutputDrawerMarkup } from "./conversation-shell";
import { ConversationView } from "./conversation-view";
import { contextSections, syncContextRail, waitingOnOperator } from "./context-rail";
import { createAttentionItem, machineEventAttention } from "./attention";
import { renderAttentionPanel } from "./attention-view";
import type { OperatorReply, OperatorTurnKind } from "./types";

const styles = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "styles.css"), "utf8");

/** The open conversation's shell with an empty grid in its pane slot, as render() builds it. */
function openShell(): HTMLElement {
  const grid = document.createElement("section");
  grid.id = "pane-grid";
  grid.className = "pane-grid";
  grid.innerHTML = '<div class="empty"></div>';
  document.body.replaceChildren(arrangeConversationShell(document, { selected: true, supervisor: "patient-pelican-9", projectDir: "/projects/cas-src", host: "Atlas · Linux", machineId: "atlas-linux", loaded: true, paired: true }, { grid }));
  return grid;
}

/** A pane card with what the Ghostty surface mounts: a canvas, its input and scrollbar. */
function mountSurface(host: HTMLElement): HTMLElement {
  host.innerHTML = '<section class="pane" data-pane-id="supervisor"><div class="terminal-mount"><canvas class="t3-ghostty-canvas"></canvas><textarea class="t3-ghostty-input" aria-label="Terminal input"></textarea><div class="t3-ghostty-scrollbar" role="scrollbar" tabindex="0"></div></div></section>';
  return host.querySelector<HTMLElement>(".terminal-mount")!;
}

describe("the hidden pane host renders nothing (cas-0546 condition 1)", () => {
  beforeEach(() => { document.head.replaceChildren(); document.body.replaceChildren(); });

  it("is hidden, inert and out of the accessibility tree from the moment it is made", () => {
    const host = createPaneHost(document);
    expect(host.hidden).toBe(true);
    expect(host.hasAttribute("inert")).toBe(true);
    expect(host.getAttribute("aria-hidden")).toBe("true");
  });

  it("keeps the thread in its own visible slot, never inside or behind the host", () => {
    const grid = openShell();
    const { slot, host } = ensureConversationStage(grid);
    mountSurface(host);
    const view = new ConversationView(document, new ConversationHistory(), { supervisor: "patient-pelican-9", project: "cas-src", header: false });
    slot.append(view.element);
    view.update();

    // The thread and the host are siblings in the grid: neither contains the other.
    expect(slot.parentElement).toBe(grid);
    expect(host.parentElement).toBe(grid);
    expect(host.contains(view.element)).toBe(false);
    expect(view.element.contains(host)).toBe(false);
    expect(slot.contains(view.element)).toBe(true);
    // Nothing hidden, inert or aria-hidden sits above the thread.
    expect(view.element.closest("[hidden], [inert], [aria-hidden='true']")).toBeNull();
    // The surface's own controls are unreachable: inert, hidden, not in the tree.
    for (const control of host.querySelectorAll<HTMLElement>("textarea, [tabindex]")) {
      expect(control.closest("[inert]")).toBe(host);
      expect(control.closest("[aria-hidden='true']")).toBe(host);
    }
    // A heartbeat re-run keeps the same nodes: the thread is never remounted.
    expect(ensureConversationStage(grid)).toEqual({ slot, host });
    expect(slot.contains(view.element)).toBe(true);
  });

  it("is display:none in the stylesheet, even if its hidden attribute were lost", () => {
    const rule = /\.pane-host,\s*\.pane-host\[hidden\]\s*\{[^}]*\}/.exec(styles)?.[0];
    expect(rule).toBeDefined();
    expect(rule).toMatch(/display:\s*none\s*!important/);
    // !important, so no later rule (a phone block, a print block) can lay it
    // out; jsdom's cascade does not model !important, so that half is the
    // regex above.
    const style = document.createElement("style");
    style.textContent = rule!;
    document.head.append(style);
    const grid = openShell();
    const { slot, host } = ensureConversationStage(grid);
    expect(getComputedStyle(host).display).toBe("none");
    host.hidden = false;
    expect(getComputedStyle(host).display).toBe("none");
    // Every other place the stylesheet names the host keeps it hidden.
    for (const match of styles.matchAll(/([^{}]*\.pane-host[^{}]*)\{([^}]*)\}/g)) {
      expect(match[2], match[1]).not.toMatch(/display:\s*(?!none\b)[a-z]/);
    }
    expect(getComputedStyle(slot).display).not.toBe("none");
  });

  it("has no Terminal markup left around the thread", () => {
    const grid = openShell();
    ensureConversationStage(grid);
    const shell = document.querySelector<HTMLElement>(".conversation-shell")!;
    expect(shell.querySelector(".shell, .machine-rail, .session-header, #talk-supervisor, #conversation-terminal, #conversation-return, .mode-badge, #lease, .primary-pane-slot, .pane-header")).toBeNull();
    // The thread's slot is visible; the panes exist only inside the host.
    expect(shell.querySelectorAll(".pane-host")).toHaveLength(1);
    expect([...shell.querySelectorAll(".pane")].every((pane) => pane.closest(".pane-host") !== null)).toBe(true);
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
