import { registerTurnRenderer, type TurnRenderContext } from "./conversation-view";
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
  sheet.setAttribute("aria-label", `${attachment.name}, ${mark}${size ? `, ${size}` : ""}${supervisor ? `, from ${supervisor}` : ""}. Open`);
  sheet.title = `${attachment.mime} · ${attachment.size_bytes} bytes · sha256 ${attachment.sha256.slice(0, 12)}…`;
  const plate = document.createElement("span"); plate.className = "plate"; plate.textContent = mark; plate.setAttribute("aria-hidden", "true");
  const text = document.createElement("span"); text.className = "ftext";
  const name = document.createElement("span"); name.className = "fname"; name.textContent = attachment.name;
  const sub = document.createElement("span"); sub.className = "fsub";
  if (size) sub.append(size);
  text.append(name, sub);
  sheet.append(plate, text);
  return sheet;
}

/** The `attachment` kind renderer: one sheet per artifact, in place of the link row. */
export const attachmentSheetRenderer = (_reply: OperatorReply, context: TurnRenderContext): HTMLElement => {
  const attachment = context.attachment;
  if (!attachment) throw new Error("attachment renderer called without an attachment");
  return renderAttachmentSheet(context.document, attachment, context.supervisor);
};

/** Register the sheet with the thread; returns the unregister function. */
export function installAttachmentSheet(): () => void {
  return registerTurnRenderer("attachment", attachmentSheetRenderer);
}
