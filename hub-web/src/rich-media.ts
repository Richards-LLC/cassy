import "./rich-content.css";
import type { ArtifactViewResult } from "./artifact-open";
import type { ArtifactRef } from "./types";

/** View URLs stay in ephemeral DOM and are never added to the reply journal. */
export async function loadMedia(viewport: HTMLElement, file: ArtifactRef, fetchView: () => Promise<ArtifactViewResult | undefined>): Promise<void> {
  const document = viewport.ownerDocument;
  const fallback = () => {
    const text = document.createElement("p"); text.textContent = "Preview unavailable. Open the file below."; viewport.replaceChildren(text);
  };
  try {
    if (!viewport.isConnected) return;
    const result = await fetchView();
    if (!viewport.isConnected) return;
    if (!result?.ok || result.view.artifact_id !== file.artifact_id) { fallback(); return; }
    const url = new URL(result.view.url);
    if (url.protocol !== "https:" || url.username || url.password) { fallback(); return; }
    const mime = file.mime.split(";")[0]!.trim().toLowerCase();
    if (mime === "application/pdf") {
      const { pdfPreview } = await import("./rich-pdf");
      await pdfPreview(viewport, url.href, file.name); return;
    }
    let media: HTMLImageElement | HTMLVideoElement | HTMLAudioElement;
    if (mime.startsWith("image/")) {
      const image = document.createElement("img"); image.alt = file.name; image.width = 640; image.height = 360; image.loading = "lazy"; image.decoding = "async"; image.referrerPolicy = "no-referrer"; media = image;
    } else {
      const player = document.createElement(mime.startsWith("video/") ? "video" : "audio"); player.controls = true; player.preload = "none";
      if (player instanceof HTMLVideoElement) { player.width = 640; player.height = 360; player.playsInline = true; }
      media = player;
    }
    media.addEventListener("error", fallback, { once: true });
    media.src = url.href;
    viewport.replaceChildren(media);
  } catch { fallback(); }
}
