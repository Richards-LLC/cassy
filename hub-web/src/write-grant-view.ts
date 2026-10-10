import type { FleetTask } from "./fleet-ops";
import type { WriteGrantState } from "./write-grant";

export interface WriteAccessHandlers {
  changed(): void;
  review(): void;
  revoke(): void;
  confirm(): void;
  cancel(): void;
}

export interface WriteAccessContext {
  readonly state: WriteGrantState;
  readonly tasks: readonly FleetTask[];
  readonly on: WriteAccessHandlers;
}

export function writeAccessPanel(document: Document, _context: WriteAccessContext): HTMLElement {
  return document.createElement("div");
}
