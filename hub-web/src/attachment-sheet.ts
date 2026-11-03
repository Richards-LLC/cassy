import { registerTurnRenderer, type TurnRenderContext } from "./conversation-view";
import { joinSpoken } from "./spoken-names";
import type { ArtifactRef, OperatorReply } from "./types";

/**
 * Pebble 4 attachment (cas-3800): treatment A from
 * docs/design/hub-messaging/round-3/pebble.css. A supervisor's artifact is
 * not a bubble — it is a sheet laid on the thread, dog-eared at the top right,
 * carrying a type mark, the file name and its size. The whole sheet is the
 * existing `#artifact:<id>` link, so a tap opens the artifact exactly as the
 * plain link row did.
 *
 * The ear is two complementary clip-path triangles on ::before/::after: a
 * canvas-toned cut and a fold-toned flap, 16px each. No overflow:hidden is
 * involved — the cut is painted, not clipped, so nothing inside the sheet can
 * be lost to it.
 */

const TYPE_MARKS: ReadonlyArray<[RegExp, string]> = [
  [/^application\/pdf$/, "PDF"],
  [/^text\/html$/, "HTML"],
  [/^text\/markdown$/, "MD"],
  [/^text\/csv$/, "CSV"],
  [/^application\/json$/, "JSON"],
  [/^text\/plain$/, "TXT"],
  [/^image\/svg/, "SVG"],
  [/^image\/png$/, "PNG"],
  [/^image\/jpe?g$/, "JPG"],
  [/^application\/zip$/, "ZIP"],
];

/** Short mark for the plate: the mime when it is a known kind, else the extension, else FILE. */
export function attachmentTypeMark(mime: string, name: string): string {
  const type = mime.split(";")[0]!.trim().toLowerCase();
  for (const [pattern, mark] of TYPE_MARKS) if (pattern.test(type)) return mark;
  const extension = name.match(/\.([a-z0-9]{1,5})$/i)?.[1];
  if (extension) return extension.toUpperCase();
  const [, subtype] = type.split("/");
  const tail = subtype?.replace(/^(x-|vnd\.)/, "").split(/[.+-]/).pop();
  return tail && tail.length <= 5 ? tail.toUpperCase() : "FILE";
}

/** Human size: bytes below 1 KB, one decimal above, binary units. */
export function attachmentSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "";
  if (bytes < 1024) return `${Math.round(bytes)} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024; let unit = 0;
  while (value >= 1024 && unit < units.length - 1) { value /= 1024; unit += 1; }
  return `${value < 10 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`;
}

export const ARTIFACT_LINK_PREFIX = "#artifact:";

/**
 * What a file card says after it failed to open (journey F6): the reason sits
 * on the card the operator pressed, not in a toast at the top of the thread.
 * Kept per artifact so a thread repaint that rebuilds the card keeps it.
 */
interface AttachmentNote {
  readonly text: string;
  /** The machine the file came from. */
  readonly machineId?: string;
  /**
   * Says something about the connection or Cloud right now ("couldn't
   * reach", "wait a minute"), not about the file itself. It is cleared when
   * the machine's connection comes back, so it never sits beside a Live
   * header (cas-c808 QA F01).
   */
  readonly transient: boolean;
  /**
   * The note in the words the connection calls for now. Set for a note that
   * depends on it ("is connected but didn't send"), so a card never keeps
   * saying Connected while the header says Reconnecting (journey F28).
   */
  readonly restate?: () => string;
  /** Last written quietly by a restate, so a rebuilt card keeps it quiet too. */
  readonly quiet?: boolean;
}
const attachmentNotes = new Map<string, AttachmentNote>();

/**
 * `announce: false` rewrites a note without speaking it: the connection
 * change that caused it is already announced by the banner, once (journey
 * F42), and the card's own name still carries the new words on focus.
 */
function applyAttachmentNote(sheet: HTMLAnchorElement, note: string | undefined, announce = true): void {
  const text = sheet.querySelector<HTMLElement>(".ftext");
  let line = sheet.querySelector<HTMLElement>(".fnote");
  if (note === undefined) {
    line?.remove();
  } else if (text) {
    const created = !line;
    if (!line) { line = sheet.ownerDocument.createElement("span"); line.className = "fnote"; }
    // The card sits in the thread's role=log, which is itself a polite live
    // region, so dropping role=status alone still let the log speak the new
    // words (cas-c945 QA F01). aria-live=off takes the note out of both.
    if (announce) { line.setAttribute("role", "status"); line.removeAttribute("aria-live"); }
    else { line.removeAttribute("role"); line.setAttribute("aria-live", "off"); }
    if (created) text.append(line);
    if (line.textContent !== note) line.textContent = note;
  }
  // The link's own name is what a screen reader reads on focus, so the note is part of it.
  const label = sheet.dataset.label;
  if (label) sheet.setAttribute("aria-label", note === undefined ? label : `${label}. ${note}`);
}

/**
 * Say (or, with undefined, clear) why a file did not open, on every card for
 * that artifact. Returns how many cards now carry it; zero means the file is
 * not on screen as a card and the caller should say it elsewhere.
 */
export function setAttachmentNote(root: ParentNode, artifactId: string, note: string | undefined, options: { machineId?: string; transient?: boolean; restate?: () => string } = {}): number {
  if (note === undefined) attachmentNotes.delete(artifactId);
  else attachmentNotes.set(artifactId, { text: note, transient: options.transient ?? false, ...(options.machineId ? { machineId: options.machineId } : {}), ...(options.restate ? { restate: options.restate } : {}) });
  const sheets = [...root.querySelectorAll<HTMLAnchorElement>("a.sheet[data-artifact-id]")].filter((sheet) => sheet.dataset.artifactId === artifactId);
  for (const sheet of sheets) applyAttachmentNote(sheet, note);
  return sheets.length;
}

/**
 * The machine's connection is back: every note about reaching it or Cloud
 * right now is out of date, so it leaves the cards. A note about the file
 * itself ("only saved on …", "isn't available any more") stays. Returns how
 * many notes were cleared.
 */
export function clearTransientAttachmentNotes(root: ParentNode, machineId: string): number {
  let cleared = 0;
  for (const [artifactId, note] of [...attachmentNotes]) {
    if (!note.transient || note.machineId !== machineId) continue;
    setAttachmentNote(root, artifactId, undefined);
    cleared += 1;
  }
  return cleared;
}

/**
 * The machine's connection changed: every note for it whose words depend on
 * the connection is said again in the words it calls for now (journey F28),
 * quietly, since the banner announces the change itself. Returns how many
 * notes changed.
 */
export function restateAttachmentNotes(root: ParentNode, machineId: string): number {
  let changed = 0;
  for (const [artifactId, note] of [...attachmentNotes]) {
    if (!note.restate || note.machineId !== machineId) continue;
    const text = note.restate();
    if (text === note.text) continue;
    attachmentNotes.set(artifactId, { ...note, text, quiet: true });
    for (const sheet of root.querySelectorAll<HTMLAnchorElement>("a.sheet[data-artifact-id]")) {
      if (sheet.dataset.artifactId === artifactId) applyAttachmentNote(sheet, text, false);
    }
    changed += 1;
  }
  return changed;
}

export function artifactHref(attachment: ArtifactRef): string {
  return `${ARTIFACT_LINK_PREFIX}${encodeURIComponent(attachment.artifact_id)}`;
}

/** Build one sheet for one artifact. */
export function renderAttachmentSheet(document: Document, attachment: ArtifactRef, supervisor?: string): HTMLAnchorElement {
  const sheet = document.createElement("a");
  sheet.className = "sheet";
  sheet.href = artifactHref(attachment);
  sheet.dataset.artifactId = attachment.artifact_id;
  sheet.dataset.mime = attachment.mime;
  const mark = attachmentTypeMark(attachment.mime, attachment.name);
  const size = attachmentSize(attachment.size_bytes);
  // cas-d8a5 (journey F32): from "the cas-src supervisor", not its codename.
  const label = `${joinSpoken([attachment.name, mark, size, supervisor && `from ${supervisor}`])}. Open`;
  sheet.dataset.label = label;
  sheet.setAttribute("aria-label", label);
  sheet.title = [attachment.mime, attachment.size_bytes >= 0 ? `${attachment.size_bytes} bytes` : "", attachment.sha256 ? `sha256 ${attachment.sha256.slice(0, 12)}…` : ""].filter(Boolean).join(" · ");
  const plate = document.createElement("span"); plate.className = "plate"; plate.textContent = mark; plate.setAttribute("aria-hidden", "true");
  const text = document.createElement("span"); text.className = "ftext";
  const name = document.createElement("span"); name.className = "fname"; name.textContent = attachment.name;
  const sub = document.createElement("span"); sub.className = "fsub";
  if (size) sub.append(size);
  text.append(name, sub);
  sheet.append(plate, text);
  const note = attachmentNotes.get(attachment.artifact_id);
  if (note !== undefined) applyAttachmentNote(sheet, note.text, !note.quiet);
  return sheet;
}

/**
 * Where an HTML attachment's sandboxed page URL comes from (cas-3aff): the
 * machine that published it mints a short-lived ticket URL on its own origin.
 * Undefined means the page can't be shown here right now.
 */
export type VisualPageSource = (artifactId: string, context: TurnRenderContext) => Promise<string | undefined>;

let visualPageSource: VisualPageSource | undefined;

/** Whether an attachment is a page Commander shows inline. */
export function isVisualPage(attachment: Pick<ArtifactRef, "mime">): boolean {
  return attachment.mime.split(";")[0]!.trim().toLowerCase() === "text/html";
}

/**
 * Visual reply (cas-3aff): the sheet stays the file's card and its tap opens
 * the page in a tab; beneath it the page itself, in a frame the browser locks
 * down twice — `sandbox=""` here (no script, no same-origin, no popups, no top
 * navigation) and the machine's CSP sandbox on the response. The page's
 * markup never enters this document: the frame only ever gets a URL on the
 * paired machine's origin. Threat model:
 * docs/design/hub-web/visual-replies-threat-model.md.
 */
export function renderVisualPage(document: Document, attachment: ArtifactRef, sheet: HTMLAnchorElement, pageUrl: () => Promise<string | undefined>): HTMLElement {
  const figure = document.createElement("figure");
  figure.className = "visual-page";
  figure.dataset.artifactId = attachment.artifact_id;
  const frame = document.createElement("div");
  frame.className = "visual-frame";
  const status = document.createElement("p");
  status.className = "visual-status";
  status.textContent = "Loading the page…";
  frame.append(status);
  figure.append(sheet, frame);
  let started = false;
  const load = () => {
    if (started) return;
    started = true;
    void pageUrl().then((url) => {
      if (!url) { status.textContent = "This page can't be shown here. Open the file above to try it in a tab."; return; }
      const iframe = document.createElement("iframe");
      iframe.setAttribute("sandbox", "");
      iframe.setAttribute("referrerpolicy", "no-referrer");
      iframe.setAttribute("allow", "");
      iframe.title = `${attachment.name}, a page from the supervisor`;
      iframe.src = url;
      status.remove();
      frame.append(iframe);
    }, () => { status.textContent = "This page can't be shown here. Open the file above to try it in a tab."; });
  };
  // Mint the ticket only once the page is near the screen, so a long
  // history full of reports doesn't open every one of them.
  const view = document.defaultView;
  if (view && "IntersectionObserver" in view) {
    const observer = new view.IntersectionObserver((entries) => {
      if (entries.some((entry) => entry.isIntersecting)) { observer.disconnect(); load(); }
    }, { rootMargin: "400px 0px" });
    observer.observe(figure);
  } else {
    load();
  }
  return figure;
}

/** The `attachment` kind renderer: one sheet per artifact, in place of the link row. */
export const attachmentSheetRenderer = (_reply: OperatorReply, context: TurnRenderContext): HTMLElement => {
  const attachment = context.attachment;
  if (!attachment) throw new Error("attachment renderer called without an attachment");
  // cas-d8a5 (journey F32): "from the cas-src supervisor" when the project is known.
  const sheet = renderAttachmentSheet(context.document, attachment, context.turn.event.kind === "send" ? "you" : context.spokenSupervisor ? `the ${context.spokenSupervisor}` : context.supervisor);
  const pages = visualPageSource;
  return pages && isVisualPage(attachment) ? renderVisualPage(context.document, attachment, sheet, () => pages(attachment.artifact_id, context)) : sheet;
};

/**
 * Register the sheet with the thread; returns the unregister function.
 * `preview` may wrap a media sheet in its inline preview (cas-9c66). With a
 * page source, HTML attachments also show their page inline (cas-3aff).
 */
export function installAttachmentSheet(preview?: (sheet: HTMLElement, file: ArtifactRef, context: TurnRenderContext) => HTMLElement, pages?: VisualPageSource): () => void {
  visualPageSource = pages;
  const unregister = registerTurnRenderer("attachment", (reply, context) => {
    const sheet = attachmentSheetRenderer(reply, context);
    const node = preview ? preview(sheet, context.attachment!, context) : sheet;
    node.dataset.attachmentSignature = JSON.stringify(context.attachment);
    return node;
  });
  return () => { unregister(); if (visualPageSource === pages) visualPageSource = undefined; };
}
