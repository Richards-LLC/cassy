import type { TerminalSurface, TerminalSurfaceCallbacks } from "../terminal";
import { GhosttyTerminalSurface } from "./ghostty/surface";

const theme = {
  background: { r: 13, g: 17, b: 23 },
  foreground: { r: 201, g: 209, b: 217 },
  cursor: { r: 88, g: 166, b: 255 },
  palette: [
    { r: 13, g: 17, b: 23 }, { r: 255, g: 123, b: 114 },
    { r: 63, g: 185, b: 80 }, { r: 210, g: 153, b: 34 },
    { r: 88, g: 166, b: 255 }, { r: 188, g: 140, b: 255 },
    { r: 57, g: 197, b: 207 }, { r: 201, g: 209, b: 217 },
    { r: 110, g: 118, b: 129 }, { r: 255, g: 167, b: 160 },
    { r: 86, g: 211, b: 100 }, { r: 226, g: 179, b: 65 },
    { r: 121, g: 192, b: 255 }, { r: 210, g: 168, b: 255 },
    { r: 86, g: 212, b: 221 }, { r: 240, g: 246, b: 252 },
  ],
};

function applicationTerminalFont(): { family: string; size: number } {
  const styles = getComputedStyle(document.documentElement);
  const family = styles.getPropertyValue("--font-mono").trim();
  const sizeToken = styles.getPropertyValue("--fs-terminal").trim();
  const rootSize = Number.parseFloat(styles.fontSize);
  const tokenSize = Number.parseFloat(sizeToken);
  const size = sizeToken.endsWith("rem") ? tokenSize * rootSize : tokenSize;
  return {
    family: family || '"JetBrains Mono", "IBM Plex Mono", monospace',
    size: Number.isFinite(size) ? size : 13,
  };
}

export async function createGhosttyTerminalSurface(
  mount: HTMLElement,
  callbacks: TerminalSurfaceCallbacks,
): Promise<TerminalSurface> {
  const decoder = new TextDecoder();
  const encoder = new TextEncoder();
  const surface = await GhosttyTerminalSurface.create(mount, {
    theme,
    font: applicationTerminalFont(),
    onData: (data) => callbacks.onData(encoder.encode(data)),
    onResize: callbacks.onResize,
    onSelectionChange: () => undefined,
    onCopy: (text) => void navigator.clipboard.writeText(text),
    beforeKey: () => true,
    onLinkActivate: (text) => {
      if (text.startsWith("https://")) window.open(text, "_blank", "noopener,noreferrer");
    },
  });
  return {
    element: mount,
    get cols() { return surface.cols; },
    get rows() { return surface.rows; },
    write(data) { surface.write(decoder.decode(data, { stream: true })); },
    setControlMode(enabled) { surface.setControlMode(enabled); },
    setAuthoritativeSize(size) { surface.setAuthoritativeSize(size); },
    focus() { surface.focus(); },
    search(query) { return surface.search(query); },
    dispose() { surface.dispose(); },
  };
}
