import { getDocument, GlobalWorkerOptions } from "pdfjs-dist";
import workerUrl from "pdfjs-dist/build/pdf.worker.min.mjs?url";

GlobalWorkerOptions.workerSrc = workerUrl;

/** Render pixels only: no PDF actions, annotations, HTML or scripting layer. */
export async function pdfPreview(viewport: HTMLElement, url: string, name: string): Promise<void> {
  const response = await fetch(url, { credentials: "omit", referrerPolicy: "no-referrer" });
  if (!response.ok) {
    await response.body?.cancel();
    throw Object.assign(new Error("PDF could not be loaded"), { status: response.status });
  }
  const data = new Uint8Array(await response.arrayBuffer());
  if (!viewport.isConnected) return;
  const task = getDocument({ data, useWasm: false, useSystemFonts: true, disableFontFace: true });
  try {
    const pdf = await task.promise;
    const page = await pdf.getPage(1);
    if (!viewport.isConnected) return;
    const original = page.getViewport({ scale: 1 });
    const rendered = page.getViewport({ scale: Math.min(2, 640 / original.width, 800 / original.height) });
    const canvas = viewport.ownerDocument.createElement("canvas"); canvas.width = Math.ceil(rendered.width); canvas.height = Math.ceil(rendered.height);
    await page.render({ canvas, viewport: rendered, annotationMode: 0 }).promise;
    if (viewport.isConnected) {
      const image = viewport.ownerDocument.createElement("img");
      image.width = canvas.width; image.height = canvas.height;
      image.alt = `${name} · PDF preview, page 1 of ${pdf.numPages}. Open the file below to read all pages.`;
      image.src = canvas.toDataURL("image/png");
      viewport.replaceChildren(image);
    }
  } finally { await task.destroy(); }
}
