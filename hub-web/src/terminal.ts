import type { TranscriptSource } from "./transcript-view";

/**
 * A pane's terminal surface, as Commander uses it (cas-0546): mounted in a
 * hidden host, read-only, pinned to the pane's real grid. It keeps the
 * emulator state the Raw output drawer reads as text.
 */
export interface TerminalSurface {
  readonly element: HTMLElement;
  /** The grid, as the reflowed transcript view reads it. */
  readonly transcript: TranscriptSource;
  write(data: Uint8Array): void;
  /**
   * Pin the surface to the pane's real PTY geometry, or pass null to let it
   * measure its own mount again (cas-37f8).
   */
  setAuthoritativeSize(size: { cols: number; rows: number } | null): void;
  /** Painting the hidden grid is skipped: only its text is ever read. */
  setCanvasPainting(enabled: boolean): void;
  dispose(): void;
}

export interface TerminalSurfaceCallbacks {
  onData(data: Uint8Array): void;
  onResize(cols: number, rows: number): void;
  /** Fires once per rendered frame, so a transcript can follow the grid. */
  onRender?(): void;
}

export type TerminalSurfaceFactory = (
  mount: HTMLElement,
  callbacks: TerminalSurfaceCallbacks,
) => Promise<TerminalSurface>;

export { createGhosttyTerminalSurface as createTerminalSurface } from "./terminal/ghostty-adapter";
