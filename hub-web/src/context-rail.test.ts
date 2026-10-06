// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from "vitest";
import { ConversationHistory } from "./conversation-history";
import { conversationShellMarkup } from "./conversation-shell";
import { ConversationView } from "./conversation-view";
import { CONTEXT_ENTRY_LIMIT, contextSections, entryText, syncContextRail, threadAttachments, BLOCKER_HINT, PINNED_MARK } from "./context-rail";
import type { ArtifactRef, OperatorReply, OperatorTurnKind } from "./types";

const at = (hh: number, mm: number) => new Date(2026, 8, 22, hh, mm).getTime();
const PDF: ArtifactRef = { artifact_id: "art-1", name: "3.26.0 release brief.pdf", mime: "application/pdf", size_bytes: 1_468_006, sha256: "a".repeat(64) };
const HTML: ArtifactRef = { artifact_id: "art-2", name: "Release report card.html", mime: "text/html", size_bytes: 88_064, sha256: "b".repeat(64) };
function reply(id: number, kind: OperatorTurnKind, message: string, attachments?: ArtifactRef[]): OperatorReply {
  return { notification_id: id, reply_to: null, message, summary: "", device_id: "d", kind, ...(attachments ? { attachments } : {}) };
}

function mountShell(): HTMLElement {
  document.body.innerHTML = conversationShellMarkup({ selected: true, supervisor: "patient-pelican-9", projectDir: "/projects/cas-src", host: "Atlas · Linux", machineId: "atlas-linux", loaded: true, paired: true });
  return document.querySelector<HTMLElement>(".conversation-context")!;
}
const section = (rail: HTMLElement, name: string) => rail.querySelector<HTMLElement>(`[data-section="${name}"]`)!;
const shell = () => document.querySelector<HTMLElement>(".conversation-shell")!;

describe("desktop context rail (P10)", () => {
  beforeEach(() => { document.body.replaceChildren(); });

  it("repeats nothing from the header: no lockup, badge, supervisor or host", () => {
    const rail = mountShell();
    expect(rail.querySelector(".cloud-brand")).toBeNull();
    expect(rail.querySelector(".project-badge")).toBeNull();
    expect(rail.textContent).not.toContain("patient-pelican-9");
    expect(rail.textContent).not.toContain("cas-src");
    expect(rail.textContent).not.toContain("Atlas");
    expect(rail.textContent).not.toContain("appears here");
  });

  it("folds to the 48px track when the thread has nothing unique", () => {
    const rail = mountShell();
    const history = new ConversationHistory();
    history.submit("a", "patient-pelican-9", "Merge the lanes.", at(9, 41));
    history.reply(reply(1, "answer", "On it."), at(9, 44));
    expect(syncContextRail(document, { history, progress: false, attention: 0 })).toBe(false);
    expect(shell().classList.contains("context-open")).toBe(false);
    expect(rail.dataset.open).toBe("false");
    expect(rail.getAttribute("aria-hidden")).toBe("true");
    for (const name of ["waiting", "progress", "attachments", "attention"]) expect(section(rail, name).hidden).toBe(true);
  });

  it("names waiting only while a real conversation turn still needs me (cas-b113)", () => {
    const rail = mountShell();
    const history = new ConversationHistory();
    // The static heading labels the section when shown, but an empty
    // section is hidden and contributes no actionable waiting entries.
    expect(section(rail, "waiting").hidden).toBe(true);
    syncContextRail(document, { history, progress: true, attention: 0 });
    expect(section(rail, "waiting").hidden).toBe(true);
    expect(section(rail, "waiting").querySelectorAll("li")).toHaveLength(0);
    history.reply(reply(1, "blocker", "Please fix the ledger."), at(9, 30));
    syncContextRail(document, { history, progress: true, attention: 0 });
    expect(section(rail, "waiting").hidden).toBe(false);
    expect(rail.querySelector("#context-waiting-heading")?.textContent).toBe("Waiting on you");
    expect(section(rail, "waiting").querySelectorAll("li")).toHaveLength(1);
    history.submit("handled", "patient-pelican-9", "Fixed.", at(9, 31));
    syncContextRail(document, { history, progress: true, attention: 0 });
    expect(section(rail, "waiting").hidden).toBe(true);
    expect(section(rail, "waiting").querySelectorAll("li")).toHaveLength(0);
    // Settling one blocker must not suppress a later genuine blocker.
    history.reply(reply(2, "blocker", "Please check another row."), at(9, 32));
    syncContextRail(document, { history, progress: true, attention: 0 });
    expect(section(rail, "waiting").hidden).toBe(false);
    expect(rail.querySelector("#context-waiting-heading")?.textContent).toBe("Waiting on you");
  });

  it("folds with no conversation open: the welcome shell has no rail sections at all", () => {
    document.body.innerHTML = conversationShellMarkup({ selected: false, loaded: true, paired: true });
    const rail = document.querySelector<HTMLElement>(".conversation-context")!;
    expect(syncContextRail(document, { progress: false, attention: 0 })).toBe(false);
    expect(rail.children).toHaveLength(0);
    expect(rail.textContent).toBe("");
  });

  it("opens with the thread's attachments, newest first and once per artifact", () => {
    const rail = mountShell();
    const history = new ConversationHistory();
    history.reply(reply(1, "answer", "Brief attached.", [PDF]), at(9, 30));
    history.reply(reply(2, "answer", "And the card.", [HTML, PDF]), at(9, 32));
    expect(threadAttachments(history).map((item) => item.attachment.artifact_id)).toEqual(["art-2", "art-1"]);
    expect(syncContextRail(document, { history, progress: false, attention: 0 })).toBe(true);
    expect(shell().classList.contains("context-open")).toBe(true);
    expect(rail.hasAttribute("aria-hidden")).toBe(false);
    expect(section(rail, "attachments").hidden).toBe(false);
    expect(section(rail, "waiting").hidden).toBe(true);
    const links = [...rail.querySelectorAll<HTMLAnchorElement>(".context-attachment")];
    expect(links.map((link) => link.getAttribute("href"))).toEqual(["#artifact:art-2", "#artifact:art-1"]);
    expect(links[1]!.getAttribute("aria-label")).toBe("3.26.0 release brief.pdf, PDF, 1.4 MB. Open");
    expect(links[0]!.querySelector(".context-meta")?.textContent).toBe("86 KB");
  });

  it("lists every open ask and blocker, newest first, marking the one pinned above the composer (cas-97d58 F09), and a click lands on the turn", () => {
    const rail = mountShell();
    const history = new ConversationHistory();
    const view = new ConversationView(document, history, { supervisor: "patient-pelican-9", header: false });
    document.querySelector("#conversation-pane-slot")!.append(view.element);
    history.reply(reply(9, "ask", "Keep the old runner pool for now?"), at(9, 56));
    history.reply(reply(10, "blocker", "The release gate went red.\nattention.rs:212 · needless_borrow"), at(9, 57));
    history.reply(reply(11, "ask", "Fix it in-train, or ship 3.26.0 with it allowlisted?"), at(9, 58));
    view.update();
    syncContextRail(document, { history, progress: false, attention: 0 });
    expect(section(rail, "waiting").hidden).toBe(false);
    expect(history.pinnedAsk()?.notification_id).toBe(11);
    const jumps = [...rail.querySelectorAll<HTMLButtonElement>(".context-jump")];
    expect(jumps.map((jump) => [jump.dataset.kind, jump.querySelector(".context-kind")?.textContent, jump.querySelector(".context-text")?.textContent])).toEqual([
      ["ask", "Question", "Fix it in-train, or ship 3.26.0 with it allowlisted?"],
      ["blocker", "Blocker", "The release gate went red."],
      ["ask", "Question", "Keep the old runner pool for now?"],
    ]);
    // A blocker's entry says how it clears; the pinned question says where it is (journey F12, F09).
    expect(jumps.map((jump) => jump.querySelector(".context-hint")?.textContent ?? null)).toEqual([PINNED_MARK, BLOCKER_HINT, null]);
    expect(jumps.map((jump) => jump.dataset.pinned ?? null)).toEqual(["true", null, null]);
    expect(BLOCKER_HINT).toBe("Reply to unblock");
    jumps[1]!.click();
    const turn = document.querySelector<HTMLElement>('.thread [data-key="reply:10"]')!;
    expect(document.activeElement).toBe(turn);
    expect(turn.tabIndex).toBe(-1);
  });

  it("cuts a long entry at a word with an ellipsis, never mid-word, and keeps short ones whole", () => {
    const ask = "Gate run 33512 failed on that one warning. Fix it in-train — one worker, about ten minutes — or ship 3.26.0 with it allowlisted?";
    const cut = entryText(ask);
    expect(cut).toBe("Gate run 33512 failed on that one warning. Fix it in-train — one worker, about ten…");
    expect(cut.length).toBeLessThanOrEqual(CONTEXT_ENTRY_LIMIT + 1);
    expect(entryText("Ship it?\nMore detail below.")).toBe("Ship it?");
  });
  it("closes an ask entry once answered, and folds when nothing else is left", () => {
    const rail = mountShell();
    const history = new ConversationHistory();
    history.reply(reply(10, "ask", "Ship it?"), at(9, 57));
    history.reply(reply(11, "ask", "And tag it?"), at(9, 58));
    // Both open asks are listed; the newest is marked as the pinned one.
    expect(syncContextRail(document, { history, progress: false, attention: 0 })).toBe(true);
    expect(rail.querySelectorAll(".context-jump")).toHaveLength(2);
    history.submit("r", "patient-pelican-9", "Ship it.", at(9, 59), 10);
    // The bookmark still points at "And tag it?", so the rail still lists it (F09).
    expect(syncContextRail(document, { history, progress: false, attention: 0 })).toBe(true);
    expect([...rail.querySelectorAll(".context-jump .context-text")].map((text) => text.textContent)).toEqual(["And tag it?"]);
    history.submit("t", "patient-pelican-9", "Tag it.", at(10, 0), 11);
    expect(syncContextRail(document, { history, progress: false, attention: 0 })).toBe(false);
    expect(rail.querySelectorAll(".context-jump")).toHaveLength(0);
    expect(shell().classList.contains("context-open")).toBe(false);
  });

  it("shows task progress and attention only when status and attention report something for this thread", () => {
    const rail = mountShell();
    const history = new ConversationHistory();
    expect(contextSections({ history, progress: true, attention: 0 })).toEqual(["progress"]);
    syncContextRail(document, { history, progress: true, attention: 0 });
    expect(section(rail, "progress").hidden).toBe(false);
    expect(section(rail, "progress").querySelector("#conversation-status-slot")).not.toBeNull();
    expect(section(rail, "attention").hidden).toBe(true);
    syncContextRail(document, { history, progress: false, attention: 2 });
    expect(section(rail, "progress").hidden).toBe(true);
    expect(section(rail, "attention").hidden).toBe(false);
    expect(section(rail, "attention").querySelector("#conversation-attention-slot")).not.toBeNull();
    expect(shell().classList.contains("context-open")).toBe(true);
  });

  it("does not rebuild an unchanged list, so focus inside the rail survives a region update", () => {
    const rail = mountShell();
    const history = new ConversationHistory();
    history.reply(reply(1, "answer", "Brief.", [PDF]), at(9, 30));
    syncContextRail(document, { history, progress: false, attention: 0 });
    const link = rail.querySelector<HTMLAnchorElement>(".context-attachment")!;
    link.focus();
    syncContextRail(document, { history, progress: true, attention: 0 });
    expect(rail.querySelector(".context-attachment")).toBe(link);
    expect(document.activeElement).toBe(link);
  });
});
