import { epicCounts, epicTitle, type EpicCounts, type ReleaseEpic } from "./progress-model";

/** "4 open · 2 in progress · 6 merged", the factory TUI's split in words. */
export function splitText(counts: EpicCounts): string {
  return `${counts.open} open · ${counts.in_progress} in progress · ${counts.merged} merged`;
}

/** A segmented bar: merged solid, in progress hatched, open as the track.
 * Segment widths are proportions of a fixed track, so a count update never
 * moves the surrounding layout. */
export function splitBar(document: Document, counts: EpicCounts & { total: number }, name: string): HTMLElement {
  const bar = document.createElement("div"); bar.className = "split-bar";
  bar.setAttribute("role", "img");
  bar.setAttribute("aria-label", `${name}: ${splitText(counts)}`);
  for (const [part, value] of [["merged", counts.merged], ["in-progress", counts.in_progress]] as const) {
    if (!value) continue;
    const segment = document.createElement("span"); segment.className = `split-bar-${part}`;
    segment.style.flexBasis = `${(value / Math.max(1, counts.total)) * 100}%`;
    bar.append(segment);
  }
  return bar;
}

/** Merges in the last hour: the producer counts whole merges; older fixtures may carry a rate. */
function rateText(rate: number): string {
  if (!Number.isFinite(rate) || rate < 0) return "Merge rate not reported";
  if (Number.isInteger(rate)) return `${rate} ${rate === 1 ? "merge" : "merges"} in the last hour`;
  return `${rate.toLocaleString(undefined, { maximumFractionDigits: 1 })} merges/hour`;
}

export interface TrainActivity { readonly building: number; readonly parked: number; readonly blocked: number; readonly workers: number }

/** The release header reads producer counts verbatim, never infers merges from closure. */
export function releaseHeader(document: Document, epic: ReleaseEpic | undefined, activity?: TrainActivity): HTMLElement {
  const header = document.createElement("header"); header.className = "release-progress";
  const eyebrow = document.createElement("p"); eyebrow.className = "release-eyebrow"; eyebrow.textContent = "Release progress";
  const title = document.createElement("h3"); title.textContent = epic ? epicTitle(epic) || "Untitled release" : "No release reported";
  if (epic?.title) title.title = epic.title;
  header.append(eyebrow, title);
  if (epic?.freeze?.summary) {
    const freeze = document.createElement("p"); freeze.className = "release-freeze"; freeze.textContent = epic.freeze.summary;
    header.append(freeze);
  }
  const counts = epicCounts(epic);
  const progress = epic?.progress;
  const valid = progress && Number.isSafeInteger(progress.total) && Number.isSafeInteger(progress.merged) && progress.total >= progress.merged && progress.merged >= 0;
  const count = document.createElement("p"); count.className = "release-count";
  if (!counts && !valid) { count.textContent = "Merge progress not reported"; header.append(count); return header; }
  const [merged, total] = counts ? [counts.merged, counts.total] : [progress!.merged, progress!.total];
  count.textContent = `${merged} of ${total} merged`;
  header.append(count);
  if (counts) {
    header.append(splitBar(document, counts, title.textContent));
    const split = document.createElement("p"); split.className = "release-split"; split.textContent = splitText(counts);
    header.append(split);
  } else {
    const bar = document.createElement("progress"); bar.max = Math.max(1, total); bar.value = merged;
    bar.setAttribute("aria-label", `${title.textContent}: ${count.textContent}`);
    header.append(bar);
  }
  const meta = document.createElement("p"); meta.className = "release-meta";
  const last = progress?.last_merged_at ? new Date(progress.last_merged_at) : undefined;
  meta.textContent = [
    counts ? `${counts.open + counts.in_progress} to go` : undefined,
    valid ? rateText(progress!.merges_per_hour) : undefined,
    last && Number.isFinite(last.getTime()) ? `Last merge ${last.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit", hour12: true })}` : "Last merge not reported",
  ].filter(Boolean).join(" · ");
  header.append(meta);
  if (activity) {
    const parts = [[activity.building, "building"], [activity.parked, "parked"], [activity.blocked, "blocked"]] as const;
    const line = document.createElement("p"); line.className = "release-activity";
    line.textContent = [...parts.filter(([value]) => value > 0).map(([value, label]) => `${value} ${label}`),
      `${activity.workers} ${activity.workers === 1 ? "worker" : "workers"}`].join(" · ");
    header.append(line);
  }
  return header;
}

/** The worker card's reported task, with identity kept available. */
export function workerTaskLine(document: Document, worker: { readonly work: string; readonly currentTask?: string }): HTMLElement {
  const work = document.createElement("p"); work.className = "status-activity";
  work.textContent = worker.work;
  if (worker.currentTask) work.title = worker.currentTask;
  return work;
}
