import "./rich-content.css";
import type { ArtifactViewResult } from "./artifact-open";
import type { ArtifactRef } from "./types";

/** Connection callbacks let cached conversation nodes leave and re-enter safely. */
export function installMediaPreviews(document: Document): void {
  const view = document.defaultView as (Window & typeof globalThis) | null;
  if (!view) return;
  const tag = "commander-media-preview";
  if (!view.customElements.get(tag)) view.customElements.define(tag, class extends view.HTMLElement {
    static get observedAttributes(): string[] { return ["data-preview-state"]; }
    declare previewStateChanged?: (state: string) => void;
    attributeChangedCallback(_name: string, _old: string | null, state: string): void { this.previewStateChanged?.(state); }
    declare loadPreview?: () => Promise<void>;
    declare machineId?: string;
    declare available?: () => boolean;
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
      if (this.dataset.previewState === "waiting") {
        if (this.available?.() === false) return;
        // A cached failed preview is hidden. Restore its reserved box before
        // observing, so re-entry after an offscreen reconnect can intersect.
        this.dataset.previewState = "pending";
      }
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
}

/** Tickets stay in ephemeral DOM and are never added to the reply journal. */
export async function loadMedia(viewport: HTMLElement, file: ArtifactRef, fetchView: () => Promise<ArtifactViewResult | undefined>): Promise<void> {
  const document = viewport.ownerDocument;
  let refreshed = false;
  let resume: { time: number; playing: boolean } | undefined;
  const fallback = (waiting = false) => {
    viewport.dataset.previewState = waiting ? "waiting" : "failed";
    const text = document.createElement("p"); text.textContent = "Preview unavailable"; viewport.replaceChildren(text);
  };
  const attempt = async (): Promise<void> => {
    try {
      if (!viewport.isConnected) return;
      const result = await fetchView();
      if (!viewport.isConnected) return;
      if (!result) { fallback(true); return; }
      if (!result.ok) { fallback(result.status === 408 || result.status === 429 || result.status >= 500); return; }
      if (result.view.artifact_id !== file.artifact_id) { fallback(); return; }
      const url = new URL(result.view.url);
      const loopback = url.protocol === "http:" && ["localhost", "127.0.0.1", "[::1]"].includes(url.hostname);
      if (url.protocol !== "https:" && !loopback || url.username || url.password) { fallback(); return; }
      let media: HTMLImageElement | HTMLVideoElement | HTMLAudioElement | undefined;
      let restart: HTMLMediaElement | undefined;
      let wantedPlayback = false;
      const expired = () => Boolean(result.view.expires_at && Date.parse(result.view.expires_at) <= Date.now());
      const failed = async (status?: number): Promise<void> => {
        if (refreshed || !viewport.isConnected) { fallback(); return; }
        let unauthorized = status === 401;
        if (!expired() && status === undefined) {
          try {
            const probe = await fetch(url, { headers: { Range: "bytes=0-0" }, credentials: "omit", referrerPolicy: "no-referrer", cache: "no-store" });
            unauthorized = probe.status === 401;
            await probe.body?.cancel();
          } catch { fallback(true); return; }
        }
        if (!viewport.isConnected) { fallback(); return; }
        if (expired() || unauthorized) {
          if (media instanceof HTMLMediaElement) resume = { time: media.currentTime, playing: wantedPlayback };
          refreshed = true; await attempt();
        }
        else fallback();
      };
      if (expired()) { await failed(401); return; }
      const mime = file.mime.split(";")[0]!.trim().toLowerCase();
      if (mime === "application/pdf") {
        try {
          const { pdfPreview } = await import("./rich-pdf");
          await pdfPreview(viewport, url.href, file.name);
          if (viewport.isConnected) viewport.dataset.previewState = "ready";
        } catch (error) { await failed(typeof (error as { status?: unknown })?.status === "number" ? (error as { status: number }).status : 0); }
        return;
      }
      if (mime.startsWith("image/")) {
        const image = document.createElement("img"); image.alt = file.name; image.width = viewport.classList.contains("rich-thumbnail") ? 48 : 640; image.height = viewport.classList.contains("rich-thumbnail") ? 48 : 360; image.loading = "lazy"; image.decoding = "async"; image.referrerPolicy = "no-referrer"; media = image;
      } else {
        const player = document.createElement(mime.startsWith("video/") ? "video" : "audio"); player.controls = true; player.preload = "none";
        player.addEventListener("play", () => { wantedPlayback = true; });
        player.addEventListener("pause", () => { if (!player.error) wantedPlayback = false; });
        const restore = resume; resume = undefined;
        if (restore) player.addEventListener("loadedmetadata", () => {
          if (!viewport.isConnected || player.parentElement !== viewport) return;
          try { if (Number.isFinite(restore.time) && restore.time > 0) player.currentTime = restore.time; } catch { /* A non-seekable clip can still resume. */ }
          player.preload = "none";
        }, { once: true });
        if (restore?.playing) restart = player;
        else if (restore && restore.time > 0) player.preload = "metadata";
        if (player instanceof HTMLVideoElement) { player.width = 640; player.height = 360; player.playsInline = true; }
        media = player;
      }
      media.addEventListener("error", () => { void failed(); }, { once: true });
      media.src = url.href;
      viewport.replaceChildren(media);
      viewport.dataset.previewState = "ready";
      if (restart) void restart.play().catch(() => { /* Native controls remain available. */ });
    } catch { fallback(true); }
  };
  await attempt();
}
