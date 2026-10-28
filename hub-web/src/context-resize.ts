import { PHONE_MEDIA_QUERY } from "./viewport";

const WIDTH_KEY = "cassy.context-width";
interface ResizeOptions {
  width(): number;
  phone(): boolean;
  storage: Pick<Storage, "getItem" | "setItem">;
}
const bindings = new WeakMap<HTMLElement, () => void>();

/** Width is device-local preference. The viewport constrains it without overwriting it. */
export function syncContextResize(shell: HTMLElement, options?: ResizeOptions): void {
  const bound = bindings.get(shell);
  if (bound) { bound(); return; }
  const win = shell.ownerDocument.defaultView!;
  const environment = options ?? { width: () => shell.clientWidth || win.innerWidth, phone: () => win.matchMedia?.(PHONE_MEDIA_QUERY).matches ?? false, storage: {
    getItem: (key: string) => win.localStorage.getItem(key), setItem: (key: string, value: string) => win.localStorage.setItem(key, value),
  } };
  let preferred = 320;
  try { const saved = Number(environment.storage.getItem(WIDTH_KEY)); if (Number.isFinite(saved) && saved >= 240) preferred = Math.min(480, saved); } catch { /* Storage can be denied; interaction remains available. */ }
  const grip = shell.ownerDocument.createElement("div");
  grip.className = "context-resize"; grip.tabIndex = 0;
  grip.setAttribute("role", "separator"); grip.setAttribute("aria-label", "Resize tasks and progress"); grip.setAttribute("aria-orientation", "vertical");
  grip.title = "Drag to resize. Left widens, Right narrows; Home and End set limits.";
  const maximum = () => Math.max(240, Math.min(480, environment.width() - 374 - 400));
  const current = () => Math.min(maximum(), Math.max(240, preferred));
  const sync = () => {
    grip.hidden = environment.phone();
    shell.style.setProperty("--worker-collapsed-width", `${current()}px`);
    grip.setAttribute("aria-valuemin", "240"); grip.setAttribute("aria-valuemax", String(maximum())); grip.setAttribute("aria-valuenow", String(current()));
  };
  const save = (width: number) => {
    preferred = Math.round(Math.max(240, Math.min(maximum(), width))); sync();
    try { environment.storage.setItem(WIDTH_KEY, String(preferred)); } catch { /* Private browsing still permits this session's width. */ }
  };
  let drag: { x: number; width: number; id: number } | undefined;
  grip.addEventListener("pointerdown", event => {
    if (environment.phone() || event.button !== 0) return;
    drag = { x: event.clientX, width: current(), id: event.pointerId };
    grip.setPointerCapture?.(event.pointerId); event.preventDefault(); grip.focus();
  });
  grip.addEventListener("pointermove", event => { if (drag && !environment.phone()) save(drag.width + drag.x - event.clientX); });
  const stop = () => { drag = undefined; };
  grip.addEventListener("pointerup", stop); grip.addEventListener("pointercancel", stop); grip.addEventListener("lostpointercapture", stop);
  grip.onkeydown = event => {
    if (environment.phone()) return;
    const step = event.shiftKey ? 48 : 16;
    const width = event.key === "ArrowLeft" ? current() + step : event.key === "ArrowRight" ? current() - step : event.key === "Home" ? 240 : event.key === "End" ? maximum() : undefined;
    if (width !== undefined) { event.preventDefault(); save(width); }
  };
  shell.append(grip); bindings.set(shell, sync); sync();
  if (win.ResizeObserver) new win.ResizeObserver(sync).observe(shell);
}
