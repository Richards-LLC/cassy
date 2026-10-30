import "./image-viewer.css";
import type { ArtifactViewResult } from "./artifact-open";
import type { ArtifactRef } from "./types";

export interface GalleryImage {
  file: ArtifactRef;
  fetchView: () => Promise<ArtifactViewResult | undefined>;
}

const viewers = new WeakMap<Document, () => void>();
const MAX_ZOOM = 8;

/** Inspect one turn's images. URLs and interaction state live only in this dialog. */
export function openImageViewer(opener: HTMLElement, images: readonly GalleryImage[], selected: number): void {
  if (!images.length) return;
  const document = opener.ownerDocument;
  viewers.get(document)?.();
  const scroll = [] as Array<{ node: HTMLElement; top: number; left: number }>;
  for (let node = opener.parentElement; node; node = node.parentElement) scroll.push({ node, top: node.scrollTop, left: node.scrollLeft });
  const dialog = document.createElement("dialog"); dialog.className = "image-viewer";
  dialog.setAttribute("aria-label", "Image viewer");
  dialog.innerHTML = `<header><div><h2></h2><p class="image-viewer-position" aria-live="polite"></p></div><button type="button" aria-label="Close image viewer">Close</button></header>
    <div class="image-viewer-canvas" role="region" aria-label="Image inspection" aria-describedby="image-viewer-help" tabindex="0"><img alt="" draggable="false" referrerpolicy="no-referrer"></div>
    <p class="image-viewer-status" role="status"></p>
    <footer><div class="image-viewer-gallery"><button type="button" aria-label="Previous image">← Previous</button><button type="button" aria-label="Next image">Next →</button></div><div class="image-viewer-zoom"><button type="button" aria-label="Zoom out">−</button><output aria-label="Image zoom">Fit</output><button type="button" aria-label="Zoom in">+</button><button type="button" aria-label="Fit image">Fit</button></div></footer>
    <p id="image-viewer-help">← → images · + − zoom · Shift + arrows pan · 0 fit · Escape close</p><button type="button" class="image-viewer-retry" aria-label="Try image again" hidden>Try again</button>`;
  const button = (name: string) => dialog.querySelector<HTMLButtonElement>(`button[aria-label="${name}"]`)!;
  const canvas = dialog.querySelector<HTMLElement>(".image-viewer-canvas")!;
  const image = dialog.querySelector<HTMLImageElement>("img")!;
  const status = dialog.querySelector<HTMLElement>('[role="status"]')!;
  const output = dialog.querySelector("output")!;
  let index = Math.max(0, Math.min(images.length - 1, selected));
  let generation = 0;
  let zoom = 1, x = 0, y = 0;
  let closed = false;
  const pointers = new Map<number, { x: number; y: number }>();
  const ready = () => dialog.dataset.state === "ready";
  const draw = () => {
    const bounds = canvas.getBoundingClientRect();
    const fit = image.naturalWidth && image.naturalHeight ? Math.min(1, bounds.width / image.naturalWidth, bounds.height / image.naturalHeight) : 1;
    const maxX = Math.max(0, ((image.naturalWidth || bounds.width) * fit * zoom - bounds.width) / 2);
    const maxY = Math.max(0, ((image.naturalHeight || bounds.height) * fit * zoom - bounds.height) / 2);
    x = Math.max(-maxX, Math.min(maxX, x)); y = Math.max(-maxY, Math.min(maxY, y));
    image.style.transform = `translate(${x}px, ${y}px) scale(${zoom})`;
    output.textContent = zoom === 1 ? "Fit" : `${zoom.toFixed(1)}×`;
    canvas.dataset.zoomed = String(zoom > 1);
    button("Zoom out").disabled = !ready() || zoom <= 1;
    button("Zoom in").disabled = !ready() || zoom >= MAX_ZOOM;
    button("Fit image").disabled = !ready();
  };
  const setZoom = (value: number) => {
    if (!ready()) return;
    zoom = Math.max(1, Math.min(MAX_ZOOM, value));
    if (zoom === 1) { x = 0; y = 0; }
    draw();
  };
  const load = async () => {
    const request = ++generation;
    const entry = images[index]!;
    pointers.clear(); zoom = 1; x = 0; y = 0;
    dialog.dataset.state = "loading"; image.hidden = true; image.removeAttribute("src"); image.alt = entry.file.name;
    dialog.querySelector("h2")!.textContent = entry.file.name;
    dialog.querySelector(".image-viewer-position")!.textContent = `${index + 1} of ${images.length}`;
    status.textContent = `Loading ${entry.file.name}…`;
    button("Previous image").disabled = index === 0;
    button("Next image").disabled = index === images.length - 1;
    button("Try image again").hidden = true;
    draw();
    const current = () => !closed && request === generation;
    const fail = () => {
      if (!current()) return;
      dialog.dataset.state = "failed"; image.hidden = true;
      status.textContent = `Couldn't load ${entry.file.name}. Try again or choose another image.`;
      button("Try image again").hidden = false; draw();
    };
    try {
      const result = await entry.fetchView();
      if (!current()) return;
      if (!result?.ok || result.view.artifact_id !== entry.file.artifact_id) { fail(); return; }
      const url = new URL(result.view.url);
      const loopback = url.protocol === "http:" && ["localhost", "127.0.0.1", "[::1]"].includes(url.hostname);
      if (url.protocol !== "https:" && !loopback || url.username || url.password || result.view.expires_at && Date.parse(result.view.expires_at) <= Date.now()) { fail(); return; }
      image.onload = () => {
        if (!current()) return;
        dialog.dataset.state = "ready"; image.hidden = false; status.textContent = ""; draw();
      };
      image.onerror = fail;
      image.src = url.href;
    } catch { fail(); }
  };
  const move = (step: number) => {
    const next = Math.max(0, Math.min(images.length - 1, index + step));
    if (next !== index) { index = next; void load(); }
  };
  const close = () => {
    if (closed) return;
    closed = true; generation++; pointers.clear();
    document.defaultView?.removeEventListener("resize", draw);
    viewers.delete(document);
    if (dialog.open) dialog.close();
    dialog.remove();
    if (opener.isConnected) opener.focus({ preventScroll: true });
    for (const saved of scroll) if (saved.node.isConnected) { saved.node.scrollTop = saved.top; saved.node.scrollLeft = saved.left; }
  };
  viewers.set(document, close);
  button("Close image viewer").onclick = close;
  button("Previous image").onclick = () => move(-1);
  button("Next image").onclick = () => move(1);
  button("Zoom in").onclick = () => setZoom(zoom + 0.5);
  button("Zoom out").onclick = () => setZoom(zoom - 0.5);
  button("Fit image").onclick = () => setZoom(1);
  button("Try image again").onclick = () => { void load(); };
  dialog.addEventListener("close", close);
  dialog.addEventListener("cancel", event => { event.preventDefault(); close(); });
  dialog.addEventListener("keydown", event => {
    if (event.ctrlKey || event.metaKey || event.altKey) return;
    if (event.key === "Escape") { event.preventDefault(); close(); return; }
    if (event.key === "Tab") {
      const stops = [...dialog.querySelectorAll<HTMLElement>('button:not(:disabled), [tabindex="0"]')].filter(node => !node.closest("[hidden]"));
      const first = stops[0]!, last = stops.at(-1)!;
      if (event.shiftKey && document.activeElement === first || !event.shiftKey && document.activeElement === last) {
        event.preventDefault(); (event.shiftKey ? last : first).focus();
      }
      return;
    }
    if (event.shiftKey && event.key.startsWith("Arrow")) {
      event.preventDefault();
      if (ready()) {
        x += event.key === "ArrowLeft" ? 40 : event.key === "ArrowRight" ? -40 : 0;
        y += event.key === "ArrowUp" ? 40 : event.key === "ArrowDown" ? -40 : 0; draw();
      }
    } else if (event.key === "ArrowLeft" || event.key === "ArrowRight") { event.preventDefault(); move(event.key === "ArrowLeft" ? -1 : 1); }
    else if (["+", "=", "-", "0"].includes(event.key)) { event.preventDefault(); setZoom(event.key === "0" ? 1 : zoom + (event.key === "-" ? -0.5 : 0.5)); }
  });
  canvas.addEventListener("wheel", event => { event.preventDefault(); setZoom(zoom * Math.exp(-event.deltaY * 0.002)); }, { passive: false });
  canvas.addEventListener("pointerdown", event => {
    if (!ready() || event.pointerType === "mouse" && event.button !== 0) return;
    pointers.set(event.pointerId, { x: event.clientX, y: event.clientY });
    try { canvas.setPointerCapture(event.pointerId); } catch { /* Synthetic or already released pointer. */ }
  });
  canvas.addEventListener("pointermove", event => {
    const previous = pointers.get(event.pointerId);
    if (!previous) return;
    const before = [...pointers.values()];
    pointers.set(event.pointerId, { x: event.clientX, y: event.clientY });
    const after = [...pointers.values()];
    if (before.length === 2) {
      const distance = (points: typeof before) => Math.hypot(points[1]!.x - points[0]!.x, points[1]!.y - points[0]!.y);
      const oldDistance = distance(before);
      if (oldDistance > 0) setZoom(zoom * distance(after) / oldDistance);
      x += (event.clientX - previous.x) / 2; y += (event.clientY - previous.y) / 2;
    } else if (zoom > 1) { x += event.clientX - previous.x; y += event.clientY - previous.y; }
    draw();
  });
  const release = (event: PointerEvent) => { pointers.delete(event.pointerId); };
  canvas.addEventListener("pointerup", release);
  canvas.addEventListener("pointercancel", release);
  canvas.addEventListener("lostpointercapture", release);
  document.defaultView?.addEventListener("resize", draw);
  document.body.append(dialog); dialog.showModal();
  button("Close image viewer").focus({ preventScroll: true });
  void load();
}
