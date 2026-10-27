import type { ArtifactViewResult } from "./artifact-open";
import type { ArtifactRef } from "./types";
export function inlineAttachment(sheet: HTMLElement, _file: ArtifactRef, _fetchView: () => Promise<ArtifactViewResult | undefined>): HTMLElement { return sheet; }
