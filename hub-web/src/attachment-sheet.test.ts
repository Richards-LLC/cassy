// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { attachmentSize, attachmentTypeMark, installAttachmentSheet, renderAttachmentSheet } from "./attachment-sheet";
import { ConversationHistory } from "./conversation-history";
import { ConversationView, registerTurnRenderer } from "./conversation-view";
import type { ArtifactRef, OperatorReply } from "./types";

const brief: ArtifactRef = { artifact_id: "report/3.26.0/brief.pdf", name: "3.26.0 release brief.pdf", mime: "application/pdf", size_bytes: 1_468_006, sha256: "9f".repeat(32) };
const card: ArtifactRef = { artifact_id: "report/3.26.0/card.html", name: "Release report card.html", mime: "text/html", size_bytes: 88_064, sha256: "1a".repeat(32) };
const at = (hh: number, mm: number) => new Date(2026, 8, 21, hh, mm).getTime();
function reply(id: number, message: string, attachments?: ArtifactRef[], kind: OperatorReply["kind"] = "receipt"): OperatorReply {
  return { notification_id: id, reply_to: null, message, summary: "", device_id: "d", kind, ...(attachments ? { attachments } : {}) };
}

describe("attachment sheet (Pebble 4)", () => {
  it("marks the type from the mime, then the extension, and never invents one", () => {
    expect(attachmentTypeMark("application/pdf", "x.bin")).toBe("PDF");
    expect(attachmentTypeMark("text/html; charset=utf-8", "card.html")).toBe("HTML");
    expect(attachmentTypeMark("text/markdown", "notes.md")).toBe("MD");
    expect(attachmentTypeMark("application/octet-stream", "trace.perfetto")).toBe("FILE");
    expect(attachmentTypeMark("application/octet-stream", "dump.tar")).toBe("TAR");
    expect(attachmentTypeMark("application/vnd.ms-excel", "sheet")).toBe("EXCEL");
    expect(attachmentTypeMark("application/octet-stream", "blob")).toBe("FILE");
  });
  it("prints sizes the way a person reads them", () => {
    expect(attachmentSize(0)).toBe("0 B");
    expect(attachmentSize(999)).toBe("999 B");
    expect(attachmentSize(88_064)).toBe("86 KB");
    expect(attachmentSize(1_468_006)).toBe("1.4 MB");
    expect(attachmentSize(-1)).toBe("");
  });
  it("renders one sheet per artifact: the artifact link, a type plate, the name and the size", () => {
    const sheet = renderAttachmentSheet(document, brief, "atlas-sup");
    expect(sheet.tagName).toBe("A");
    expect(sheet.className).toBe("sheet");
    expect(sheet.getAttribute("href")).toBe("#artifact:report%2F3.26.0%2Fbrief.pdf");
    expect(sheet.dataset.artifactId).toBe(brief.artifact_id);
    expect(sheet.querySelector(".plate")?.textContent).toBe("PDF");
    expect(sheet.querySelector(".fname")?.textContent).toBe("3.26.0 release brief.pdf");
    expect(sheet.querySelector(".fsub")?.textContent).toBe("1.4 MB");
    expect(sheet.getAttribute("aria-label")).toBe("3.26.0 release brief.pdf, PDF, 1.4 MB, from atlas-sup. Open");
    expect(sheet.querySelector("script")).toBeNull();
  });
});

describe("sheets in the thread", () => {
  let uninstall: () => void;
  beforeEach(() => { uninstall = installAttachmentSheet(); });
  afterEach(() => { uninstall(); });

  it("lays each artifact on the thread beside the bubble, never inside it, keyed so updates keep the nodes", () => {
    const history = new ConversationHistory();
    const view = new ConversationView(document, history, { supervisor: "atlas-sup", machine: "Atlas", project: "cas-src" });
    document.body.replaceChildren(view.element);
    history.reply(reply(71, "Gate green. The brief and the report card are attached.", [brief, card]), at(9, 52));
    view.update();
    const turn = view.element.querySelector<HTMLElement>(".msgs > .turn.sup")!;
    const children = [...turn.children].map((child) => child.tagName === "TIME" ? "time" : child.className.split(" ")[0]);
    expect(children).toEqual(["bub", "sheet", "sheet", "time"]);
    expect(turn.querySelector(".bub .sheet")).toBeNull();
    expect(turn.querySelector(".bub a")).toBeNull();
    expect(turn.querySelector(".bub .receipt-text")?.textContent).toBe("Gate green. The brief and the report card are attached.");
    const sheets = turn.querySelectorAll<HTMLElement>(".sheet");
    expect(sheets[0]?.dataset.key).toBe("reply:71#0"); expect(sheets[1]?.dataset.key).toBe("reply:71#1");
    expect(sheets[1]?.querySelector(".plate")?.textContent).toBe("HTML");
    expect(turn.querySelectorAll("time")).toHaveLength(1);
    const first = sheets[0]!;
    history.reply(reply(72, "Also tagged.", undefined, "answer"), at(9, 53)); view.update();
    expect(view.element.querySelector(".sheet")).toBe(first);
    expect(view.element.querySelector<HTMLElement>(".bub")?.classList.contains("group-first")).toBe(true);
  });
  it("shows an attachment-only turn as its sheet alone", () => {
    const history = new ConversationHistory();
    const view = new ConversationView(document, history, "sup"); document.body.replaceChildren(view.element);
    history.reply(reply(51, "", [brief], "answer"), at(9, 52)); view.update();
    const turn = view.element.querySelector<HTMLElement>(".msgs > .turn")!;
    expect(turn.querySelector(".bub")?.classList.contains("bub-empty")).toBe(true);
    expect(turn.querySelector(".sheet .fname")?.textContent).toBe(brief.name);
  });
  it("keeps the attachments of a custom ask object inside that object", () => {
    // Pebble 3 owns the ask silhouette; its body() still receives the sheets.
    const unregister = registerTurnRenderer("ask", (_reply, ctx) => { const node = ctx.document.createElement("div"); node.className = "obj"; node.append(...ctx.body()); return node; });
    try {
      const history = new ConversationHistory();
      const view = new ConversationView(document, history, "sup"); document.body.replaceChildren(view.element);
      history.reply(reply(60, "Fix or ship?", [brief], "ask"), at(9, 58)); view.update();
      const ask = view.element.querySelector<HTMLElement>('[data-kind="ask"]')!;
      expect(ask.querySelector(".sheet")).not.toBeNull();
      expect(ask.nextElementSibling?.classList.contains("sheet")).toBe(false);
    } finally { unregister(); }
  });
});

describe("without the sheet installed", () => {
  it("falls back to the link row inside the bubble", () => {
    const history = new ConversationHistory();
    const view = new ConversationView(document, history, "sup"); document.body.replaceChildren(view.element);
    history.reply(reply(44, "A report is ready.", [brief]), at(9, 52)); view.update();
    expect(view.element.querySelector(".sheet")).toBeNull();
    expect(view.element.querySelector(".bub a")?.getAttribute("href")).toBe("#artifact:report%2F3.26.0%2Fbrief.pdf");
  });
});
