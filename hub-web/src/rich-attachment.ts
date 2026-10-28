import type { ArtifactViewResult } from "./artifact-open";
import type { ArtifactRef } from "./types";

type Preview = HTMLElement & {
  loadPreview?: () => Promise<void>;
  retryPreview?: () => void;
  machineId?: string;
  available?: () => boolean;
};

/** Connection callbacks let cached conversation nodes leave and re-enter safely. */
function previewElement(document: Document): Preview {
  const view = document.defaultView as (Window & typeof globalThis) | null;
  if (!view) return document.createElement("div");
  const tag = "commander-media-preview";
  if (!view.customElements.get(tag)) view.customElements.define(tag, class extends view.HTMLElement {
    loadPreview?: () => Promise<void>;
    machineId?: string;
    available?: () => boolean;
    private observer?: IntersectionObserver;
    private loading = false;
    private retryAfterLoad = false;
    private get complete(): boolean { return this.dataset.previewState === "ready" || this.dataset.previewState === "failed"; }
    retryPreview(): void {
      if (this.loading) { this.retryAfterLoad = true; return; }
      if (this.dataset.previewState !== "waiting") return;
      this.dataset.previewState = "pending";
      this.connectedCallback();
    }
    connectedCallback(): void {
      this.observer?.disconnect(); this.observer = undefined;
      if (this.complete || this.loading) return;
      if (this.dataset.previewState === "waiting" && this.available?.() === false) return;
      const load = async () => {
        this.observer?.disconnect(); this.observer = undefined;
        if (this.complete || this.loading || !this.isConnected) return;
        this.loading = true;
        this.dataset.previewState = "loading";
        try { await this.loadPreview?.(); }
        finally {
          this.loading = false;
          if (this.retryAfterLoad) { this.retryAfterLoad = false; this.retryPreview(); }
          else if (this.isConnected && this.dataset.previewState === "loading") this.connectedCallback();
        }
      };
      if (!view.IntersectionObserver) { void load(); return; }
      this.observer = new view.IntersectionObserver((entries) => { if (entries.some((entry) => entry.isIntersecting)) void load(); });
      this.observer.observe(this);
    }
    disconnectedCallback(): void { this.observer?.disconnect(); this.observer = undefined; }
  });
  return document.createElement(tag) as Preview;
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
    if (preview.machineId === machineId) preview.retryPreview?.();
  }
}
