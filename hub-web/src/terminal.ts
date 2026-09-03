export interface TerminalSurface {
  readonly element: HTMLElement;
  readonly cols: number;
  readonly rows: number;
  write(data: Uint8Array): void;
  setControlMode(enabled: boolean): void;
  /**
   * Pin the surface to the pane's real PTY geometry, or pass null to let it
   * measure its own mount again (cas-37f8).
   */
  setAuthoritativeSize(size: { cols: number; rows: number } | null): void;
  focus(): void;
  search(query: string): boolean;
  dispose(): void;
}

export interface TerminalSurfaceCallbacks {
  onData(data: Uint8Array): void;
  onResize(cols: number, rows: number): void;
}

export type TerminalSurfaceFactory = (
  mount: HTMLElement,
  callbacks: TerminalSurfaceCallbacks,
) => Promise<TerminalSurface>;

export { createGhosttyTerminalSurface as createTerminalSurface } from "./terminal/ghostty-adapter";
