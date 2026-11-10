import "./quality-panel.css";
import { escapeHtml } from "./cloud-brand";
import { ACTORS, qualityView, tokens, type Point, type QualityReport, type QualityView, type RubricRow } from "./quality-model";

export interface QualityHost {
  load(signal: AbortSignal): Promise<QualityReport>;
  returnFocus(): void;
}

const fmt = (row: RubricRow, value: number | undefined) => value === undefined || !row.format ? "–" : row.format(value);

/** One point per release; the newest segment in verdict colour, ▾ under releases that changed a gate or skill. */
function chart(row: RubricRow & { points: Point[] }, hero: boolean): string {
  const points = row.points.map((point, index) => ({ ...point, index })).filter((point): point is Point & { index: number; value: number } => point.value !== undefined);
  if (points.length < 2) return "";
  const [width, height] = hero ? [560, 200] : [120, 28];
  const [left, right, top, bottom] = hero ? [24, 56, 22, 34] : [2, 2, 4, 4];
  const w = width - left - right, h = height - top - bottom, count = row.points.length;
  const peak = Math.max(...points.map(point => point.value)) * 1.15 || 1;
  const x = (index: number) => left + (count > 1 ? index * w / (count - 1) : 0);
  const y = (value: number) => top + h - value / peak * h;
  const parts = [`<line x1="${left}" y1="${top + h}" x2="${left + w}" y2="${top + h}" class="quality-axis"/>`];
  row.points.forEach((point, index) => {
    if (!point.change) return;
    parts.push(`<line x1="${x(index).toFixed(1)}" y1="${top}" x2="${x(index).toFixed(1)}" y2="${top + h}" class="quality-marker"/>`);
    if (hero) parts.push(`<path d="M${(x(index) - 4).toFixed(1)},${top + h + 4} l4,6 l4,-6 z" class="quality-marker-head"><title>${escapeHtml(point.label)} changed: ${escapeHtml(point.change)}</title></path>`);
  });
  const path = (list: typeof points) => list.map((point, i) => `${i ? "L" : "M"}${x(point.index).toFixed(1)},${y(point.value).toFixed(1)}`).join(" ");
  parts.push(`<path d="${path(points.slice(0, -1))}" class="quality-series"/>`, `<path d="${path(points.slice(-2))}" class="quality-lead"/>`);
  for (const point of points) {
    const lead = point === points.at(-1);
    parts.push(`<circle cx="${x(point.index).toFixed(1)}" cy="${y(point.value).toFixed(1)}" r="${hero ? (lead ? 5 : 2.5) : (lead ? 3 : 1.6)}" class="${lead ? "quality-dot-lead" : "quality-dot"}"/>`);
    if (!hero) continue;
    const below = y(point.value) - top < 18 && !lead;
    parts.push(`<text x="${(x(point.index) + (lead ? 8 : 0)).toFixed(1)}" y="${(below ? y(point.value) + 16 : y(point.value) - 8).toFixed(1)}" class="${lead ? "quality-value-lead" : "quality-value"}" text-anchor="${lead ? "start" : "middle"}">${escapeHtml(fmt(row, point.value))}</text>`);
  }
  if (hero) row.points.forEach((point, index) => parts.push(`<text x="${x(index).toFixed(1)}" y="${top + h + 26}" class="quality-tick" text-anchor="middle">${escapeHtml(point.label)}</text>`));
  return `<svg viewBox="0 0 ${width} ${height}" role="img" aria-label="${escapeHtml(row.name)} by release, ${escapeHtml(row.points[0].label)} to ${escapeHtml(row.points.at(-1)!.label)}" preserveAspectRatio="none" class="quality-chart${hero ? "" : " quality-spark"}">${parts.join("")}</svg>`;
}

/** The drill-down: every release's value, per hour for rates, and its window. */
function dataTable(row: RubricRow & { points: Point[] }): string {
  const perHour = row.points.some(point => point.perHour !== undefined);
  const head = `<tr><th scope="col">Release</th><th scope="col">${escapeHtml(row.unit)}</th>${perHour ? '<th scope="col">Per hour</th>' : ""}<th scope="col">Window</th></tr>`;
  const body = row.points.map(point => `<tr><th scope="row">${escapeHtml(point.label)}</th><td>${escapeHtml(fmt(row, point.value))}</td>${perHour ? `<td>${escapeHtml(fmt(row, point.perHour))}</td>` : ""}<td>${point.hours.toFixed(1)} h</td></tr>`).join("");
  const changes = row.points.filter(point => point.change).map(point => `${escapeHtml(point.label)}: ${escapeHtml(point.change!)}`);
  return `<table class="quality-data"><thead>${head}</thead><tbody>${body}</tbody></table>${changes.length ? `<p>▾ changed a gate or skill. ${changes.join("; ")}.</p>` : ""}`;
}

function rubricRow(row: RubricRow): string {
  if (!row.points) return `<li class="quality-rubric quality-pending"><span class="quality-now">–</span><span class="quality-name">${escapeHtml(row.name)}<small>${escapeHtml(row.unit)}</small></span><span class="quality-source">awaiting ${escapeHtml(row.pending ?? "scoring")}</span></li>`;
  const points = row.points, word = row.trend?.word ?? "no change";
  return `<li class="quality-rubric" data-trend="${escapeHtml(word)}"><span class="quality-now">${escapeHtml(fmt(row, points.at(-1)?.value))}</span><span class="quality-name">${escapeHtml(row.name)}<small>${escapeHtml(row.unit)}</small></span><span class="quality-spark-cell">${chart({ ...row, points }, false)}</span><span class="quality-was">was ${escapeHtml(fmt(row, points.at(-2)?.value))}</span><span class="quality-change">${escapeHtml(row.trend?.change ?? "no data")}</span><details class="quality-row-data"><summary>Data</summary>${dataTable({ ...row, points })}</details></li>`;
}

export function qualityMarkup(view: QualityView): string {
  const hero = view.hero;
  const heroMarkup = hero ? `<section class="quality-hero" aria-labelledby="quality-verdict"><div><p class="quality-eyebrow">Moved most this release · ${escapeHtml(hero.actor)}</p><h3 id="quality-verdict" class="quality-verdict">${escapeHtml(hero.name)} <em class="quality-factor">${escapeHtml(hero.trend.change)}</em> in ${escapeHtml(view.current)}</h3><p class="quality-why"><strong>${escapeHtml(fmt(hero, hero.points.at(-1)?.value))}</strong> against <strong>${escapeHtml(fmt(hero, hero.points.at(-2)?.value))}</strong> ${escapeHtml(hero.unit)}.</p></div><figure class="quality-figure"><figcaption class="quality-eyebrow">${escapeHtml(hero.name)}, ${escapeHtml(hero.unit)}, by release</figcaption>${chart(hero, true)}<details><summary>Data and change markers</summary>${dataTable(hero)}</details></figure></section>` : "";
  const groups = ACTORS.map(([actor, title]) => {
    const rows = view.rubrics.filter(row => row.actor === actor);
    return `<section class="quality-group" aria-label="${escapeHtml(title)}"><h4>${escapeHtml(title)} <span>${rows.filter(row => row.points).length} of ${rows.length} measured</span></h4><ul class="quality-rubrics">${rows.map(rubricRow).join("")}</ul></section>`;
  }).join("");
  const wake = view.wakeSources.length ? `<table class="quality-data quality-wakes"><caption class="quality-eyebrow">Supervisor tokens by wake source, ${escapeHtml(view.current)}</caption><thead><tr><th scope="col">Supervisor</th><th scope="col">Woken by</th><th scope="col">Wakes</th><th scope="col">No-op</th><th scope="col">Avg input</th><th scope="col">Was</th><th scope="col">Total</th></tr></thead><tbody>${view.wakeSources.map(row => `<tr><th scope="row">${escapeHtml(row.supervisor)}</th><td>${escapeHtml(row.source)}</td><td>${row.wakes}</td><td>${row.noOp}</td><td>${tokens(row.average)}</td><td>${row.was === undefined ? "–" : tokens(row.was)}</td><td>${tokens(row.total)}</td></tr>`).join("")}</tbody></table>` : "";
  const mix = view.interventions.length ? `<ul class="quality-mix">${view.interventions.map(([kind, count]) => `<li><b>${count}</b><span>${escapeHtml(kind.replace(/_/g, " "))}</span></li>`).join("")}</ul>` : '<p class="quality-quiet">No gate overrides.</p>';
  const serious = view.serious.length ? `<table class="quality-drill"><caption class="quality-eyebrow">Serious friction filed this release</caption><tbody>${view.serious.map(row => `<tr><td class="quality-sev" data-severity="${escapeHtml(row.severity ?? "")}">${escapeHtml((row.severity ?? "").toUpperCase())}</td><td>${escapeHtml((row.title ?? "").split(" (GH")[0])}</td><td class="quality-state">${escapeHtml(row.state)}</td><td class="quality-ref">${escapeHtml(row.reference)}</td></tr>`).join("")}</tbody></table>` : '<p class="quality-quiet">No P0 or P1 friction filed this release.</p>';
  return `${heroMarkup}<section class="quality-card" aria-labelledby="quality-rubrics-title"><h3 id="quality-rubrics-title">Every rubric, by who it scores</h3><div class="quality-groups">${groups}</div></section><section class="quality-card" aria-labelledby="quality-behind-title"><h3 id="quality-behind-title">Behind the numbers</h3><div class="quality-split"><div><p class="quality-eyebrow">Gate overrides, by kind</p>${mix}</div>${serious}</div>${wake}<p class="quality-folio">Source: the friction ledger, ${escapeHtml(view.rubrics.find(row => row.points)?.points?.[0]?.label ?? view.current)} to ${escapeHtml(view.current)}${view.inProgress ? " in progress" : ""}. Rates are per merged delivery, a task whose close the verifier accepted; each row's data shows per hour.</p></section>`;
}

/** Body-mounted modal, as Accounts: shell redraws leave it alone. */
export class QualitySheet {
  private dialog?: HTMLDialogElement;
  private loads = new AbortController();
  constructor(private readonly host: QualityHost, private readonly doc: Document = document) {}

  open(project: string): void {
    const dialog = this.ensureDialog();
    this.loads.abort(); this.loads = new AbortController();
    const signal = this.loads.signal;
    dialog.querySelector(".quality-project")!.textContent = project;
    const body = dialog.querySelector<HTMLElement>(".quality-body")!;
    body.innerHTML = '<p role="status">Reading the friction ledger…</p>';
    if (!dialog.open) dialog.showModal();
    dialog.querySelector<HTMLButtonElement>("[data-quality-close]")!.focus();
    void this.host.load(signal).then(report => {
      if (signal.aborted) return;
      const view = qualityView(report);
      dialog.querySelector(".quality-window")!.textContent = view ? `${view.current}${view.inProgress ? " in progress" : ""} · ${view.hours.toFixed(0)} h${view.inProgress ? " so far" : ""}${view.previous ? ` · vs ${view.previous}` : ""}` : "";
      body.innerHTML = view ? qualityMarkup(view) : '<p class="quality-quiet">No releases in this project\'s friction ledger yet.</p>';
    }, () => {
      if (signal.aborted) return;
      body.innerHTML = '<p role="alert">Could not read this project\'s friction ledger. The machine may be offline or on an older Cassy. <button type="button" data-quality-retry>Try again</button></p>';
    });
  }

  private ensureDialog(): HTMLDialogElement {
    if (this.dialog?.isConnected) return this.dialog;
    const dialog = this.doc.createElement("dialog");
    dialog.className = "quality-sheet"; dialog.setAttribute("aria-labelledby", "quality-title");
    dialog.innerHTML = '<header class="quality-heading"><p class="quality-lockup"><b id="quality-title">Quality</b> <span class="quality-project"></span> <span class="quality-window"></span></p><button type="button" data-quality-close aria-label="Close quality">Close</button></header><div class="quality-body"></div>';
    this.doc.body.append(dialog); this.dialog = dialog;
    dialog.addEventListener("close", () => { this.loads.abort(); this.host.returnFocus(); });
    dialog.addEventListener("click", event => {
      const button = (event.target as Element).closest<HTMLButtonElement>("button");
      if (button?.hasAttribute("data-quality-close")) dialog.close();
      if (button?.hasAttribute("data-quality-retry")) this.open(dialog.querySelector(".quality-project")!.textContent ?? "");
    });
    return dialog;
  }
}
