// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { artifactHref, attachmentSize, attachmentTypeMark, installAttachmentSheet, renderAttachmentSheet, setAttachmentNote, clearTransientAttachmentNotes, restateAttachmentNotes } from "./attachment-sheet";
import { inlineAttachment, retryAttachmentPreviews } from "./rich-attachment";
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
    const files = [{ ...brief }, card];
    history.reply(reply(71, "Gate green. The brief and the report card are attached.", files), at(9, 52));
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
    history.markReplyPersisted(71); view.update();
    expect(view.element.querySelector(".sheet")).toBe(first);
    history.reply(reply(72, "Also tagged.", undefined, "answer"), at(9, 53)); view.update();
    expect(view.element.querySelector(".sheet")).toBe(first);
    expect(view.element.querySelector<HTMLElement>(".bub")?.classList.contains("group-first")).toBe(true);
    files[0]!.name = "Updated evidence.pdf"; view.update();
    expect(view.element.querySelector(".sheet")).not.toBe(first);
    expect(view.element.querySelector(".sheet .fname")?.textContent).toBe("Updated evidence.pdf");
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
      const sheet = ask.querySelector(".sheet");
      history.markReplyPersisted(60); view.update();
      expect(view.element.querySelector('.obj .sheet')).toBe(sheet);
    } finally { unregister(); }
  });
});

describe("a page attachment shows inline in a sandboxed frame (cas-3aff)", () => {
  it("frames only the machine's page URL, with every sandbox permission withheld, and keeps the frame across updates", async () => {
    const asked: string[] = [];
    const uninstall = installAttachmentSheet(undefined, async (id) => { asked.push(id); return id === card.artifact_id ? "https://atlas.test/v1/pages/T/card" : undefined; });
    try {
      const history = new ConversationHistory();
      const view = new ConversationView(document, history, "sup"); document.body.replaceChildren(view.element);
      const broken: ArtifactRef = { ...card, artifact_id: "art-broken", name: "Broken.html" };
      history.reply(reply(80, "Report attached.", [brief, card, broken]), at(10, 1)); view.update();
      await Promise.resolve(); await Promise.resolve();
      const turn = view.element.querySelector<HTMLElement>(".msgs > .turn.sup")!;
      expect([...turn.children].map((child) => child.tagName === "TIME" ? "time" : child.className.split(" ")[0])).toEqual(["bub", "sheet", "visual-page", "visual-page", "time"]);
      expect(asked).toEqual([card.artifact_id, "art-broken"]);
      const [page, failed] = [...turn.querySelectorAll<HTMLElement>(".visual-page")];
      // The card stays the file's link (its tap opens the page in a tab).
      expect(page!.querySelector("a.sheet")?.getAttribute("href")).toBe(artifactHref(card));
      const frame = page!.querySelector("iframe")!;
      expect(frame.getAttribute("sandbox")).toBe("");
      expect(frame.getAttribute("referrerpolicy")).toBe("no-referrer");
      expect(frame.getAttribute("allow")).toBe("");
      expect(frame.getAttribute("src")).toBe("https://atlas.test/v1/pages/T/card");
      expect(frame.title).toBe("Release report card.html, a page from the supervisor");
      expect(page!.querySelector("iframe[srcdoc]")).toBeNull();
      expect(failed!.querySelector("iframe")).toBeNull();
      // pin: the failed frame must say where the page can still be opened.
      expect(failed!.querySelector(".visual-status")?.textContent).toBe("This page can't be shown here. Open the file above to try it in a tab.");
      history.reply(reply(81, "Also tagged.", undefined, "answer"), at(10, 2)); view.update();
      expect(view.element.querySelector("iframe")).toBe(frame);
      expect(asked).toHaveLength(2);
    } finally { uninstall(); }
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

describe("a file card says why it did not open (journey F6)", () => {
  it("puts the reason on every card for that file, keeps it across a rebuild, and clears it", () => {
    document.body.replaceChildren(renderAttachmentSheet(document, brief), renderAttachmentSheet(document, card));
    const message = "This file was only saved on Atlas · Linux. It was never uploaded to Cloud, so it can't open here.";
    expect(setAttachmentNote(document, brief.artifact_id, message)).toBe(1);
    const sheet = document.querySelector<HTMLAnchorElement>(`a.sheet[data-artifact-id="${brief.artifact_id}"]`)!;
    expect(sheet.querySelector(".fnote")?.textContent).toBe(message);
    expect(sheet.querySelector(".fnote")?.getAttribute("role")).toBe("status");
    expect(sheet.getAttribute("aria-label")).toBe(`3.26.0 release brief.pdf, PDF, 1.4 MB. Open. ${message}`);
    // The other file is untouched.
    expect(document.querySelector(`a.sheet[data-artifact-id="${card.artifact_id}"] .fnote`)).toBeNull();
    // A repaint that rebuilds the card keeps what it said.
    const rebuilt = renderAttachmentSheet(document, brief);
    expect(rebuilt.querySelector(".fnote")?.textContent).toBe(message);
    // A file not shown as a card reports zero, so the caller says it elsewhere.
    expect(setAttachmentNote(document, "art-elsewhere", "x")).toBe(0);
    setAttachmentNote(document, "art-elsewhere", undefined);
    expect(setAttachmentNote(document, brief.artifact_id, undefined)).toBe(1);
    expect(sheet.querySelector(".fnote")).toBeNull();
    expect(sheet.getAttribute("aria-label")).toBe("3.26.0 release brief.pdf, PDF, 1.4 MB. Open");
    expect(renderAttachmentSheet(document, brief).querySelector(".fnote")).toBeNull();
  });
});

describe("a note about the connection does not outlive it (cas-c808 QA F01)", () => {
  it("clears that machine's transient notes when it reconnects, and keeps notes about the file", () => {
    document.body.replaceChildren(renderAttachmentSheet(document, brief), renderAttachmentSheet(document, card));
    setAttachmentNote(document, brief.artifact_id, "Couldn't reach Atlas · Linux. Check that it's on and connected, then open the file again.", { machineId: "atlas", transient: true });
    setAttachmentNote(document, card.artifact_id, "This file was only saved on Atlas · Linux. It was never uploaded to Cloud, so it can't open here.", { machineId: "atlas", transient: false });
    // Another machine reconnecting changes nothing here.
    expect(clearTransientAttachmentNotes(document, "studio")).toBe(0);
    expect(document.querySelector(`a.sheet[data-artifact-id="${brief.artifact_id}"] .fnote`)).not.toBeNull();
    expect(clearTransientAttachmentNotes(document, "atlas")).toBe(1);
    expect(document.querySelector(`a.sheet[data-artifact-id="${brief.artifact_id}"] .fnote`)).toBeNull();
    expect(document.querySelector(`a.sheet[data-artifact-id="${brief.artifact_id}"]`)?.getAttribute("aria-label")).toBe("3.26.0 release brief.pdf, PDF, 1.4 MB. Open");
    expect(document.querySelector(`a.sheet[data-artifact-id="${card.artifact_id}"] .fnote`)?.textContent).toContain("only saved on Atlas · Linux");
    // A rebuilt card does not bring the cleared note back.
    expect(renderAttachmentSheet(document, brief).querySelector(".fnote")).toBeNull();
    setAttachmentNote(document, card.artifact_id, undefined);
  });
});

describe("a note about the connection follows it (journey F28)", () => {
  it("says a connected-but-silent note again, quietly, when the machine leaves Connected", () => {
    document.body.replaceChildren(renderAttachmentSheet(document, brief), renderAttachmentSheet(document, card));
    let reach = "live";
    const words = () => reach === "live" ? "Atlas · Linux is connected but didn't send the file. Try again in a moment." : "Lost connection to Atlas · Linux. Reconnecting… Open the file again when it's back.";
    setAttachmentNote(document, brief.artifact_id, words(), { machineId: "atlas", transient: true, restate: words });
    setAttachmentNote(document, card.artifact_id, "This file was only saved on Atlas · Linux. It was never uploaded to Cloud, so it can't open here.", { machineId: "atlas", transient: false });
    const line = () => document.querySelector<HTMLElement>(`a.sheet[data-artifact-id="${brief.artifact_id}"] .fnote`);
    expect(line()?.getAttribute("role")).toBe("status");
    // Nothing changed: nothing is rewritten.
    expect(restateAttachmentNotes(document, "atlas")).toBe(0);
    reach = "reconnecting";
    // Another machine's change leaves this card alone.
    expect(restateAttachmentNotes(document, "studio")).toBe(0);
    expect(line()?.textContent).toContain("is connected");
    expect(restateAttachmentNotes(document, "atlas")).toBe(1);
    expect(line()?.textContent).toBe("Lost connection to Atlas · Linux. Reconnecting… Open the file again when it's back.");
    // The banner announces the outage; the card's rewrite is not a second announcement (journey F42).
    expect(line()?.hasAttribute("role")).toBe(false);
    // The card lives in the thread's role=log, a polite live region of its
    // own: the restated note opts out of it too (cas-c945 QA F01).
    expect(line()?.getAttribute("aria-live")).toBe("off");
    // Its name carries the new words on focus.
    expect(document.querySelector(`a.sheet[data-artifact-id="${brief.artifact_id}"]`)?.getAttribute("aria-label")).toBe("3.26.0 release brief.pdf, PDF, 1.4 MB. Open. Lost connection to Atlas · Linux. Reconnecting… Open the file again when it's back.");
    // A rebuilt card shows the current words, not the click-time ones.
    const rebuilt = renderAttachmentSheet(document, brief).querySelector(".fnote");
    expect(rebuilt?.textContent).toContain("Reconnecting");
    expect(rebuilt?.getAttribute("aria-live")).toBe("off");
    expect(rebuilt?.hasAttribute("role")).toBe(false);
    // A note about the file itself is not restated.
    expect(document.querySelector(`a.sheet[data-artifact-id="${card.artifact_id}"] .fnote`)?.textContent).toContain("only saved on Atlas · Linux");
    // Back to Live: the reconnect clears it, as before (cas-c808 QA F01).
    expect(clearTransientAttachmentNotes(document, "atlas")).toBe(1);
    expect(line()).toBeNull();
    setAttachmentNote(document, card.artifact_id, undefined);
  });
});

it("groups a served image thumbnail and a failed preview with their own file cards", async () => {
  for (const sender of [false, true]) {
    const file = { ...brief, artifact_id: `thumbnail-${sender}`, name: "Screenshot.png", mime: "image/png" };
    const sheet = renderAttachmentSheet(document, file, sender ? "you" : "supervisor");
    const root = inlineAttachment(sheet, file, async () => ({ ok: true, view: { artifact_id: file.artifact_id, url: "https://files.example/screenshot.png" } }));
    document.body.append(root);
    try {
      await vi.waitFor(() => expect(root.querySelector("img")).not.toBeNull());
      expect(sheet.querySelector(".plate img")?.getAttribute("src")).toBe("https://files.example/screenshot.png");
      expect(root.querySelectorAll("a")).toHaveLength(1);
      expect(sheet.querySelector<HTMLElement>(".rich-media")?.style.height).toBe("48px");
      expect(sheet.textContent).toContain(file.name);
      const unavailable = { ...file, artifact_id: `missing-${sender}`, name: "Unavailable.png" };
      const failedSheet = renderAttachmentSheet(document, unavailable);
      const failed = inlineAttachment(failedSheet, unavailable, async () => ({ ok: false, status: 404 }));
      document.body.append(failed);
      try {
        await vi.waitFor(() => expect(failedSheet.querySelector(".preview-note")?.textContent).toBe("Preview unavailable"));
        expect(failed.querySelector<HTMLElement>(".rich-media")?.style.display).toBe("none");
        expect(failed.querySelector("a.sheet")).toBe(failedSheet);
      } finally { failed.remove(); }
    } finally { root.remove(); }
  }
});

it("inspects only the turn's images with zoom, pan, gallery, failure and restored keyboard focus", async () => {
  const files = ["First.png", "Second.png", "Missing.png"].map((name) => ({ ...brief, name, mime: "image/png", artifact_id: name }));
  const fetchImage = async (file: ArtifactRef) => ({ ok: true as const, view: { artifact_id: file.artifact_id, url: `https://files.example/${file.name}` } });
  const sheet = renderAttachmentSheet(document, files[0]!);
  const root = inlineAttachment(sheet, files[0]!, () => fetchImage(files[0]!), { gallery: [...files, brief], fetchImage });
  document.body.append(root); root.scrollTop = 120;
  const show = Object.getOwnPropertyDescriptor(HTMLDialogElement.prototype, "showModal");
  const close = Object.getOwnPropertyDescriptor(HTMLDialogElement.prototype, "close");
  Object.defineProperties(HTMLDialogElement.prototype, {
    showModal: { configurable: true, value(this: HTMLDialogElement) { this.open = true; } },
    close: { configurable: true, value(this: HTMLDialogElement) { this.open = false; this.dispatchEvent(new Event("close")); } },
  });
  try {
    sheet.focus(); sheet.click();
    await vi.waitFor(() => expect(document.querySelector(".image-viewer[open]")).not.toBeNull());
    const dialog = document.querySelector<HTMLDialogElement>(".image-viewer")!;
    const button = (name: string) => dialog.querySelector<HTMLButtonElement>(`button[aria-label="${name}"]`)!;
    const canvas = dialog.querySelector<HTMLElement>(".image-viewer-canvas")!;
    const image = () => dialog.querySelector<HTMLImageElement>("img")!;
    vi.spyOn(canvas, "getBoundingClientRect").mockReturnValue({ width: 200, height: 200, left: 0, top: 0, right: 200, bottom: 200, x: 0, y: 0, toJSON: () => ({}) });
    Object.defineProperties(image(), { naturalWidth: { value: 800 }, naturalHeight: { value: 800 } });
    image().dispatchEvent(new Event("load"));
    expect(document.activeElement).toBe(button("Close image viewer"));
    dialog.dispatchEvent(new KeyboardEvent("keydown", { key: "+", bubbles: true }));
    expect(image().style.transform).toContain("scale(1.5)");
    dialog.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", shiftKey: true, bubbles: true }));
    expect(image().style.transform).not.toContain("translate(0px, 0px)");
    const pointer = (type: string, id: number, x: number) => canvas.dispatchEvent(Object.assign(new Event(type, { bubbles: true, cancelable: true }), { pointerId: id, pointerType: "touch", button: 0, clientX: x, clientY: 100 }));
    pointer("pointerdown", 1, 50); pointer("pointerdown", 2, 150); pointer("pointermove", 2, 180);
    expect(image().style.transform).not.toContain("scale(1.5)");
    pointer("pointerup", 1, 50); pointer("pointerup", 2, 180);
    button("Fit image").click();
    expect(image().style.transform).toContain("scale(1)");
    dialog.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
    await vi.waitFor(() => expect(image().getAttribute("src")).toContain("Second.png"));
    image().dispatchEvent(new Event("load"));
    expect(dialog.textContent).toContain("2 of 3");
    button("Next image").click();
    await vi.waitFor(() => expect(image().getAttribute("src")).toContain("Missing.png"));
    image().dispatchEvent(new Event("error"));
    expect(dialog.querySelector('[role="status"]')!.textContent).toContain("Couldn't load Missing.png");
    expect(button("Next image").disabled).toBe(true);
    button("Try image again").focus();
    dialog.dispatchEvent(new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true }));
    expect(document.activeElement).toBe(button("Close image viewer"));
    dialog.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(document.querySelector(".image-viewer")).toBeNull();
    expect(document.activeElement).toBe(sheet);
    expect(root.scrollTop).toBe(120);
  } finally {
    document.querySelector(".image-viewer")?.remove(); root.remove();
    for (const [name, descriptor] of [["showModal", show], ["close", close]] as const) {
      if (descriptor) Object.defineProperty(HTMLDialogElement.prototype, name, descriptor);
      else Reflect.deleteProperty(HTMLDialogElement.prototype, name);
    }
  }
});

it("loads supported inline media into reserved space and keeps the original Open link", async () => {
  for (const [mime, tag] of [["image/png", "img"], ["video/mp4", "video"], ["audio/mpeg", "audio"]]) {
    const file = { ...brief, mime: mime!, name: "Evidence", artifact_id: `art-${tag}` };
    const sheet = renderAttachmentSheet(document, file, "supervisor");
    const resolve = vi.fn(async () => ({ ok: true as const, view: { artifact_id: file.artifact_id, url: "https://files.example/evidence" } }));
    const root = inlineAttachment(sheet, file, resolve); document.body.append(root);
    const viewport = root.querySelector<HTMLElement>(".rich-media");
    expect(viewport).not.toBeNull();
    expect(viewport!.style.aspectRatio || viewport!.style.height).toBeTruthy();
    await vi.waitFor(() => expect(root.querySelector(tag!)).not.toBeNull());
    expect(root.querySelector(tag!)?.getAttribute("src")).toBe("https://files.example/evidence");
    expect(root.querySelector("a.sheet")).toBe(sheet);
    root.remove(); document.body.append(root);
    await Promise.resolve();
    expect(resolve).toHaveBeenCalledTimes(1);
    if (tag === "video" || tag === "audio") { expect(root.querySelector(tag)?.hasAttribute("controls")).toBe(true); expect(root.querySelector(tag)?.hasAttribute("autoplay")).toBe(false); }
    root.remove();
  }
  const root = inlineAttachment(renderAttachmentSheet(document, brief), brief, async () => ({ ok: true, view: { artifact_id: brief.artifact_id, url: "javascript:alert(1)" } }));
  document.body.append(root);
  await vi.waitFor(() => expect(root.textContent).toContain("Preview unavailable"));
  expect(root.querySelector("canvas")).toBeNull(); root.remove();
});

it("renews a preview once on 401 or expiry, and never retries a refusal in a loop", async () => {
  const realNow = Date.now.bind(Date);
  let clockOffset = 0;
  const clock = vi.spyOn(Date, "now").mockImplementation(() => realNow() + clockOffset);
  try {
    for (const reason of ["401", "expired", "403"]) {
      let cancelled = 0;
      const probe = vi.fn(async (_url: unknown, init?: RequestInit) => {
        expect((init?.headers as Record<string, string>).Range).toBe("bytes=0-0");
        return new Response(new ReadableStream({ start(controller) { controller.enqueue(new Uint8Array([0])); }, cancel() { cancelled += 1; } }), { status: reason === "403" ? 403 : 401 });
      });
      vi.stubGlobal("fetch", probe);
      let tickets = 0;
      const file = { ...brief, artifact_id: `art-refresh-${reason}`, mime: "image/png" };
      const resolve = vi.fn(async () => ({ ok: true as const, view: { artifact_id: file.artifact_id, url: `https://files.example/content?t=ticket-${++tickets}`, expires_at: new Date(Date.now() + 5_000).toISOString() } }));
      const root = inlineAttachment(renderAttachmentSheet(document, file), file, resolve); document.body.append(root);
      try {
        await vi.waitFor(() => expect(root.querySelector("img")).not.toBeNull());
        if (reason === "expired") clockOffset += 6_000;
        root.querySelector("img")!.dispatchEvent(new Event("error"));
        if (reason !== "403") {
          await vi.waitFor(() => expect(root.querySelector("img")?.getAttribute("src")).toContain("ticket-2"));
          root.querySelector("img")!.dispatchEvent(new Event("error"));
        }
        await vi.waitFor(() => expect(root.textContent).toContain("Preview unavailable"));
        expect(resolve).toHaveBeenCalledTimes(reason === "403" ? 1 : 2);
        expect(probe).toHaveBeenCalledTimes(reason === "expired" ? 0 : 1);
        expect(cancelled).toBe(reason === "expired" ? 0 : 1);
      } finally { root.remove(); }
    }
  } finally { clock.mockRestore(); vi.unstubAllGlobals(); }
});

it("retries a disconnected preview when its machine reconnects, preserving cached media", async () => {
  let live = false;
  const file = { ...brief, artifact_id: "art-reconnected", mime: "image/png" };
  const resolve = vi.fn(async () => live ? { ok: true as const, view: { artifact_id: file.artifact_id, url: "https://files.example/reconnected" } } : undefined);
  const root = inlineAttachment(renderAttachmentSheet(document, file), file, resolve, { machineId: "atlas", available: () => live });
  document.body.append(root);
  try {
    await vi.waitFor(() => expect(root.textContent).toContain("Preview unavailable"));
    root.querySelector(".rich-media p")!.textContent = "Translated fallback";
    root.remove(); document.body.append(root);
    retryAttachmentPreviews(document, "studio");
    await Promise.resolve(); expect(resolve).toHaveBeenCalledTimes(1);
    live = true; retryAttachmentPreviews(document, "atlas");
    await vi.waitFor(() => expect(root.querySelector("img")?.getAttribute("src")).toBe("https://files.example/reconnected"));
    const image = root.querySelector("img");
    retryAttachmentPreviews(document, "atlas");
    root.remove(); document.body.append(root);
    await Promise.resolve();
    expect(resolve).toHaveBeenCalledTimes(2); expect(root.querySelector("img")).toBe(image);
  } finally { root.remove(); }
});
