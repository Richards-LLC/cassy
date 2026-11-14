/**
 * Commander Quality sheet view model (cas-3dd7): release-over-release rubrics from the
 * friction ledger (`GET /v1/sessions/{s}/quality`, one `cas ledger report` per release).
 * Rubrics are a registry: adding one is one entry, and a rubric without a signal yet
 * shows its pending source instead of a number. Lower is better for every rubric.
 */
export interface TurnTotals { wakes: number; no_op_wakes: number; input_tokens: number; output_tokens: number }
export interface CheckRow { pass: number; fail: number }
export interface FrictionRow { reference: string; severity?: string | null; title?: string | null; project?: string | null; link?: string | null; state: string; status?: string }
export interface MergedRow { task: string; title?: string | null; merged_at: string }
/** A ledger measure: the value, or why it is unknown. */
export type Measure<T> = T | { unknown: string };
const known = <T>(measure: Measure<T> | undefined): T | undefined => measure === undefined || (typeof measure === "object" && measure !== null && "unknown" in measure) ? undefined : measure as T;
export interface LedgerRelease {
  release: string;
  published: boolean;
  from?: string | null;
  to?: string | null;
  turns: TurnTotals;
  turns_by_actor?: Record<string, Record<string, TurnTotals>>;
  deliveries?: Measure<number>;
  merged?: MergedRow[];
  changes?: Measure<string[]>;
  interventions_total: number;
  interventions: Record<string, number>;
  checks: Record<string, CheckRow>;
  escapes: unknown[];
  friction: FrictionRow[];
}
export interface QualityReport { releases: LedgerRelease[]; supervisors?: Record<string, string> }

export type Actor = "workers" | "supervisors";
type Pending = "session scoring" | "AI judge";
interface Measured {
  actor: Actor; name: string; unit: string;
  /** Rates are counted per merged delivery; per hour appears only in the drill-down. */
  rate?: true;
  read: (release: LedgerRelease) => number | undefined;
  format: (value: number) => string;
}
interface Awaiting { actor: Actor; name: string; unit: string; pending: Pending }
type Rubric = Measured | Awaiting;

const share = (part: number, whole: number) => whole > 0 ? 100 * part / whole : undefined;
const failShare = (row?: CheckRow) => share(row?.fail ?? 0, (row?.pass ?? 0) + (row?.fail ?? 0));
const percent = (value: number) => `${value.toFixed(0)}%`;
const millions = (value: number) => `${value.toFixed(value >= 10 ? 0 : 1)}M`;
const decimal = (value: number) => value.toFixed(value >= 10 ? 0 : value >= 0.1 || value === 0 ? 1 : 2);

export const RUBRICS: readonly Rubric[] = [
  { actor: "workers", name: "Claims the verifier refused", unit: "% of closes", read: r => failShare(r.checks.verifier), format: percent },
  { actor: "workers", name: "Lane check failures", unit: "% of runs", read: r => failShare(r.checks.lane), format: percent },
  { actor: "workers", name: "Escapes", unit: "breakage traced to a delivery", read: r => r.escapes.length, format: decimal },
  { actor: "workers", name: "Context burn", unit: "input tokens per delivery", rate: true, read: r => r.turns.input_tokens / 1e6, format: millions },
  { actor: "workers", name: "Efficiency", unit: "rework, repeated reads, steering", pending: "session scoring" },
  { actor: "workers", name: "Claim honesty", unit: "claims against receipts", pending: "session scoring" },
  { actor: "workers", name: "Unreported workaround", unit: "recoveries never reported", pending: "session scoring" },
  { actor: "workers", name: "Rule breaks", unit: "guard denials per session", pending: "session scoring" },
  { actor: "workers", name: "Waiting", unit: "time blocked on CI, runners or review", pending: "session scoring" },
  { actor: "workers", name: "Scope creep", unit: "diff beyond the task's scope", pending: "session scoring" },
  { actor: "workers", name: "Reproduce first", unit: "fixes reproduced before changing code", pending: "AI judge" },
  { actor: "workers", name: "Redundant tests and verbosity", unit: "per delivery", pending: "AI judge" },
  { actor: "supervisors", name: "Gate overrides", unit: "per delivery", rate: true, read: r => r.interventions_total, format: decimal },
  { actor: "supervisors", name: "Friction filed", unit: "per delivery", rate: true, read: r => r.friction.length, format: decimal },
  { actor: "supervisors", name: "Wakes that did nothing", unit: "% of wakes", read: r => share(r.turns.no_op_wakes, r.turns.wakes), format: percent },
  { actor: "supervisors", name: "Decision latency", unit: "minutes from ask to decision", pending: "session scoring" },
  { actor: "supervisors", name: "Reversed calls", unit: "per release", pending: "session scoring" },
  { actor: "supervisors", name: "Noise", unit: "messages answered “Ignored.”, duplicate relays", pending: "session scoring" },
  { actor: "supervisors", name: "Needless operator pings", unit: "pings the operator didn't need", pending: "AI judge" },
  { actor: "supervisors", name: "Operator-facing quality", unit: "raw ids, UTC, walls of text, wrong advice", pending: "AI judge" },
];
export const ACTORS: readonly [Actor, string][] = [["workers", "Workers"], ["supervisors", "Supervisors"]];

export interface Point { label: string; release: number; value: number | undefined; perHour?: number; hours: number; change?: string }
/** What stands behind one release's numbers: the merges counted and the friction filed. */
export interface ReleaseRecords { label: string; deliveries?: number; coverage?: string; merged: MergedRow[]; friction: FrictionRow[]; wakeSources: WakeSourceRow[] }
export interface Trend { change: string; word: "worse" | "better" | "no change" | "new"; ratio?: number }
export interface RubricRow { actor: Actor; name: string; unit: string; points?: Point[]; trend?: Trend; format?: (value: number) => string; pending?: Pending }
export interface WakeSourceRow { supervisor: string; source: string; wakes: number; noOp: number; average: number; total: number; was?: number }
export interface QualityView {
  current: string; inProgress: boolean; previous?: string; hours: number;
  hero?: RubricRow & { points: Point[]; trend: Trend & { ratio: number } };
  rubrics: RubricRow[];
  interventions: [string, number][];
  serious: FrictionRow[];
  wakeSources: WakeSourceRow[];
  records: ReleaseRecords[];
}

/** "v4.13.0" → "4.13"; work in progress takes the next minor after the newest release. */
export function releaseLabel(releases: readonly LedgerRelease[], index: number): string {
  const short = (name: string) => name.replace(/^v/, "").replace(/\.0$/, "");
  const release = releases[index];
  if (release.published) return short(release.release);
  const last = [...releases.slice(0, index)].reverse().find(item => item.published);
  const match = last?.release.match(/^v?(\d+)\.(\d+)/);
  return match ? `${match[1]}.${Number(match[2]) + 1}` : "next";
}

function windowHours(release: LedgerRelease, now: Date): number {
  if (!release.from) return 0;
  const end = release.to ? Date.parse(release.to) : now.getTime();
  return Math.max(0, (end - Date.parse(release.from)) / 3_600_000);
}

/** Factor since the previous release: "19.4× worse" for big moves, "+12% worse" for small ones. */
export function trendOf(now?: number, was?: number): Trend | undefined {
  if (now === undefined || was === undefined) return undefined;
  if (was === 0) return now === 0 ? { change: "no change", word: "no change" } : { change: "new", word: "new" };
  const ratio = now / was;
  const word = ratio > 1 ? "worse" : ratio < 1 ? "better" : "no change";
  if (word === "no change") return { change: "no change", word, ratio };
  const factor = ratio >= 1.5 ? `${ratio.toFixed(1)}×` : ratio <= 0.67 ? `${(1 / ratio).toFixed(1)}×` : `${ratio > 1 ? "+" : "−"}${Math.abs((ratio - 1) * 100).toFixed(0)}%`;
  return { change: `${factor} ${word}`, word, ratio };
}

export function qualityView(report: QualityReport, now = new Date()): QualityView | undefined {
  const releases = report.releases;
  if (!releases.length) return undefined;
  const labels = releases.map((_, index) => releaseLabel(releases, index));
  const hours = releases.map(release => windowHours(release, now));
  // The skills, guards and gates git shows each release changed.
  const changes = releases.map(release => {
    const names = known(release.changes) ?? [];
    return names.length ? names.slice(0, 4).join(", ") + (names.length > 4 ? ` and ${names.length - 4} more` : "") : undefined;
  });
  const rubrics: RubricRow[] = RUBRICS.map(rubric => {
    if ("pending" in rubric) return { ...rubric };
    const points = releases.map((release, index): Point => {
      const raw = rubric.read(release);
      const deliveries = known(release.deliveries) ?? 0;
      const value = raw === undefined ? undefined : rubric.rate ? (deliveries > 0 ? raw / deliveries : undefined) : raw;
      const perHour = rubric.rate && raw !== undefined && hours[index] > 0 ? raw / hours[index] : undefined;
      return { label: labels[index], release: index, value, perHour, hours: hours[index], change: changes[index] };
    });
    const trend = trendOf(points.at(-1)?.value, points.at(-2)?.value);
    return { actor: rubric.actor, name: rubric.name, unit: rubric.unit, points, trend, format: rubric.format };
  });
  // Inside each actor group, the biggest move worse comes first; awaiting rubrics last.
  const order = (row: RubricRow) => row.pending ? -Infinity : row.trend?.word === "new" ? Infinity : row.trend?.ratio ?? 0;
  rubrics.sort((a, b) => ACTORS.findIndex(([key]) => key === a.actor) - ACTORS.findIndex(([key]) => key === b.actor) || order(b) - order(a));
  const movers = rubrics.filter((row): row is RubricRow & { points: Point[]; trend: Trend & { ratio: number } } => row.trend?.ratio !== undefined);
  const hero = movers.length ? movers.reduce((best, row) => row.trend.ratio > best.trend.ratio ? row : best) : undefined;
  const current = releases.at(-1)!;
  const names = report.supervisors ?? {};
  // Each release's supervisor tokens by wake source, against the release before it.
  const wakeSourcesOf = (index: number): WakeSourceRow[] => Object.entries(releases[index].turns_by_actor ?? {}).flatMap(([actor, sources]) =>
    Object.entries(sources).map(([source, totals]) => {
      const before = releases[index - 1]?.turns_by_actor?.[actor]?.[source];
      return {
        supervisor: names[actor] ?? `supervisor ${actor.slice(0, 8)}`, source,
        wakes: totals.wakes, noOp: totals.no_op_wakes, total: totals.input_tokens,
        average: totals.wakes ? totals.input_tokens / totals.wakes : 0,
        was: before?.wakes ? before.input_tokens / before.wakes : undefined,
      };
    })).sort((a, b) => b.total - a.total);
  return {
    current: labels.at(-1)!, inProgress: !current.published, previous: labels.at(-2), hours: hours.at(-1)!, hero, rubrics,
    interventions: Object.entries(current.interventions).sort((a, b) => b[1] - a[1]),
    serious: current.friction.filter(row => row.severity === "p0" || row.severity === "p1")
      .sort((a, b) => (a.severity ?? "").localeCompare(b.severity ?? "") || Number(a.state !== "open") - Number(b.state !== "open")).slice(0, 8),
    wakeSources: wakeSourcesOf(releases.length - 1),
    records: releases.map((release, index) => {
      const deliveries = release.deliveries;
      return {
        label: labels[index], deliveries: known(deliveries), merged: release.merged ?? [], friction: release.friction, wakeSources: wakeSourcesOf(index),
        coverage: deliveries !== undefined && typeof deliveries === "object" ? deliveries.unknown : deliveries === undefined ? "this machine's Cassy reports no merge record" : undefined,
      };
    }),
  };
}

/** Token counts for people: 2.4M, 950K, 12. */
export function tokens(value: number): string {
  if (value >= 1e9) return `${(value / 1e9).toFixed(1)}B`;
  if (value >= 1e6) return `${(value / 1e6).toFixed(value >= 1e7 ? 0 : 1)}M`;
  if (value >= 1e3) return `${(value / 1e3).toFixed(0)}K`;
  return value.toFixed(0);
}
