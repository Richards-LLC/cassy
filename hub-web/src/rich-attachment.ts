import type { ArtifactViewResult } from "./artifact-open";
import type { ArtifactRef } from "./types";

type Preview = HTMLElement & { loadPreview?: () => Promise<void> };

/** Connection callbacks let cached conversation nodes leave and re-enter safely. */
function previewElement(document: Document): Preview {
  const view = document.defaultView as (Window & typeof globalThis) | null;
  if (!view) return document.createElement("div");
  const tag = "commander-media-preview";
  if (!view.customElements.get(tag)) view.customElements.define(tag, class extends view.HTMLElement {
    loadPreview?: () => Promise<void>;
    private observer?: IntersectionObserver;
    private loading = false;
    connectedCallback(): void {
      if (this.querySelector("img,video,audio,canvas") || this.textContent?.startsWith("Preview unavailable")) return;
      const load = async () => {
        this.observer?.disconnect(); this.observer = undefined;
        if (this.loading || !this.isConnected) return;
        this.loading = true;
        try { await this.loadPreview?.(); }
        finally { this.loading = false; if (this.isConnected && this.textContent === "Loading preview…") this.connectedCallback(); }
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
export function inlineAttachment(sheet: HTMLElement, file: ArtifactRef, fetchView: () => Promise<ArtifactViewResult | undefined>): HTMLElement {
  const mime = file.mime.split(";")[0]!.trim().toLowerCase();
  if (!/^(image\/(png|jpeg|gif|webp|avif)|video\/(mp4|webm|ogg)|audio\/(mpeg|mp4|ogg|wav|webm)|application\/pdf)$/.test(mime)) return sheet;
  const document = sheet.ownerDocument;
  const root = document.createElement("div"); root.className = "rich-attachment"; root.style.width = "640px"; root.style.maxWidth = "100%"; root.style.minWidth = "0";
  const viewport = previewElement(document); viewport.className = "rich-media";
  viewport.style.width = "100%"; viewport.style.display = "grid"; viewport.style.overflow = "hidden";
  if (mime.startsWith("audio/")) viewport.style.height = "64px";
  else viewport.style.aspectRatio = mime === "application/pdf" ? "4 / 5" : "16 / 9";
  viewport.setAttribute("aria-label", `${file.name} preview`);
  const label = document.createElement("p"); label.textContent = "Loading preview…"; viewport.append(label);
  viewport.loadPreview = async () => {
    try {
      const { loadMedia } = await import("./rich-media");
      await loadMedia(viewport, file, fetchView);
    } catch { label.textContent = "Preview unavailable. Open the file below."; }
  };
  root.append(viewport, sheet);
  return root;
}
