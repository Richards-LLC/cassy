import type { TranscriptSource } from "./transcript-view";

export interface TerminalSurface {
  readonly element: HTMLElement;
  readonly cols: number;
  readonly rows: number;
  /** The grid, as the reflowed transcript view reads it. */
  readonly transcript: TranscriptSource;
  write(data: Uint8Array): void;
  setControlMode(enabled: boolean): void;
  /**
   * Pin the surface to the pane's real PTY geometry, or pass null to let it
   * measure its own mount again (cas-37f8).
   */
  setAuthoritativeSize(size: { cols: number; rows: number } | null): void;
  /** Floor on the columns handed to the PTY, independent of the CSS width. */
  setMinimumColumns(columns: number): void;
  /** Painting the hidden grid is skipped while the transcript is the visible view. */
  setCanvasPainting(enabled: boolean): void;
  focus(): void;
  search(query: string): boolean;
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
