export interface TerminalSurface {
  readonly element: HTMLElement;
  readonly cols: number;
  readonly rows: number;
  write(data: Uint8Array): void;
  setControlMode(enabled: boolean): void;
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
