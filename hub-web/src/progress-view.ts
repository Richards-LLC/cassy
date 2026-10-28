import type { ReleaseEpic } from "./progress-model";

/** The release strip reads producer metrics verbatim, never infers merges from closure. */
export function releaseHeader(document: Document, epic: ReleaseEpic | undefined): HTMLElement {
  const header = document.createElement("header"); header.className = "release-progress";
  const eyebrow = document.createElement("p"); eyebrow.className = "release-eyebrow"; eyebrow.textContent = "Release progress";
  const title = document.createElement("h3"); title.textContent = epic?.title || epic?.id || "No release reported";
  header.append(eyebrow, title);
  const progress = epic?.progress;
  const valid = progress && Number.isSafeInteger(progress.total) && Number.isSafeInteger(progress.merged) && progress.total >= progress.merged && progress.merged >= 0;
  const count = document.createElement("p"); count.className = "release-count";
  if (!valid) { count.textContent = "Merge progress not reported"; header.append(count); return header; }
  count.textContent = `${progress.merged} of ${progress.total} merged`;
  const bar = document.createElement("progress"); bar.max = Math.max(1, progress.total); bar.value = progress.merged;
  bar.setAttribute("aria-label", `${title.textContent}: ${count.textContent}`);
  const meta = document.createElement("p"); meta.className = "release-meta";
  const rate = Number.isFinite(progress.merges_per_hour) && progress.merges_per_hour >= 0 ? `${progress.merges_per_hour.toLocaleString(undefined, { maximumFractionDigits: 1 })} merges/hour` : "Merge rate not reported";
  const last = progress.last_merged_at ? new Date(progress.last_merged_at) : undefined;
  meta.textContent = `${rate} · ${last && Number.isFinite(last.getTime()) ? `Last merge ${last.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit", hour12: true })}` : "No merge reported yet"}`;
  header.append(count, bar, meta);
  return header;
}
