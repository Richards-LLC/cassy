// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { ConversationHistory, RECEIPT_TIMEOUT_MS } from "./conversation-history";
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
  it("shows a loading line, not the empty state, until the first history page lands (cas-04ee)", () => {
    const history = new ConversationHistory();
    let loading = true;
    const view = new ConversationView(document, history, { supervisor: "sup", loadingHistory: () => loading });
    document.body.replaceChildren(view.element);
    expect(view.element.dataset.mountOverlay).toBe("");
    view.update();
    const empty = view.element.querySelector<HTMLElement>(".empty")!;
    expect(empty.hidden).toBe(false);
    expect(empty.dataset.state).toBe("loading");
    expect(empty.querySelector('[role="status"]')?.textContent).toBe("Loading your conversation with sup…");
    expect(empty.textContent).not.toContain("Nothing waiting");
    // The page lands empty: now the empty state is the truth.
    loading = false; view.update();
    expect(empty.dataset.state).toBeUndefined();
    expect(empty.querySelector(".said")?.textContent).toBe("Nothing waiting on you. sup will write here when it needs a decision.");
    // A page with turns shows the turns, whatever the flag says.
    loading = true; history.reply(reply(1, "answer", "Ready."), at(9, 0)); view.update();
    expect(empty.hidden).toBe(true);
    expect(view.element.querySelector(".msgs")?.textContent).toContain("Ready.");
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
    history.reject("x", "forbidden"); view.update();
    const refused = view.element.querySelector<HTMLElement>('.conversation-turn[data-state="error"]')!;
    expect(refused.querySelector(".conversation-delivery")?.textContent).toBe("Not sent · This device isn't the one in control of the session. Take control, then retry.");
    refused.querySelector("button")!.click(); expect(edit).toHaveBeenCalledWith("Ship it", expect.objectContaining({ id: "x" }));
  });
  it("marks a refused send as not sent: warning glyph, a Not sent lead, the reason, then Edit and Retry (P8, cas-b1ee)", () => {
    const history = new ConversationHistory();
    const edit = vi.fn(); const retry = vi.fn();
    const view = new ConversationView(document, history, { supervisor: "sup", editMessage: edit, retryMessage: retry }); document.body.replaceChildren(view.element);
    history.submit("x", "sup", "Ship it", at(9, 0), 52); history.reject("x", "forbidden"); view.update();
    const refused = view.element.querySelector<HTMLElement>('.turn.you .bub[data-state="error"]')!;
    const label = refused.querySelector<HTMLElement>(".conversation-refused")!;
    expect(label.getAttribute("role")).toBe("status");
    expect(label.querySelector("svg.warn")?.getAttribute("aria-hidden")).toBe("true");
    expect(label.querySelector("b")?.textContent).toBe("Not sent");
    // F6: the hub's code becomes a plain reason and the step that gets it through.
    expect(label.querySelector(".conversation-refused-reason")?.firstChild?.textContent).toBe("This device isn't the one in control of the session.");
    expect(label.querySelector(".conversation-refused-next")?.textContent).toBe(" Take control, then retry.");
    expect(label.textContent).not.toContain("forbidden");
    const buttons = [...refused.querySelectorAll<HTMLButtonElement>(".conversation-actions button")];
    expect(buttons.map((button) => [button.className, button.textContent, button.getAttribute("aria-label")])).toEqual([
      ["conversation-edit", "Edit", "Edit message"], ["conversation-retry", "Retry", "Retry sending"],
    ]);
    buttons[0]!.click(); expect(edit).toHaveBeenCalledWith("Ship it", expect.objectContaining({ id: "x" }));
    buttons[1]!.click(); expect(retry).toHaveBeenCalledWith(expect.objectContaining({ id: "x", text: "Ship it", replyTo: 52, state: "error" }));
    // cas-3433: the refusal says "Take control, then retry", so with a
    // takeControl handler the control sits on the message, first in the row.
    const take = vi.fn();
    const controlled = new ConversationView(document, history, { supervisor: "sup", editMessage: edit, retryMessage: retry, takeControl: take }); controlled.update();
    const withControl = [...controlled.element.querySelectorAll<HTMLButtonElement>('.bub[data-state="error"] .conversation-actions button')];
    expect(withControl.map((button) => [button.className, button.textContent, button.getAttribute("aria-label"), button.type])).toEqual([
      ["conversation-take-control", "Take control", "Take control of the session", "button"],
      ["conversation-edit", "Edit", "Edit message", "button"], ["conversation-retry", "Retry", "Retry sending", "button"],
    ]);
    withControl[0]!.click(); expect(take).toHaveBeenCalledWith(expect.objectContaining({ id: "x", state: "error" }));
    // Only a control refusal offers it: taking control fixes nothing else.
    const other = new ConversationHistory();
    other.submit("z", "sup", "Late answer", at(9, 2), 7); other.reject("z", "semantic message enqueue failed: in_reply_to notification 7 does not exist");
    const stale = new ConversationView(document, other, { supervisor: "sup", editMessage: edit, retryMessage: retry, takeControl: take }); stale.update();
    expect(stale.element.querySelector(".conversation-take-control")).toBeNull();
    expect(stale.element.querySelector(".conversation-retry")).not.toBeNull();
    // Without the callbacks a refused send still says Not sent, with no dead buttons.
    const bare = new ConversationView(document, history, "sup"); bare.update();
    expect(bare.element.querySelector(".conversation-refused b")?.textContent).toBe("Not sent");
    expect(bare.element.querySelector(".conversation-actions")).toBeNull();
    // The sending state keeps its quiet line and never offers the actions.
    history.submit("y", "sup", "Again", at(9, 1)); view.update();
    const sending = view.element.querySelector<HTMLElement>('.bub[data-state="sending"]')!;
    expect(sending.querySelector(".conversation-delivery")?.textContent).toBe("Sending…");
    expect(sending.querySelector(".conversation-refused, .conversation-actions")).toBeNull();
  });
  it("turns a send whose receipt never came into Not confirmed with Retry, not Sending… forever (cas-1622)", () => {
    const history = new ConversationHistory();
    const retry = vi.fn();
    const view = new ConversationView(document, history, { supervisor: "sup", editMessage: vi.fn(), retryMessage: retry }); document.body.replaceChildren(view.element);
    history.submit("x", "sup", "Is the gate green?", at(9, 0), 52);
    expect(history.unconfirmSilent(at(9, 0) + RECEIPT_TIMEOUT_MS - 1)).toEqual([]);
    view.update();
    expect(view.element.querySelector(".conversation-delivery")?.textContent).toBe("Sending…");
    expect(history.unconfirmSilent(at(9, 0) + RECEIPT_TIMEOUT_MS)).toEqual(["x"]);
    view.update();
    const bubble = view.element.querySelector<HTMLElement>('.turn.you .bub[data-state="unconfirmed"]')!;
    const label = bubble.querySelector<HTMLElement>(".conversation-unconfirmed")!;
    expect(label.getAttribute("role")).toBe("status");
    expect(label.querySelector("svg.warn")?.getAttribute("aria-hidden")).toBe("true");
    expect(label.textContent).toBe("Not confirmed · The hub never confirmed this reached sup. Retry sends it again.");
    // It may have arrived: no "Not sent", and only Retry (an edit could reach the supervisor twice as easily).
    expect(label.textContent).not.toContain("Not sent");
    const buttons = [...bubble.querySelectorAll<HTMLButtonElement>(".conversation-actions button")];
    expect(buttons.map((button) => [button.textContent, button.getAttribute("aria-label")])).toEqual([["Retry", "Retry sending"]]);
    buttons[0]!.click(); expect(retry).toHaveBeenCalledWith(expect.objectContaining({ id: "x", text: "Is the gate green?", replyTo: 52, state: "unconfirmed" }));
    // A late receipt still turns it into Delivered.
    expect(history.acknowledge({ client_ref: "x", notification_id: 60, target: "sup", stamped: true })).toBe(true);
    view.update();
    expect(view.element.querySelector(".conversation-unconfirmed")).toBeNull();
    expect(view.element.querySelector(".conversation-delivered")?.textContent).toBe("Delivered");
  });
  it("names each message group's speaker and time for assistive tech (cas-17e3)", () => {
    const history = new ConversationHistory();
    const view = new ConversationView(document, history, { supervisor: "sup" }); document.body.replaceChildren(view.element);
    history.submit("a", "sup", "Are we back?", at(12, 45));
    history.reply(reply(2, "answer", "Back. Nothing was lost."), at(12, 45));
    history.reply(reply(3, "status", "gate 1 of 3"), at(12, 46)); history.reply(reply(4, "status", "gate 2 of 3"), at(12, 46));
    view.update();
    const groups = [...view.element.querySelectorAll<HTMLElement>('.msgs [role="group"]')];
    expect(groups.map((group) => group.getAttribute("aria-label")?.replace(/\d{1,2}:\d{2}/, "<t>"))).toEqual(["You, <t>", "sup, <t>", "sup, status, <t>"]);
    // The visible time is not read twice: the group label carries it.
    for (const time of view.element.querySelectorAll(".msgs time")) expect(time.getAttribute("aria-hidden")).toBe("true");
  });
  it("says Delivered on the latest delivered send until the reply lands (F5)", () => {
    const history = new ConversationHistory();
    const view = new ConversationView(document, history, { supervisor: "sup" }); document.body.replaceChildren(view.element);
    history.submit("a", "sup", "First", at(9, 0)); view.update();
    expect(view.element.querySelector(".conversation-delivered")).toBeNull();
    history.acknowledge({ client_ref: "a", notification_id: 10, target: "sup", stamped: true }); view.update();
    const delivered = view.element.querySelector<HTMLElement>('.bub[data-state="acknowledged"] .conversation-delivered')!;
    expect(delivered.textContent).toBe("Delivered");
    expect(delivered.getAttribute("role")).toBe("status");
    expect(delivered.querySelector("svg.tick")?.getAttribute("aria-hidden")).toBe("true");
    // A second send still in flight leaves the first as the latest delivered one.
    history.submit("b", "sup", "Second", at(9, 1)); view.update();
    expect(view.element.querySelectorAll(".conversation-delivered")).toHaveLength(1);
    expect(view.element.querySelector('.bub[data-state="sending"] .conversation-delivery')?.textContent).toBe("Sending…");
    // Once it is delivered too, only the latest says so.
    history.acknowledge({ client_ref: "b", notification_id: 11, target: "sup", stamped: true }); view.update();
    expect([...view.element.querySelectorAll(".conversation-delivered")].map((node) => node.closest(".bub")?.textContent)).toEqual(["SecondDelivered"]);
    // Any supervisor turn after it is the evidence now; Delivered steps aside.
    history.reply({ notification_id: 12, reply_to: null, message: "On it.", summary: "", device_id: "d" }, at(9, 2)); view.update();
    expect(view.element.querySelector(".conversation-delivered")).toBeNull();
    expect(history.delivered()).toBeUndefined();
  });
  it("retires a refused send once its edited version is sent: collapsed, no Retry (F6)", () => {
    const history = new ConversationHistory();
    const view = new ConversationView(document, history, { supervisor: "sup", editMessage: vi.fn(), retryMessage: vi.fn() }); document.body.replaceChildren(view.element);
    history.submit("bad", "sup", "Ship it without the gate.", at(9, 0)); history.reject("bad", "forbidden"); view.update();
    expect(history.preview()).toBe("Not sent: Ship it without the gate.");
    expect(history.retireRefused("missing")).toBe(false);
    expect(history.retireRefused("bad")).toBe(true);
    history.submit("good", "sup", "Ship it after the gate passes.", at(9, 1)); view.update();
    const retired = view.element.querySelector<HTMLElement>('.bub[data-state="error"]')!;
    expect(retired.dataset.replaced).toBe("true");
    expect(retired.querySelector(".conversation-replaced")?.textContent).toBe("Not sent · replaced by your edit");
    expect(retired.querySelector(".conversation-actions, .conversation-retry, .conversation-edit")).toBeNull();
    expect(history.preview()).toBe("You: Ship it after the gate passes.");
    // Only a refused send can be retired.
    expect(history.retireRefused("good")).toBe(false);
  });
  it("never previews unsent text as said, and skips a retired refusal (F6)", () => {
    const history = new ConversationHistory();
    expect(history.preview()).toBeUndefined();
    history.reply({ notification_id: 1, reply_to: null, message: "Ready.", summary: "", device_id: "d" }, at(9, 0));
    history.submit("bad", "sup", "Wrong", at(9, 1)); history.reject("bad", "forbidden");
    expect(history.preview()).toBe("Not sent: Wrong");
    history.retireRefused("bad");
    expect(history.preview()).toBe("Ready.");
  });
  it("places a new send after every turn already shown, even when the machine's clock runs ahead (cas-ce17)", () => {
    const history = new ConversationHistory();
    const now = Date.now();
    // Hydrated from a machine whose clock is five minutes ahead of this browser.
    // Its future stamp is clamped to the arrival (cas-1f13), so the ask that follows sorts after it.
    history.hydrateReply({ notification_id: 51, reply_to: null, message: "The gate went red.", summary: "", device_id: "d", kind: "blocker", attachments: [], at: new Date(now + 300_000).toISOString() }, now);
    history.receive({ notification_id: 52, reply_to: null, message: "Fix or ship?", summary: "", device_id: "d", kind: "ask", options: ["Fix", "Ship"] }, now);
    expect(history.waiting().map((reply) => reply.notification_id)).toEqual([51, 52]);
    history.submit("answer", "sup", "Fix", now, 52);
    expect(history.events.at(-1)).toMatchObject({ kind: "send", value: { id: "answer" } });
    // The ask is answered and the blocker acknowledged: nothing waits.
    expect(history.waiting()).toEqual([]);
    // A blocker arriving live after the answer waits, and follows the answer in the thread.
    history.receive({ notification_id: 53, reply_to: null, message: "A second gate went red.", summary: "", device_id: "d", kind: "blocker" }, Date.now());
    expect(history.waiting().map((reply) => reply.notification_id)).toEqual([53]);
    expect(history.events.at(-1)).toMatchObject({ kind: "reply", value: { notification_id: 53 } });
  });
  it("marks a turn from a machine clock ahead quietly, at its arrival time, with no future day (cas-1f13)", () => {
    const history = new ConversationHistory();
    const now = Date.now();
    history.hydrateReply({ notification_id: 61, reply_to: null, message: "Mac build is queued.", summary: "", device_id: "d", kind: "answer", attachments: [], at: new Date(now + 86_400_000).toISOString() }, now);
    const view = new ConversationView(document, history, { supervisor: "calm-otter-4" }); document.body.replaceChildren(view.element); view.update();
    expect([...view.element.querySelectorAll(".day")].map((day) => day.textContent)).toEqual(["Today"]);
    const group = view.element.querySelector<HTMLElement>('.turn.sup[role="group"]')!;
    const clock = `${String(new Date(now).getHours()).padStart(2, "0")}:${String(new Date(now).getMinutes()).padStart(2, "0")}`;
    expect(group.getAttribute("aria-label")).toBe(`calm-otter-4, ${clock}, machine clock ahead`);
    const time = group.querySelector<HTMLElement>(":scope > time")!;
    expect(time.textContent).toBe(`${clock} · machine clock ahead`);
    expect(time.querySelector(".clock-ahead")).not.toBeNull();
    expect(time.title).toContain("clock is ahead");
    // A turn on time carries no hint.
    history.receive({ notification_id: 62, reply_to: null, message: "Started.", summary: "", device_id: "d", kind: "answer" }, now + 60_000);
    history.submit("s", "calm-otter-4", "Thanks", now + 61_000); view.update();
    // The hint belongs to the time shown: once an on-time turn is the group's latest, it goes.
    expect(group.querySelector(".clock-ahead")).toBeNull();
    const you = view.element.querySelector<HTMLElement>('.turn.you[role="group"]')!;
    expect(you.querySelector(".clock-ahead")).toBeNull();
    expect(you.getAttribute("aria-label")).not.toContain("clock");
  });
  it("discards only a refused send when a retry replaces it", () => {
    const history = new ConversationHistory();
    history.submit("ok", "sup", "Fine", at(9, 0)); history.submit("bad", "sup", "Ship it", at(9, 1)); history.reject("bad", "no access");
    expect(history.discardRefused("ok")).toBe(false);
    expect(history.discardRefused("bad")).toBe(true);
    expect(history.events.map((event) => event.kind === "send" && event.value.id)).toEqual(["ok"]);
    expect(history.discardRefused("bad")).toBe(false);
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
    // Journey F7: the project titles the card as it titles the header and the list; machine · codename beneath.
    expect(empty.querySelector("b")?.textContent).toBe("cas-hub-static");
    expect(empty.querySelector(".proj2")?.textContent).toBe("Bench · calm-heron-5");
    expect(empty.querySelector(".proj2 > .codename")?.textContent).toBe("calm-heron-5");
    expect(empty.querySelector(".said")?.textContent).toBe("Nothing waiting on you. calm-heron-5 will write here when it needs a decision.");
    // The codename in the sentence is an identifier span that never breaks at its hyphen.
    expect(empty.querySelector(".said .codename")?.textContent).toBe("calm-heron-5");
    expect(empty.querySelector("b")?.classList.contains("codename")).toBe(false);
    expect(empty.querySelector(".quiet")?.textContent).toBe("Promoted the hub to production on Monday.");
    echo = undefined; view.update();
    expect(empty.querySelector(".quiet")).toBeNull();
    // No project: the codename is the only name and keeps the title.
    const bare = new ConversationView(document, new ConversationHistory(), { supervisor: "calm-heron-5", machine: "Bench" });
    bare.update();
    const bareEmpty = bare.element.querySelector<HTMLElement>(".empty")!;
    expect(bareEmpty.querySelector("b.codename")?.textContent).toBe("calm-heron-5");
    expect(bareEmpty.querySelector(".proj2")?.textContent).toBe("Bench");
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
