/**
 * Inline images in supervisor replies (cas-85bfb).
 *
 * `![alt](artifact:<id>)` names a published artifact; `![alt](<path>)` names a
 * PNG, JPEG, WebP or GIF file the supervisor saved under its project's
 * artifacts directory. Both resolve only through the publishing machine's
 * session media ticket, which re-proves the file on the machine; a remote URL
 * or any other scheme stays literal text, and a reference the machine refuses
 * falls back to that same text.
 */
import type { ArtifactViewResult } from "./artifact-open";

const IMAGE_PATH = /\.(png|jpe?g|webp|gif)$/i;

function base64Url(text: string): string {
  let binary = "";
  for (const byte of new TextEncoder().encode(text)) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

/** The machine-side reference an image URL names, or undefined to keep it as text. */
export function imageReference(url: string): string | undefined {
  const value = url.trim();
  if (!value || /[\s<>"]/.test(value) || value.length > 2_048) return undefined;
  const artifact = /^artifact:([A-Za-z0-9_-]{1,200})$/.exec(value);
  if (artifact) return artifact[1];
  // Any scheme (https:, data:, file:, javascript:) or a protocol-relative URL is never fetched.
  if (/^[a-z][a-z0-9+.-]*:/i.test(value) || value.startsWith("//") || !IMAGE_PATH.test(value)) return undefined;
  return `local-${base64Url(value)}`;
}

/** The placeholder the Markdown renderer emits; nothing loads until a machine resolves it. */
export function imagePlaceholder(document: Document, alt: string, reference: string, source: string): HTMLElement {
  const button = document.createElement("button");
  button.type = "button"; button.className = "markdown-image"; button.dataset.imageRef = reference; button.dataset.imageSource = source;
  button.setAttribute("aria-label", alt ? `${alt}. Enlarge image` : "Enlarge image");
  const image = document.createElement("img");
  image.alt = alt; image.setAttribute("loading", "lazy"); image.setAttribute("decoding", "async"); image.referrerPolicy = "no-referrer"; image.draggable = false;
  button.append(image);
  return button;
}

/** Point each placeholder at its machine-issued URL; a refusal becomes the reply's own text. */
export function hydrateReplyImages(nodes: readonly HTMLElement[], resolve: (reference: string) => Promise<ArtifactViewResult | undefined>): void {
  const buttons = nodes.flatMap(node => [...(node.matches(".markdown-image") ? [node] : []), ...node.querySelectorAll<HTMLButtonElement>(".markdown-image")]);
  const asText = (button: HTMLElement) => button.replaceWith(button.ownerDocument.createTextNode(button.dataset.imageSource ?? ""));
  for (const button of buttons) {
    const reference = button.dataset.imageRef!;
    const image = button.querySelector("img")!;
    button.dataset.state = "loading";
    image.onload = () => { button.dataset.state = "ready"; };
    image.onerror = () => asText(button);
    void resolve(reference).then(result => {
      if (result?.ok) image.src = result.view.url; else asText(button);
    }, () => asText(button));
    button.onclick = event => {
      if (button.dataset.state !== "ready") return;
      event.preventDefault();
      const gallery = buttons.filter(item => item.isConnected && item.dataset.state === "ready");
      void import("./image-viewer").then(({ openImageViewer }) => openImageViewer(button, gallery.map(item => ({
        file: { artifact_id: item.dataset.imageRef!, name: item.querySelector("img")!.alt || "Image", mime: "image/*", size_bytes: 0, sha256: "" },
        fetchView: () => resolve(item.dataset.imageRef!),
      })), gallery.indexOf(button)));
    };
  }
}
