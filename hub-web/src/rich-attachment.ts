import type { ArtifactViewResult } from "./artifact-open";
import type { ArtifactRef } from "./types";

type Preview = HTMLElement & {
  loadPreview?: () => Promise<void>;
  retryPreview?: () => void;
  machineId?: string;
  available?: () => boolean;
};

/** The lifecycle travels with media code, leaving only the reserved box here. */
function installPreview(document: Document): Promise<void> {
  return import("./rich-media").then(({ installMediaPreviews }) => installMediaPreviews(document));
}
function previewElement(document: Document): Preview {
  const viewport = document.createElement("commander-media-preview") as Preview;
  void installPreview(document).catch(() => {
    viewport.dataset.previewState = "waiting";
    viewport.textContent = "Preview unavailable. Open the file below.";
  });
  return viewport;
}

/** Reserve media's final viewport before any lazy code, URL or bytes arrive. */
export function inlineAttachment(sheet: HTMLElement, file: ArtifactRef, fetchView: () => Promise<ArtifactViewResult | undefined>, options: { machineId?: string; available?: () => boolean } = {}): HTMLElement {
  const mime = file.mime.split(";")[0]!.trim().toLowerCase();
  if (!/^(image\/(png|jpeg|gif|webp|avif)|video\/(mp4|webm|ogg)|audio\/(mpeg|mp4|ogg|wav|webm)|application\/pdf)$/.test(mime)) return sheet;
  const document = sheet.ownerDocument;
  const root = document.createElement("div"); root.className = "rich-attachment"; root.style.width = "640px"; root.style.maxWidth = "100%"; root.style.minWidth = "0";
  const viewport = previewElement(document); viewport.className = "rich-media";
  viewport.machineId = options.machineId; viewport.available = options.available;
  viewport.dataset.previewState = "pending";
  viewport.style.width = "100%"; viewport.style.display = "grid"; viewport.style.overflow = "hidden";
  if (mime.startsWith("audio/")) viewport.style.height = "64px";
  else viewport.style.aspectRatio = mime === "application/pdf" ? "4 / 5" : "16 / 9";
  viewport.setAttribute("aria-label", `${file.name} preview`);
  const label = document.createElement("p"); label.textContent = "Loading preview…"; viewport.append(label);
  viewport.loadPreview = async () => {
    try {
      const { loadMedia } = await import("./rich-media");
      await loadMedia(viewport, file, fetchView);
    } catch {
      viewport.dataset.previewState = "waiting";
      label.textContent = "Preview unavailable. Open the file below."; viewport.replaceChildren(label);
    }
  };
  root.append(viewport, sheet);
  return root;
}

/** Only transient failures on this machine retry; loaded media keeps its node. */
export function retryAttachmentPreviews(root: ParentNode, machineId: string): void {
  for (const preview of root.querySelectorAll<Preview>("commander-media-preview")) {
    if (preview.machineId !== machineId) continue;
    if (preview.retryPreview) preview.retryPreview();
    else void installPreview(preview.ownerDocument).catch(() => { /* Next reconnect can retry the chunk. */ });
  }
}
