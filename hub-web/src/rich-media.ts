import "./rich-content.css";
import type { ArtifactViewResult } from "./artifact-open";
import type { ArtifactRef } from "./types";

/** Tickets stay in ephemeral DOM and are never added to the reply journal. */
export async function loadMedia(viewport: HTMLElement, file: ArtifactRef, fetchView: () => Promise<ArtifactViewResult | undefined>): Promise<void> {
  const document = viewport.ownerDocument;
  let refreshed = false;
  let resume: { time: number; playing: boolean } | undefined;
  const fallback = () => {
    const text = document.createElement("p"); text.textContent = "Preview unavailable. Open the file below."; viewport.replaceChildren(text);
  };
  const attempt = async (): Promise<void> => {
    try {
      if (!viewport.isConnected) return;
      const result = await fetchView();
      if (!viewport.isConnected) return;
      if (!result?.ok || result.view.artifact_id !== file.artifact_id) { fallback(); return; }
      const url = new URL(result.view.url);
      const loopback = url.protocol === "http:" && ["localhost", "127.0.0.1", "[::1]"].includes(url.hostname);
      if (url.protocol !== "https:" && !loopback || url.username || url.password) { fallback(); return; }
      let media: HTMLImageElement | HTMLVideoElement | HTMLAudioElement | undefined;
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
          } catch { fallback(); return; }
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
        } catch (error) { await failed(typeof (error as { status?: unknown })?.status === "number" ? (error as { status: number }).status : 0); }
        return;
      }
      if (mime.startsWith("image/")) {
        const image = document.createElement("img"); image.alt = file.name; image.width = 640; image.height = 360; image.loading = "lazy"; image.decoding = "async"; image.referrerPolicy = "no-referrer"; media = image;
      } else {
        const player = document.createElement(mime.startsWith("video/") ? "video" : "audio"); player.controls = true; player.preload = "none";
        player.addEventListener("play", () => { wantedPlayback = true; });
        player.addEventListener("pause", () => { if (!player.error) wantedPlayback = false; });
        const restore = resume; resume = undefined;
        if (restore) player.addEventListener("loadedmetadata", () => {
          if (!viewport.isConnected || player.parentElement !== viewport) return;
          try { if (Number.isFinite(restore.time) && restore.time > 0) player.currentTime = restore.time; } catch { /* A non-seekable clip can still resume. */ }
          if (restore.playing) void player.play().catch(() => { /* Native controls remain available. */ });
        }, { once: true });
        if (player instanceof HTMLVideoElement) { player.width = 640; player.height = 360; player.playsInline = true; }
        media = player;
      }
      media.addEventListener("error", () => { void failed(); }, { once: true });
      media.src = url.href;
      viewport.replaceChildren(media);
    } catch { fallback(); }
  };
  await attempt();
}
