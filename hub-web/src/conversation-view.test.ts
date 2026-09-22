// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { ConversationHistory } from "./conversation-history";
import { ConversationView, registerTurnRenderer } from "./conversation-view";
import type { OperatorReply, OperatorTurnKind } from "./types";
import ROW_20812 from "./fixtures/hub-row-20812.txt?raw";

const at = (hh: number, mm: number) => new Date(2026, 8, 21, hh, mm).getTime();
function reply(id: number, kind: OperatorTurnKind, message = `m${id}`, reply_to: number | null = null): OperatorReply {
  return { notification_id: id, reply_to, message, summary: "", device_id: "d", kind };
}

describe("ConversationView (Pebble thread)", () => {
  it("paints operator pebbles right, supervisor pebbles in the accent scope, grouped corners and one time per group", () => {
    const history = new ConversationHistory();
    const view = new ConversationView(document, history, { supervisor: "atlas-sup", machine: "Atlas", project: "cas-src", accentClass: "machine-accent-0" });
    document.body.replaceChildren(view.element);
    history.submit("a", "atlas-sup", "Merge the lanes.", at(9, 41));
    history.acknowledge({ client_ref: "a", notification_id: 41, target: "atlas-sup", stamped: true });
    history.reply(reply(1, "answer", "On it.", 41), at(9, 44));
    history.reply(reply(2, "receipt", "Both lanes are on the epic branch.", 41), at(9, 47));
    view.update();
    expect(view.element.classList.contains("machine-accent-0")).toBe(true);
    const groups = view.element.querySelectorAll(".msgs > .turn");
    expect(groups).toHaveLength(2);
    expect(groups[0]?.className).toBe("turn you");
    expect(groups[0]?.querySelector<HTMLElement>(".bub")?.dataset.state).toBe("replied");
    expect(groups[0]?.querySelector("time")?.textContent).toBe("09:41");
    const sup = groups[1]!;
    const bubbles = sup.querySelectorAll(".bub");
    expect(bubbles[0]?.classList.contains("group-first")).toBe(true); expect(bubbles[0]?.classList.contains("group-last")).toBe(false);
    expect(bubbles[1]?.classList.contains("group-first")).toBe(false); expect(bubbles[1]?.classList.contains("group-last")).toBe(true);
    expect(bubbles[1]?.classList.contains("receipt")).toBe(true); expect(bubbles[1]?.querySelector(".tick")).not.toBeNull();
    expect(sup.querySelectorAll("time")).toHaveLength(1); expect(sup.querySelector("time")?.textContent).toBe("09:47");
  });
  it("re-derives grouping on incremental updates so a later turn tightens the earlier outer corner", () => {
    const history = new ConversationHistory();
    const view = new ConversationView(document, history, "sup"); document.body.replaceChildren(view.element);
    history.reply(reply(1, "answer"), at(9, 44)); view.update();
    const first = view.element.querySelector(".bub")!;
    expect(first.classList.contains("group-last")).toBe(true);
    history.reply(reply(2, "answer"), at(9, 45)); view.update();
    expect(view.element.querySelector(".bub")).toBe(first);
    expect(first.classList.contains("group-first")).toBe(true); expect(first.classList.contains("group-last")).toBe(false);
    expect(view.element.querySelectorAll(".msgs > .turn")).toHaveLength(1);
  });
  it("shows session dividers and the no-earlier-history marker", () => {
    const history = new ConversationHistory();
    const view = new ConversationView(document, history, {
      supervisor: "sup",
      historyEnd: () => true,
    });
    document.body.replaceChildren(view.element);
    history.submit("a", "sup", "Older", at(9, 0), undefined, "factory-old");
    history.reply(reply(1, "answer", "Reply"), at(9, 1), "factory-old");
    view.update();
    expect(view.element.querySelector(".history-end")?.textContent).toBe("No earlier history");
    expect(view.element.querySelector(".session-divider")?.textContent).toBe("session factory-old started 09:00");
  });
  it("shows the empty state instead of the history marker for an empty loaded page", () => {
    const history = new ConversationHistory();
    const view = new ConversationView(document, history, { supervisor: "sup", historyEnd: () => true, working: () => true });
    document.body.replaceChildren(view.element);
    view.update();

    expect(view.element.querySelector<HTMLElement>(".empty")?.hidden).toBe(false);
    expect(view.element.querySelector(".said")?.textContent).toBe("Nothing waiting on you. sup will write here when it needs a decision.");
    expect(view.element.querySelector(".history-end")).toBeNull();
    expect(view.element.querySelector(".working")).toBeNull();
  });
  it("coalesces status runs into one quiet line and shows the working indicator while executing", () => {
    const history = new ConversationHistory();
    let working = false;
    const view = new ConversationView(document, history, { supervisor: "sup", working: () => working }); document.body.replaceChildren(view.element);
    history.reply(reply(1, "status", "Gate started"), at(9, 49)); history.reply(reply(2, "status", "gate 11 of 14"), at(9, 55)); view.update();
    const line = view.element.querySelector<HTMLElement>(".coalesce");
    expect(view.element.querySelectorAll(".coalesce")).toHaveLength(1);
    expect(line?.textContent).toBe("1 more update · gate 11 of 14"); expect(line?.dataset.count).toBe("2");
    expect(view.element.querySelector(".working")).toBeNull();
    working = true; view.refreshWorking();
    expect(view.element.querySelector(".msgs > :last-child .working")?.textContent).toContain("working");
    working = false; view.refreshWorking();
    expect(view.element.querySelector(".working")).toBeNull();
  });
  it("renders the operator's real row 20812 as a full supervisor bubble, not the quiet status line", () => {
    const history = new ConversationHistory();
    const view = new ConversationView(document, history, "sup"); document.body.replaceChildren(view.element);
    history.reply(reply(20811, "status", "Gate 11 of 14 targets green"), at(13, 20));
    history.reply(reply(20812, "status", ROW_20812.trim()), at(13, 24)); view.update();
    const bubble = view.element.querySelector<HTMLElement>('.bub[data-kind="status"]');
    expect(bubble).not.toBeNull();
    expect(bubble!.closest(".turn")?.className).toBe("turn sup");
    // The operator's asks keep their shape: the lead-in is the bubble's heading and (1)–(3) are a real list.
    expect(bubble!.querySelector(".markdown-heading")?.textContent).toBe("Status 13:2xZ. WAITING ON YOU:");
    expect([...bubble!.querySelectorAll("ol.markdown-list > li")].map((li) => li.textContent?.slice(0, 13))).toEqual(["mockup shape ", '"post";', '"run it" — th']);
    expect(bubble!.textContent).toContain("IN FLIGHT: recall re-check, dev-pages cleanup PR. Nothing posted.");
    expect(bubble!.closest(".coalesce")).toBeNull();
    const quiet = view.element.querySelectorAll<HTMLElement>(".coalesce");
    expect(quiet).toHaveLength(1);
    expect(quiet[0]!.textContent).toBe("Gate 11 of 14 targets green");
    expect(quiet[0]!.textContent).not.toContain("WAITING ON YOU");
  });
  it("offers Show full update on a folded run, opens every update, keeps it open across repaints, and closes again", () => {
    const history = new ConversationHistory();
    const view = new ConversationView(document, history, "sup"); document.body.replaceChildren(view.element);
    history.reply(reply(1, "status", "Gate started"), at(9, 49)); history.reply(reply(2, "status", "gate 11 of 14"), at(9, 55)); view.update();
    const line = view.element.querySelector<HTMLElement>(".coalesce")!;
    const more = view.element.querySelector<HTMLButtonElement>(".coalesce-expand")!;
    expect(more.hidden).toBe(false);
    expect(more.textContent).toBe("Show full update");
    expect(more.getAttribute("aria-expanded")).toBe("false");
    expect(more.getAttribute("aria-controls")).toBe(line.id);
    expect(line.dataset.expanded).toBe("false");
    more.click();
    const open = view.element.querySelector<HTMLElement>(".coalesce")!;
    expect(open.dataset.expanded).toBe("true");
    expect([...open.querySelectorAll("p")].map((p) => p.textContent)).toEqual(["Gate started", "gate 11 of 14"]);
    const less = view.element.querySelector<HTMLButtonElement>(".coalesce-expand")!;
    expect(less.textContent).toBe("Show less"); expect(less.getAttribute("aria-expanded")).toBe("true");
    expect(document.activeElement).toBe(less);
    // A later status joins the open run and it stays open.
    history.reply(reply(3, "status", "gate 12 of 14"), at(9, 57)); view.update();
    expect([...view.element.querySelectorAll(".coalesce p")].map((p) => p.textContent)).toEqual(["Gate started", "gate 11 of 14", "gate 12 of 14"]);
    view.element.querySelector<HTMLButtonElement>(".coalesce-expand")!.click();
    const closed = view.element.querySelector<HTMLElement>(".coalesce")!;
    expect(closed.dataset.expanded).toBe("false");
    expect(closed.textContent).toBe("2 more updates · gate 12 of 14");
  });
  it("hides Show full update on a lone short status that nothing clamps", () => {
    const history = new ConversationHistory();
    const view = new ConversationView(document, history, "sup"); document.body.replaceChildren(view.element);
    history.reply(reply(1, "status", "Rebasing"), at(9, 49)); view.update();
    expect(view.element.querySelector<HTMLButtonElement>(".coalesce-expand")?.hidden).toBe(true);
  });
  it("keeps Loading earlier readable: aria-busy on the thread and the working mark while a page loads", () => {
    const history = new ConversationHistory();
    let loading = false;
    const loadEarlier = vi.fn();
    const view = new ConversationView(document, history, { supervisor: "sup", hasEarlier: () => true, loadingEarlier: () => loading, loadEarlier });
    document.body.replaceChildren(view.element);
    history.reply(reply(20812, "status", ROW_20812.trim()), at(13, 24)); view.update();
    const button = view.element.querySelector<HTMLButtonElement>(".conversation-load-earlier")!;
    expect(button.hidden).toBe(false); expect(button.disabled).toBe(false);
    expect(button.textContent).toBe("Load earlier"); expect(button.querySelector(".dots")).toBeNull();
    expect(view.element.hasAttribute("aria-busy")).toBe(false);
    button.click(); expect(loadEarlier).toHaveBeenCalledTimes(1);
    loading = true; view.update();
    expect(view.element.getAttribute("aria-busy")).toBe("true");
    expect(button.disabled).toBe(true); expect(button.dataset.loading).toBe("true");
    expect(button.textContent).toBe("Loading earlier…");
    const dots = button.querySelector<HTMLElement>(".dots");
    expect(dots?.getAttribute("aria-hidden")).toBe("true"); expect(dots?.querySelectorAll("i")).toHaveLength(3);
    loading = false; view.update();
    expect(view.element.hasAttribute("aria-busy")).toBe(false);
    expect(button.disabled).toBe(false); expect(button.textContent).toBe("Load earlier"); expect(button.querySelector(".dots")).toBeNull();
  });
  it("falls back to data-kind bubbles for ask and blocker when nothing is registered", () => {
    const history = new ConversationHistory();
    const view = new ConversationView(document, history, "sup"); document.body.replaceChildren(view.element);
    history.reply(reply(1, "ask", "Fix in-train or ship?"), at(9, 58)); history.reply(reply(2, "blocker", "Gate red."), at(9, 59)); view.update();
    const ask = view.element.querySelector<HTMLElement>('[data-kind="ask"]')!;
    expect(ask.classList.contains("bub")).toBe(true); expect(ask.textContent).toBe("Fix in-train or ship?");
    expect(view.element.querySelector<HTMLElement>('[data-kind="blocker"]')?.classList.contains("bub")).toBe(true);
  });
  it("renders a markdown table as an evidence table with toned cells", () => {
    const history = new ConversationHistory();
    const view = new ConversationView(document, history, "sup"); document.body.replaceChildren(view.element);
    history.reply(reply(1, "answer", "Every pack:\n\n| pack | cases | result |\n| --- | --- | --- |\n| core | 412 | pass |\n| cli | 318 | 1 flake |"), at(9, 30)); view.update();
    const table = view.element.querySelector<HTMLElement>(".evi")!;
    expect(table.dataset.columns).toBe("3");
    expect(table.querySelectorAll(".evi-row")).toHaveLength(3);
    expect(table.querySelector(".evi-head span")?.textContent).toBe("pack");
    expect(table.querySelector(".pass")?.textContent).toBe("pass"); expect(table.querySelector(".flake")?.textContent).toBe("1 flake");
    expect(view.element.querySelector(".bub p")?.textContent).toBe("Every pack:");
  });
  it("renders the same markdown body for hydrated replies and pinned asks", () => {
    const unregister = registerTurnRenderer("ask", (_reply, context) => {
      const object = context.document.createElement("div"); object.className = "obj"; object.append(...context.body()); return object;
    });
    try {
      const history = new ConversationHistory();
      history.hydrateReply({ notification_id: 7, reply_to: null, message: "**Hydrated**\n\n- history", summary: "", device_id: "d", at: "2026-09-21T09:30:00Z" });
      history.reply(reply(8, "ask", "**Pinned**\n\n- choose"), at(9, 31));
      const view = new ConversationView(document, history, "sup"); document.body.replaceChildren(view.element, view.pinned); view.update();
      expect(view.element.querySelector('.turn.sup strong')?.textContent).toBe("Hydrated");
      expect(view.element.querySelector('.turn.sup .markdown-list li')?.textContent).toBe("history");
      expect(view.pinned.querySelector('strong')?.textContent).toBe("Pinned");
      expect(view.pinned.querySelector('.markdown-list li')?.textContent).toBe("choose");
    } finally { unregister(); }
  });
  it("shows sending and refused states on the operator pebble with an edit affordance", () => {
    const history = new ConversationHistory();
    const edit = vi.fn();
    const view = new ConversationView(document, history, { supervisor: "sup", editMessage: edit }); document.body.replaceChildren(view.element);
    history.submit("x", "sup", "Ship it", at(9, 0)); view.update();
    expect(view.element.querySelector('.conversation-turn[data-state="sending"] .conversation-delivery')?.textContent).toBe("Sending…");
    history.reject("x", "no access"); view.update();
    const refused = view.element.querySelector<HTMLElement>('.conversation-turn[data-state="error"]')!;
    expect(refused.querySelector(".conversation-delivery")?.textContent).toBe("Not sent · no access");
    refused.querySelector("button")!.click(); expect(edit).toHaveBeenCalledWith("Ship it");
  });
  it("renders nothing-waiting beside the log with the monogram, the quiet line and the echo, and clears it when a turn arrives", () => {
    const history = new ConversationHistory();
    let echo: string | undefined = "Promoted the hub to production on Monday.";
    const view = new ConversationView(document, history, { supervisor: "calm-heron-5", machine: "Bench", project: "cas-hub-static", echo: () => echo }); document.body.replaceChildren(view.element);
    view.update();
    const empty = view.element.querySelector<HTMLElement>(".empty")!;
    expect(empty.hidden).toBe(false); expect(view.element.querySelector<HTMLElement>(".msgs")?.hidden).toBe(true);
    expect(view.element.querySelector(".msgs")?.children).toHaveLength(0);
    expect(empty.querySelector(".mono")?.textContent).toBe("B");
    expect(empty.querySelector("b")?.textContent).toBe("calm-heron-5");
    expect(empty.querySelector(".proj2")?.textContent).toBe("Bench · cas-hub-static");
    expect(empty.querySelector(".said")?.textContent).toBe("Nothing waiting on you. calm-heron-5 will write here when it needs a decision.");
    expect(empty.querySelector(".quiet")?.textContent).toBe("Promoted the hub to production on Monday.");
    echo = undefined; view.update();
    expect(empty.querySelector(".quiet")).toBeNull();
    history.reply(reply(1, "answer", "Back on it."), at(10, 0)); view.update();
    expect(empty.hidden).toBe(true); expect(view.element.querySelector<HTMLElement>(".msgs")?.hidden).toBe(false);
    expect(empty.children).toHaveLength(0);
  });
  it("lets siblings register kind renderers for ask, blocker and attachment", () => {
    const unregisterAsk = registerTurnRenderer("ask", (reply, ctx) => { const node = ctx.document.createElement("div"); node.className = "obj t-a"; node.append(...ctx.body()); return node; });
    const unregisterSheet = registerTurnRenderer("attachment", (reply, ctx) => { const node = ctx.document.createElement("div"); node.className = "sheet"; node.textContent = ctx.attachment!.name; return node; });
    try {
      const history = new ConversationHistory();
      const view = new ConversationView(document, history, "sup"); document.body.replaceChildren(view.element);
      history.reply({ ...reply(1, "ask", "Fix or ship?"), attachments: [{ artifact_id: "r/1", name: "brief.pdf", mime: "application/pdf", size_bytes: 1, sha256: "a".repeat(64) }] }, at(9, 58));
      history.reply(reply(2, "blocker", "Gate red."), at(9, 59));
      view.update();
      const ask = view.element.querySelector<HTMLElement>('[data-kind="ask"]')!;
      expect(ask.classList.contains("obj")).toBe(true); expect(ask.querySelector("p")?.textContent).toBe("Fix or ship?");
      expect(ask.querySelector(".sheet")?.textContent).toBe("brief.pdf"); expect(ask.querySelector("a")).toBeNull();
      expect(view.element.querySelector<HTMLElement>('[data-kind="blocker"]')?.classList.contains("bub")).toBe(true);
    } finally { unregisterAsk(); unregisterSheet(); }
    const history = new ConversationHistory(); const view = new ConversationView(document, history, "sup");
    history.reply(reply(3, "ask"), at(10, 0)); view.update();
    expect(view.element.querySelector<HTMLElement>('[data-kind="ask"]')?.classList.contains("bub")).toBe(true);
  });
});
